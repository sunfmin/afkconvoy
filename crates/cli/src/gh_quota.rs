//! Expiring observations of GitHub quotas, never fleet state. Each login on
//! each host has separate core/GraphQL observations and a shared secondary
//! cooldown. Reads of this note precede requests, including in a later tick or
//! another CLI process. Losing a note costs a rejected request, never a fact.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::gh::GitHub;
use crate::look::Reading;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Api {
    Core,
    Graphql,
}

impl Api {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Graphql => "graphql",
        }
    }
    pub(crate) fn other(self) -> Self {
        match self {
            Self::Core => Self::Graphql,
            Self::Graphql => Self::Core,
        }
    }
    pub(crate) fn of(args: &[&str]) -> Option<Self> {
        match args {
            ["api", "graphql", ..]
            | ["issue", "view", ..]
            | ["pr" | "issue", "list" | "create" | "close" | "edit", ..] => Some(Self::Graphql),
            ["api", ..] => Some(Self::Core),
            _ => None,
        }
    }
}

/// One login's quota on one GitHub host, as this host observed it: its
/// notes are rows of the host database's `github_quota`, and the locks its
/// shared readings are read under are files of `locks`.
#[derive(Clone, Debug)]
pub(crate) struct Scope {
    db: crate::host_db::Host,
    locks: PathBuf,
    pub(crate) host: String,
    pub(crate) login: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Observation {
    pub(crate) remaining: Option<u64>,
    pub(crate) reset: u64,
    /// The hour's quota, as GitHub said it with `remaining`; none where it
    /// said none (a rejection's own words).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) limit: Option<u64>,
}

/// The longest a hold grows to by itself: a `Retry-After` GitHub says that
/// is longer still is waited in full.
pub(crate) const LONGEST_HOLD: u64 = 15 * 60;

/// How far back the write ledger reaches: the writes a hold names as run
/// before the one that met the limit.
const LEDGER: u64 = 60;

/// GitHub's secondary limit as the host holds it, its note `secondary` in the host database: every
/// afkconvoy process of the login on the host waits on it before any GitHub
/// write. Met again soon after a hold ends, the next hold waits twice as long,
/// up to `LONGEST_HOLD`, so the host's writers stay quiet long enough for
/// GitHub to lift its block instead of extending it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Hold {
    #[serde(flatten)]
    pub(crate) seen: Observation,
    /// How long this hold waits, in seconds.
    #[serde(default)]
    pub(crate) wait: u64,
    /// How many holds in a row this one is: each met the limit before the
    /// last one was quiet for the wait the next would take.
    #[serde(default)]
    pub(crate) trips: u64,
    /// The write that first met the limit in this row of holds, and the
    /// writes the host ran in the minute before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first: Option<Tripped>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Tripped {
    pub(crate) what: String,
    pub(crate) at: u64,
    pub(crate) before: Vec<Wrote>,
}

/// One GitHub write a process of the host made, named by its kind
/// (`gh::writes`), in the host's ledger of the last minute's writes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Wrote {
    pub(crate) what: String,
    pub(crate) at: u64,
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn segment(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

static HOSTS: Mutex<BTreeMap<PathBuf, String>> = Mutex::new(BTreeMap::new());
/// The login's quota scope per host. It stands for the process's life:
/// nobody changes `gh`'s login under a running runner.
pub(crate) static SCOPES: Reading<Result<Scope, String>> = Reading::per_process("gh quota scope");

/// Whether this look has observed the quota note of a login, per its note
/// directory. A process that is one look — `doctor` — observes at most once.
pub(crate) static NOTE: Reading<()> = Reading::per_look("gh quota note");

/// One `rate_limit` body per host in this look. `observe` and an env-token
/// login both read the note; the second of them uses this body.
pub(crate) static RATES: Reading<Result<String, String>> = Reading::per_look("gh rate_limit");

/// The `rate_limit` body for `host`, read once in this look. A second ask
/// waits on that read instead of calling `rate_limit` again.
fn rate_limit_body(gh: &GitHub, host: &str) -> Result<String> {
    RATES
        .read(host, || match gh.raw(&["api", "rate_limit", "--hostname", host]) {
            Ok(body) => Ok(body),
            Err(error) => Err(format!("{error:#}")),
        })
        .map_err(|error| anyhow::anyhow!(error))
}

/// The GitHub host a checkout uses, read once per checkout in this process.
fn host(gh: &GitHub) -> String {
    if let Ok(host) = std::env::var("GH_HOST") {
        return host.to_ascii_lowercase();
    }
    let mut hosts = HOSTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    hosts
        .entry(gh.dir.clone())
        .or_insert_with_key(|dir| {
            let host = crate::census::command("git")
                .current_dir(dir)
                .args(["remote", "get-url", "origin"])
                .output()
                .ok()
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .and_then(|remote| {
                    let remote = remote.trim();
                    if let Some((_, rest)) = remote.split_once("://") {
                        rest.split('/').next()?.rsplit('@').next()?.split(':').next()
                    } else if let Some((_, rest)) = remote.split_once('@') {
                        rest.split(':').next()
                    } else {
                        None
                    }
                    .map(str::to_string)
                })
                .unwrap_or_else(|| "github.com".into());
            host.to_ascii_lowercase()
        })
        .clone()
}

impl Scope {
    /// The login's quota scope on this host, discovered once in this process.
    pub(crate) fn of(gh: &GitHub) -> Result<Self> {
        let host = host(gh);
        SCOPES
            .read(&host, || Self::discover(gh, &host).map_err(|error| format!("{error:#}")))
            .map_err(|error| anyhow::anyhow!(error))
    }

    fn discover(gh: &GitHub, host: &str) -> Result<Self> {
        // `auth status` makes hidden API calls (GraphQL for env tokens).
        // The configured active user is local; an overriding token needs its
        // own authoritative identity, bound to an opaque credential fingerprint.
        let local = gh.raw(&["config", "get", "user", "--host", host]).unwrap_or_default();
        let local = local.trim();
        let login = match credential_fingerprint(gh, local)? {
            None => local.to_string(),
            Some(key) => environment_login(gh, host, &key)?,
        };
        if login.is_empty() || login.lines().count() != 1 {
            bail!(
                "GitHub quota identity unavailable for {host}; run gh auth status --hostname {host}"
            );
        }
        let cache = crate::copies::cache()?;
        set_aside_old_notes(&cache)?;
        let locks = cache.join(LOCKS).join(segment(host)).join(segment(&login));
        Ok(Self { db: crate::host_db::Host::of_host()?, locks, host: host.to_string(), login })
    }

    /// Where the host's shared readings of this login are kept (`crate::shared_reading`).
    pub(crate) fn shared(&self) -> crate::shared_reading::Shared {
        crate::shared_reading::Shared {
            db: self.db.clone(),
            scope: format!("{}/{}", self.host, self.login),
            locks: self.locks.clone(),
        }
    }

    pub(crate) fn read(&self, name: &str) -> Result<Observation> {
        self.get(&self.db.open()?, name)
    }

    /// The note `name` as `db` holds it; the default one where none.
    fn get<T: serde::de::DeserializeOwned + Default>(
        &self,
        db: &rusqlite::Connection,
        name: &str,
    ) -> Result<T> {
        use rusqlite::OptionalExtension;
        let note: Option<String> = db
            .query_row(
                "SELECT note FROM github_quota WHERE host = ?1 AND login = ?2 AND name = ?3",
                [&self.host, &self.login, name],
                |row| row.get(0),
            )
            .optional()?;
        let Some(note) = note else { return Ok(T::default()) };
        serde_json::from_str(&note).with_context(|| {
            format!(
                "unreadable GitHub quota note {name} of {}/{} in the host database",
                self.host, self.login
            )
        })
    }

    /// Makes `db` hold `value` as the note `name`.
    fn put(&self, db: &rusqlite::Connection, name: &str, value: &impl Serialize) -> Result<()> {
        db.execute(
            "INSERT OR REPLACE INTO github_quota (host, login, name, note) VALUES (?1, ?2, ?3, ?4)",
            [&self.host, &self.login, name, &serde_json::to_string(value)?],
        )?;
        Ok(())
    }

    /// Runs `change` in one transaction of the host database: every change of
    /// the note is made so, by whichever process of the host.
    fn change<R>(&self, change: impl FnOnce(&rusqlite::Transaction) -> Result<R>) -> Result<R> {
        let mut db = self.db.open()?;
        crate::host_db::write(&mut db, change)
    }

    /// The host's hold of the secondary limit; a default one, long over,
    /// where none was ever made.
    pub(crate) fn hold(&self) -> Result<Hold> {
        self.get(&self.db.open()?, "secondary")
    }

    /// Notes that a process of the host made the GitHub write `what` at `at`.
    /// The ledger keeps the last minute's.
    pub(crate) fn wrote(&self, what: &str, at: u64) -> Result<()> {
        self.change(|tx| {
            let mut ledger: Vec<Wrote> = self.get(tx, "writes").unwrap_or_default();
            ledger.retain(|w| w.at.saturating_add(LEDGER) > at);
            ledger.push(Wrote { what: what.into(), at });
            self.put(tx, "writes", &ledger)
        })
    }

    /// The write `what` met GitHub's secondary limit at `now`, which GitHub
    /// lifts at `said` at the earliest → the instant the host's hold ends.
    /// A hold still standing is not grown by a write that was in flight
    /// before it; one met again within the wait the next hold would take of
    /// the last one's end waits twice the last one's, up to `LONGEST_HOLD`.
    pub(crate) fn trip(&self, what: &str, now: u64, said: u64) -> Result<u64> {
        self.change(|tx| {
            let prior: Hold = self.get(tx, "secondary")?;
            if prior.seen.reset > now {
                let mut hold = prior;
                hold.seen.reset = hold.seen.reset.max(said);
                self.put(tx, "secondary", &hold)?;
                return Ok(hold.seen.reset);
            }
            let floor = said.saturating_sub(now);
            let next = prior.wait.saturating_mul(2).min(LONGEST_HOLD);
            let again = prior.trips > 0 && now < prior.seen.reset.saturating_add(next);
            let (wait, trips, first) = if again {
                (floor.max(next), prior.trips + 1, prior.first)
            } else {
                let ledger: Vec<Wrote> = self.get(tx, "writes").unwrap_or_default();
                let before = ledger.into_iter().filter(|w| w.at.saturating_add(LEDGER) > now);
                (floor, 1, Some(Tripped { what: what.into(), at: now, before: before.collect() }))
            };
            let reset = now.saturating_add(wait).max(said);
            let hold = Hold {
                seen: Observation { remaining: Some(0), reset, limit: None },
                wait,
                trips,
                first,
            };
            self.put(tx, "secondary", &hold)?;
            Ok(hold.seen.reset)
        })
    }

    /// Keeps `value` as the observation `name`, as it adds to what was kept,
    /// in one transaction of the host database.
    pub(crate) fn save(&self, name: &str, value: &Observation) -> Result<()> {
        self.change(|tx| self.save_in(tx, name, value))
    }

    fn save_in(&self, db: &rusqlite::Connection, name: &str, value: &Observation) -> Result<()> {
        let prior: Observation = self.get(db, name)?;
        // Responses from concurrent requests may arrive out of order. A late
        // positive observation cannot erase a primary rejection before reset.
        if prior.remaining == Some(0) && prior.reset > now() && value.remaining != Some(0) {
            return Ok(());
        }
        let mut value = value.clone();
        value.limit = value.limit.or(prior.limit);
        // Two rejections in flight at once: `api graphql` says its reset in
        // its headers, `pr list` says none and is bounded to a minute. The
        // one saved last does not shorten a reset already known
        // (sunfmin/control#319).
        if prior.remaining == Some(0) && prior.reset > now() && value.remaining == Some(0) {
            value.reset = value.reset.max(prior.reset);
        }
        if prior.reset == value.reset {
            value.remaining = match (prior.remaining, value.remaining) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (_, b) => b,
            };
        }
        self.put(db, name, &value)
    }

    pub(crate) fn describe(&self) -> Result<String> {
        let core = self.read("core")?;
        let gql = self.read("graphql")?;
        let next = [&core, &gql]
            .into_iter()
            .filter(|r| r.remaining == Some(0) && r.reset > now())
            .map(|r| r.reset)
            .min();
        Ok(format!(
            "{}/{}: REST core remaining={:?} reset={}; GraphQL remaining={:?} reset={}; next_attempt={next:?}",
            self.host, self.login, core.remaining, core.reset, gql.remaining, gql.reset
        ))
    }

    /// The rate-limit endpoint has its own exemption from primary quotas.
    /// An unavailable observation never clears an exhaustion already seen.
    /// At most one observation is made for this login in this look.
    pub(crate) fn observe(&self, gh: &GitHub) -> Result<()> {
        let mut first = false;
        NOTE.read(&format!("{}/{}", self.host, self.login), || first = true);
        if !first {
            return Ok(());
        }
        let out = match rate_limit_body(gh, &self.host) {
            Ok(out) => out,
            Err(error) => {
                let now = now();
                if let Some(said) = secondary(&error.to_string(), now) {
                    let reset = self.trip("api rate_limit", now, said)?;
                    bail!(
                        "GitHub secondary rate limit on quota observation: both APIs wait until {reset} (Unix seconds); {error}"
                    );
                }
                eprintln!(
                    "GitHub quota observation unavailable; remaining/reset unknown except prior observations: {error:#}"
                );
                return Ok(());
            }
        };
        if let Ok(doc) = serde_json::from_str::<Value>(&out) {
            for api in [Api::Core, Api::Graphql] {
                let row = &doc["resources"][api.name()];
                if let (Some(remaining), Some(reset)) =
                    (row["remaining"].as_u64(), row["reset"].as_u64())
                {
                    let limit = row["limit"].as_u64();
                    self.save(
                        api.name(),
                        &Observation { remaining: Some(remaining), reset, limit },
                    )?;
                }
            }
        } else {
            eprintln!(
                "GitHub quota observation invalid; remaining/reset unknown except prior observations"
            );
        }
        Ok(())
    }
}

/// Only an explicit primary quota rejection permits switching APIs. A 403
/// alone, permissions, auth errors, transport failures and abuse limits do not.
pub(crate) fn primary(said: &str) -> bool {
    let s = said.to_ascii_lowercase();
    secondary(said, 0).is_none()
        && (s.contains("api rate limit exceeded")
            || s.contains("api rate limit already exceeded")
            || s.contains("\"type\":\"rate_limited\""))
}

/// Retry-After is a host/login-wide cooldown, even when the response also
/// mentions the primary quota. Answers the instant the cooldown ends: an HTTP
/// date is that instant exactly; a delay in seconds, or GitHub's one-minute
/// floor when none is said, counts from the caller's one reading of `now`.
pub(crate) fn secondary(said: &str, now: u64) -> Option<u64> {
    let s = said.to_ascii_lowercase();
    if !(s.contains("secondary rate") || s.contains("abuse") || s.contains("retry-after:")) {
        return None;
    }
    let reset = s
        .lines()
        .find_map(|line| {
            let after = line.trim().strip_prefix("retry-after:")?.trim();
            let delay = after.parse::<u64>().ok();
            delay.map(|delay| now.saturating_add(delay)).or_else(|| retry_date(after))
        })
        .unwrap_or(now.saturating_add(60));
    Some(reset)
}

fn retry_date(date: &str) -> Option<u64> {
    let parts: Vec<_> = date.split_whitespace().collect();
    let [_, day, month, year, time, "gmt"] = parts.as_slice() else { return None };
    let month =
        ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
            .iter()
            .position(|m| m == month)?
            + 1;
    let day: u8 = day.parse().ok()?;
    crate::doctor::watching::since(&format!("{year}-{month:02}-{day:02}T{time}Z")).ok()
}

/// Separate response headers from gh api --include's projected bodies, also
/// across --paginate pages. Only response headers are captured, never request
/// headers or authentication material.
pub(crate) fn response(out: &str) -> (String, String) {
    let mut body = String::new();
    let mut headers = String::new();
    let mut in_headers = false;
    for line in out.split_inclusive('\n') {
        if line.starts_with("HTTP/") {
            in_headers = true;
        }
        if in_headers {
            let lower = line.to_ascii_lowercase();
            if line.starts_with("HTTP/")
                || [
                    "x-ratelimit-limit:",
                    "x-ratelimit-remaining:",
                    "x-ratelimit-reset:",
                    "x-ratelimit-resource:",
                    "retry-after:",
                ]
                .iter()
                .any(|key| lower.starts_with(key))
            {
                headers.push_str(line);
            }
            if line.trim().is_empty() {
                in_headers = false;
            }
        } else {
            body.push_str(line);
        }
    }
    (body, headers)
}

impl Scope {
    pub(crate) fn headers(&self, headers: &str) -> Result<()> {
        let fields: std::collections::HashMap<_, _> = headers
            .lines()
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
            .collect();
        if let (Some(resource), Some(remaining), Some(reset)) = (
            fields.get("x-ratelimit-resource"),
            fields.get("x-ratelimit-remaining"),
            fields.get("x-ratelimit-reset"),
        ) && ["core", "graphql"].contains(resource)
        {
            self.save(
                resource,
                &Observation {
                    remaining: Some(remaining.parse()?),
                    reset: reset.parse()?,
                    limit: fields.get("x-ratelimit-limit").and_then(|limit| limit.parse().ok()),
                },
            )?;
        }
        Ok(())
    }

    pub(crate) fn json(&self) -> Result<Value> {
        let core = self.read("core")?;
        let graphql = self.read("graphql")?;
        let secondary = self.hold()?;
        let held = secondary.seen.reset > now();
        Ok(
            serde_json::json!({"host":self.host,"login":self.login,"core":core,"graphql":graphql,"secondary":secondary,
            "for_writes":if held {"wait"}else{"write"},
            "for_rest_reads":if held {"wait"}else if core.remaining==Some(0)&&core.reset>now() {"graphql when equivalent"}else{"core"},
            "for_graphql_reads":if held {"wait"}else if graphql.remaining==Some(0)&&graphql.reset>now() {"core when equivalent"}else{"graphql"}}),
        )
    }
}

/// Fingerprint only: no token is put in argv, written to disk or included in
/// diagnostics. `git hash-object` without -w hashes stdin without storing it.
/// Include the local user as an irrelevant env token may target another host.
fn credential_fingerprint(gh: &GitHub, local: &str) -> Result<Option<String>> {
    let mut bytes = vec![];
    for name in ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"] {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    bytes.extend_from_slice(local.as_bytes());
    let mut child = crate::census::command("git")
        .current_dir(&gh.dir)
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not fingerprint GitHub credential environment")?;
    child.stdin.take().context("credential fingerprint input unavailable")?.write_all(&bytes)?;
    let result = child.wait_with_output()?;
    if !result.status.success() {
        bail!("GitHub credential fingerprint failed; no credential was stored")
    }
    let key = String::from_utf8(result.stdout)?;
    let key = key.trim();
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid GitHub credential fingerprint")
    }
    Ok(Some(key.into()))
}

#[derive(Default, Serialize, Deserialize)]
struct IdentityNote {
    login: Option<String>,
    until: u64,
    why: Option<String>,
}

/// An env credential is immutable for this fingerprint. The binding avoids
/// rediscovering its login through an exhausted API on every subsequent tick.
/// A changed credential/configuration receives a new binding, never the old
/// user's quota. A rejected identity lookup is bounded and persisted too.
fn environment_login(gh: &GitHub, host: &str, key: &str) -> Result<String> {
    use rusqlite::OptionalExtension;
    let db = crate::host_db::Host::of_host()?;
    let held: Option<String> = db
        .open()?
        .query_row(
            "SELECT note FROM github_identities WHERE host = ?1 AND credential = ?2",
            [host, key],
            |row| row.get(0),
        )
        .optional()?;
    let note: IdentityNote = match held {
        Some(note) => serde_json::from_str(&note).with_context(|| {
            format!("invalid GitHub identity observation of {host} in the host database")
        })?,
        None => IdentityNote::default(),
    };
    if let Some(login) = note.login {
        return Ok(login);
    }
    if note.until > now() {
        bail!(
            "GitHub credential identity waits until {} (Unix seconds): {}",
            note.until,
            note.why.unwrap_or_default()
        )
    }
    let rates = rate_limit_body(gh, host);
    let doc = rates
        .as_ref()
        .ok()
        .and_then(|out| serde_json::from_str::<Value>(out).ok())
        .unwrap_or(Value::Null);
    let blocked = |name: &str| {
        doc["resources"][name]["remaining"] == 0
            && doc["resources"][name]["reset"].as_u64().is_some_and(|t| t > now())
    };
    let gql = [
        "api",
        "graphql",
        "--hostname",
        host,
        "-f",
        "query=query ConvoyQuotaIdentity { viewer { login } }",
        "--jq",
        ".data.viewer.login",
    ];
    let rest = ["api", "user", "--hostname", host, "--jq", ".login"];
    let looked = if let Err(error) = &rates
        && secondary(&error.to_string(), 0).is_some()
    {
        Err(anyhow::anyhow!("{error}"))
    } else if blocked("core") && blocked("graphql") {
        Err(anyhow::anyhow!(
            "both GitHub primary quotas exhausted before credential identity is available: REST core {}; GraphQL {}; next attempt at reset",
            doc["resources"]["core"],
            doc["resources"]["graphql"]
        ))
    } else {
        let (first, other) =
            if blocked("core") { (&gql[..], &rest[..]) } else { (&rest[..], &gql[..]) };
        match gh.raw(first) {
            Err(error) if primary(&error.to_string()) => gh.raw(other).with_context(|| {
                format!(
                    "credential identity primary rejection ({error}); the alternate API also failed"
                )
            }),
            made => made,
        }
    };
    let note = match looked {
        Ok(login) if !login.trim().is_empty() && login.trim().lines().count() == 1 => {
            IdentityNote { login: Some(login.trim().to_string()), until: 0, why: None }
        }
        Ok(_) => bail!(
            "GitHub credential identity returned no login; run gh auth status --hostname {host}"
        ),
        Err(error) => {
            let now = now();
            let cooled = secondary(&error.to_string(), now);
            if cooled.is_none()
                && !primary(&error.to_string())
                && !blocked("core")
                && !blocked("graphql")
            {
                return Err(error.context(
                    "GitHub credential identity unavailable; no alternate login attempted",
                ));
            }
            let reset = ["core", "graphql"]
                .into_iter()
                .filter_map(|a| doc["resources"][a]["reset"].as_u64())
                .filter(|t| *t > now)
                .min()
                .unwrap_or(now.saturating_add(60));
            IdentityNote {
                login: None,
                until: cooled.unwrap_or(reset),
                why: Some(format!("{error:#}")),
            }
        }
    };
    let mut held = db.open()?;
    crate::host_db::write(&mut held, |tx| {
        tx.execute(
            "INSERT OR REPLACE INTO github_identities (host, credential, note) VALUES (?1, ?2, ?3)",
            [host, key, &serde_json::to_string(&note)?],
        )?;
        Ok(())
    })?;
    match note.login {
        Some(login) => Ok(login),
        None => bail!(
            "GitHub credential identity unavailable until {} (Unix seconds): {}",
            note.until,
            note.why.unwrap_or_default()
        ),
    }
}

/// The cache directory's directory of the locks the shared readings are read
/// under, by GitHub host and login.
const LOCKS: &str = "shared-readings";

/// What an older release kept as files of the cache directory, `github-quota/`:
/// moved aside to `github-quota.migrated` once no host runner of an older
/// release holds the host. Its notes are not taken in: each is an expiring
/// observation, whose loss costs one rejected request, never a fact.
fn set_aside_old_notes(cache: &std::path::Path) -> Result<()> {
    let old = cache.join("github-quota");
    if old.exists() && !crate::host_db::another_build_holds(cache) {
        crate::host_db::aside(&old)?;
    }
    Ok(())
}

pub(crate) fn credential_present() -> bool {
    ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejection_saved_last_does_not_shorten_a_known_reset() {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope {
            db: crate::host_db::Host::at(dir.path().join("host.db")),
            locks: dir.path().into(),
            host: "github.com".into(),
            login: "me".into(),
        };
        let known = now() + 3600;
        scope
            .save("graphql", &Observation { remaining: Some(0), reset: known, limit: None })
            .unwrap();
        scope
            .save("graphql", &Observation { remaining: Some(0), reset: now() + 60, limit: None })
            .unwrap();
        assert_eq!(scope.read("graphql").unwrap().reset, known);
        scope
            .save("graphql", &Observation { remaining: Some(0), reset: known + 60, limit: None })
            .unwrap();
        assert_eq!(scope.read("graphql").unwrap().reset, known + 60, "a later reset is taken");
    }

    #[test]
    fn an_older_releases_quota_notes_are_moved_aside_once_no_older_runner_holds_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let old = cache.join("github-quota/github.com/me");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("core.json"), r#"{"remaining":0,"reset":1}"#).unwrap();
        let runner = crate::host_db::tests::runner(&cache, "0.0.1");
        set_aside_old_notes(&cache).unwrap();
        assert!(old.exists(), "an older host runner reads them still");
        drop(runner);
        set_aside_old_notes(&cache).unwrap();
        assert!(!cache.join("github-quota").exists());
        assert!(cache.join("github-quota.migrated/github.com/me/core.json").exists());
        set_aside_old_notes(&cache).unwrap();
    }

    #[test]
    fn a_secondary_limit_met_again_soon_after_doubles_the_hold_up_to_fifteen_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope {
            db: crate::host_db::Host::at(dir.path().join("host.db")),
            locks: dir.path().into(),
            host: "github.com".into(),
            login: "me".into(),
        };
        scope.wrote("gh pr comment", 950).unwrap();
        scope.wrote("gh pr create", 1000).unwrap();
        assert_eq!(scope.trip("gh pr create", 1000, 1060).unwrap(), 1060);
        let hold = scope.hold().unwrap();
        let first = hold.first.clone().unwrap();
        assert_eq!((hold.wait, hold.trips, first.what.as_str()), (60, 1, "gh pr create"));
        assert_eq!(first.before.len(), 2, "{first:?}");
        // in flight before the hold: it stands as it was
        assert_eq!(scope.trip("gh pr merge", 1030, 1090).unwrap(), 1090);
        assert_eq!(scope.hold().unwrap().wait, 60);
        // met again as the hold opens: each next waits twice, GitHub's minimum
        // notwithstanding, up to fifteen minutes
        let mut at = 1090;
        for wait in [120, 240, 480, 900, 900] {
            let reset = scope.trip("gh pr create", at, at + 60).unwrap();
            let hold = scope.hold().unwrap();
            assert_eq!((reset, hold.wait), (at + wait, wait));
            assert_eq!(hold.first.unwrap().at, 1000, "the row keeps its first write");
            at = reset + 5;
        }
        // GitHub's longer word is waited in full
        assert_eq!(scope.trip("gh pr create", at, at + 2000).unwrap(), at + 2000);
        // quiet for the next hold's wait: the row ends, and GitHub's wait is taken again
        let quiet = scope.hold().unwrap().seen.reset + LONGEST_HOLD;
        assert_eq!(scope.trip("gh pr comment", quiet, quiet + 60).unwrap(), quiet + 60);
        let hold = scope.hold().unwrap();
        assert_eq!((hold.trips, hold.first.unwrap().what.as_str()), (1, "gh pr comment"));
    }

    #[test]
    fn writes_are_named_by_kind_and_reads_are_none() {
        use crate::gh::writes;
        assert_eq!(writes(&["pr", "create", "--base", "main"]).unwrap(), "gh pr create");
        assert_eq!(
            writes(&["api", "--method", "POST", "repos/o/r/issues/7/comments", "-f", "body=x"])
                .unwrap(),
            "api POST repos/o/r/issues/:n/comments"
        );
        assert_eq!(
            writes(&["api", "repos/o/r/issues/7/labels", "-f", "labels[]=x"]).unwrap(),
            "api POST repos/o/r/issues/:n/labels"
        );
        assert_eq!(
            writes(&["api", "graphql", "-f", "query=mutation($r:ID!){x}", "-f", "r=1"]).unwrap(),
            "api graphql mutation"
        );
        for read in [
            &["api", "graphql", "-f", "query=query { viewer { login } }"][..],
            &["api", "rate_limit", "--hostname", "github.com"],
            &["api", "--paginate", "repos/o/r/pulls?state=all", "--jq", ".x"],
            &["pr", "list", "--json", "number"],
            &["pr", "view", "3"],
        ] {
            assert_eq!(writes(read), None, "{read:?}");
        }
    }

    #[test]
    fn a_retry_after_date_is_that_instant_whatever_the_clock_reads() {
        let said = "gh: secondary rate limit\nRetry-After: Fri, 01 Jan 2038 00:00:00 GMT";
        let date = crate::doctor::watching::since("2038-01-01T00:00:00Z").unwrap();
        for now in [0, 1000, date - 1, date, date + 1] {
            assert_eq!(secondary(said, now), Some(date), "now={now}");
        }
        let said = "gh: secondary rate limit\nRetry-After: 30";
        assert_eq!(secondary(said, 1000), Some(1030));
        assert_eq!(secondary("gh: secondary rate limit", 1000), Some(1060));
    }
}

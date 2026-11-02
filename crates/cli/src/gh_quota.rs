//! Expiring observations of GitHub quotas, never fleet state. Each login on
//! each host has separate core/GraphQL observations and a shared secondary
//! cooldown. Reads of this note precede requests, including in a later tick or
//! another CLI process. Losing a note costs a rejected request, never a fact.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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

#[derive(Clone, Debug)]
pub(crate) struct Scope {
    dir: PathBuf,
    pub(crate) host: String,
    pub(crate) login: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Observation {
    pub(crate) remaining: Option<u64>,
    pub(crate) reset: u64,
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
            let host = Command::new("git")
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
        let dir =
            crate::copies::cache()?.join("github-quota").join(segment(host)).join(segment(&login));
        Ok(Self { dir, host: host.to_string(), login })
    }

    pub(crate) fn read(&self, name: &str) -> Result<Observation> {
        let file = self.dir.join(format!("{name}.json"));
        match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "unreadable GitHub quota observation {}; remove it and retry",
                    file.display()
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Observation::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn save(&self, name: &str, value: &Observation) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let lock = std::fs::File::create(self.dir.join("observations.lock"))?;
        lock.lock()?;
        let prior = self.read(name)?;
        // Responses from concurrent requests may arrive out of order. A late
        // positive observation cannot erase a primary rejection before reset.
        if prior.remaining == Some(0) && prior.reset > now() && value.remaining != Some(0) {
            return Ok(());
        }
        let mut value = value.clone();
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
        if name == "secondary" {
            value.reset = value.reset.max(prior.reset);
        }
        let file = self.dir.join(format!("{name}.json"));
        let temp = file.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(&temp, serde_json::to_vec(&value)?)?;
        std::fs::rename(temp, file)?;
        Ok(())
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
        NOTE.read(&self.dir.to_string_lossy(), || first = true);
        if !first {
            return Ok(());
        }
        let out = match rate_limit_body(gh, &self.host) {
            Ok(out) => out,
            Err(error) => {
                if let Some(reset) = secondary(&error.to_string(), now()) {
                    self.save("secondary", &Observation { remaining: Some(0), reset })?;
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
                    self.save(api.name(), &Observation { remaining: Some(remaining), reset })?;
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
                &Observation { remaining: Some(remaining.parse()?), reset: reset.parse()? },
            )?;
        }
        Ok(())
    }

    pub(crate) fn json(&self) -> Result<Value> {
        let core = self.read("core")?;
        let graphql = self.read("graphql")?;
        let secondary = self.read("secondary")?;
        let now = now();
        Ok(
            serde_json::json!({"host":self.host,"login":self.login,"core":core,"graphql":graphql,"secondary":secondary,
            "for_rest_reads":if secondary.reset>now {"wait"}else if core.remaining==Some(0)&&core.reset>now {"graphql when equivalent"}else{"core"},
            "for_graphql_reads":if secondary.reset>now {"wait"}else if graphql.remaining==Some(0)&&graphql.reset>now {"core when equivalent"}else{"graphql"}}),
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
    let mut child = Command::new("git")
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
    let dir = crate::copies::cache()?.join("github-quota").join(segment(host)).join("identities");
    let file = dir.join(format!("{key}.json"));
    let note: IdentityNote = match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
            format!("invalid GitHub identity observation {}; remove it and retry", file.display())
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => IdentityNote::default(),
        Err(e) => return Err(e.into()),
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
    std::fs::create_dir_all(&dir)?;
    let tmp = file.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&tmp, serde_json::to_vec(&note)?)?;
    std::fs::rename(tmp, &file)?;
    match note.login {
        Some(login) => Ok(login),
        None => bail!(
            "GitHub credential identity unavailable until {} (Unix seconds): {}",
            note.until,
            note.why.unwrap_or_default()
        ),
    }
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
        let scope = Scope { dir: dir.path().into(), host: "github.com".into(), login: "me".into() };
        let known = now() + 3600;
        scope.save("graphql", &Observation { remaining: Some(0), reset: known }).unwrap();
        scope.save("graphql", &Observation { remaining: Some(0), reset: now() + 60 }).unwrap();
        assert_eq!(scope.read("graphql").unwrap().reset, known);
        scope.save("graphql", &Observation { remaining: Some(0), reset: known + 60 }).unwrap();
        assert_eq!(scope.read("graphql").unwrap().reset, known + 60, "a later reset is taken");
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

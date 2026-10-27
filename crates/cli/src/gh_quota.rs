//! Expiring observations of GitHub quotas, never fleet state. Each login on
//! each host has separate core/GraphQL observations and a shared secondary
//! cooldown. Reads of this note precede requests, including in a later tick or
//! another CLI process. Losing a note costs a rejected request, never a fact.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::gh::GitHub;

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

impl Scope {
    pub(crate) fn of(gh: &GitHub) -> Result<Self> {
        let host = std::env::var("GH_HOST")
            .ok()
            .or_else(|| {
                let out = Command::new("git")
                    .current_dir(&gh.dir)
                    .args(["remote", "get-url", "origin"])
                    .output()
                    .ok()?;
                let remote = String::from_utf8(out.stdout).ok()?;
                let remote = remote.trim();
                let host = if let Some((_, rest)) = remote.split_once("://") {
                    rest.split('/').next()?.rsplit('@').next()?.split(':').next()?
                } else if let Some((_, rest)) = remote.split_once('@') {
                    rest.split(':').next()?
                } else {
                    return None;
                };
                Some(host.to_string())
            })
            .unwrap_or_else(|| "github.com".into())
            .to_ascii_lowercase();
        // Never request --show-token, nor persist any credential or stderr.
        let out = gh.raw(&[
            "auth",
            "status",
            "--active",
            "--hostname",
            &host,
            "--json",
            "hosts",
            "--jq",
            ".hosts[][] | select(.active) | .login",
        ])?;
        let login = out.trim().to_string();
        if login.is_empty() || login.lines().count() != 1 {
            bail!(
                "GitHub quota identity unavailable for {host}; run gh auth status --hostname {host}"
            );
        }
        let dir =
            crate::copies::cache()?.join("github-quota").join(segment(&host)).join(segment(&login));
        Ok(Self { dir, host, login })
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
    pub(crate) fn observe(&self, gh: &GitHub) -> Result<()> {
        let out = match gh.raw(&["api", "rate_limit", "--hostname", &self.host]) {
            Ok(out) => out,
            Err(error) => {
                if let Some(delay) = secondary(&error.to_string()) {
                    let reset = now().saturating_add(delay);
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
    secondary(said).is_none()
        && (s.contains("api rate limit exceeded")
            || s.contains("api rate limit already exceeded")
            || s.contains("\"type\":\"rate_limited\""))
}

/// Retry-After is a host/login-wide cooldown, even when the response also
/// mentions the primary quota. Missing duration uses GitHub's one-minute floor.
pub(crate) fn secondary(said: &str) -> Option<u64> {
    let s = said.to_ascii_lowercase();
    if !(s.contains("secondary rate") || s.contains("abuse") || s.contains("retry-after:")) {
        return None;
    }
    let delay = s
        .lines()
        .find_map(|line| {
            let after = line.trim().strip_prefix("retry-after:")?.trim();
            after.parse().ok().or_else(|| retry_date(after).map(|t| t.saturating_sub(now())))
        })
        .unwrap_or(60);
    Some(delay)
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

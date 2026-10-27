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
            ["api", "graphql", ..] | ["pr" | "issue", "list" | "create" | "close" | "edit", ..] => {
                Some(Self::Graphql)
            }
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
        let file = self.dir.join(format!("{name}.json"));
        let temp = file.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(&temp, serde_json::to_vec(value)?)?;
        std::fs::rename(temp, file)?;
        Ok(())
    }

    pub(crate) fn describe(&self) -> Result<String> {
        let core = self.read("core")?;
        let gql = self.read("graphql")?;
        Ok(format!(
            "{}/{}: REST core remaining={:?} reset={}; GraphQL remaining={:?} reset={}",
            self.host, self.login, core.remaining, core.reset, gql.remaining, gql.reset
        ))
    }

    /// The rate-limit endpoint has its own exemption from primary quotas.
    /// An unavailable observation never clears an exhaustion already seen.
    pub(crate) fn observe(&self, gh: &GitHub) -> Result<()> {
        if let Ok(out) = gh.raw(&["api", "rate_limit", "--hostname", &self.host])
            && let Ok(doc) = serde_json::from_str::<Value>(&out)
        {
            for api in [Api::Core, Api::Graphql] {
                let row = &doc["resources"][api.name()];
                if let (Some(remaining), Some(reset)) =
                    (row["remaining"].as_u64(), row["reset"].as_u64())
                {
                    self.save(api.name(), &Observation { remaining: Some(remaining), reset })?;
                }
            }
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
            || s.contains("x-ratelimit-remaining: 0")
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
        .find_map(|line| line.trim().strip_prefix("retry-after:")?.trim().parse().ok())
        .unwrap_or(60);
    Some(delay)
}

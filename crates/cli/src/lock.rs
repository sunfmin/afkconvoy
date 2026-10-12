//! Raising a fleet lock (docs/adr/0011), and the read of the locks standing
//! that a join asks before it puts a PR on the landing train. What a lock is
//! and what it keeps the fleet off is the core's (`afkconvoy_core::lock`).

use afkconvoy_core::config::Config;
use afkconvoy_core::lock::{raised_comment, standing};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::Repo;
use crate::wake::wake;

/// The fleet locks standing on GitHub now, lowest number first.
pub fn standing_locks(gh: &GitHub, config: &Config) -> Result<Vec<u64>> {
    Ok(standing(&gh.open_issues()?, config))
}

/// `afkconvoy lock --issue <n>`: raise a fleet lock on an open issue — leave
/// the record of the `role` that did, put the lock label on it, and wake the
/// fleet. An issue that is a lock already gets no second record.
///
///   {"issue", "label", "role", "raised": true, "locks", "woke"}
///   {…, "raised": false, …}
///
/// The record goes first: a lock nobody is on record for never stands.
pub fn lock(
    repo: &Repo,
    config: &Config,
    gh: &GitHub,
    issue: u64,
    role: Option<&str>,
) -> Result<Value> {
    let read = gh.issue(issue)?;
    if read.is_closed() {
        bail!("issue #{issue} is closed: a fleet lock is an open issue, and ends when it closes");
    }
    let label = &config.lock_label;
    let raised = !read.labels.contains(label);
    if raised {
        gh.comment(issue, &raised_comment(role))?;
        gh.add_label(issue, label)?;
    }
    Ok(json!({"issue": issue, "label": label, "role": role, "raised": raised,
        "locks": standing_locks(gh, config)?, "woke": wake(repo)?["woke"]}))
}

//! Raising a fleet lock (docs/adr/0011), and the read of the locks standing
//! that a join asks before it puts a PR on the landing train. What a lock is
//! and what it keeps the fleet off is the core's (`afkconvoy_core::lock`).

use afkconvoy_core::config::Config;
use afkconvoy_core::lock::{Closed, explains, raised_comment, standing};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::gh::GitHub;

/// The fleet locks standing on GitHub now, lowest number first.
pub fn standing_locks(gh: &GitHub, config: &Config) -> Result<Vec<u64>> {
    Ok(standing(&gh.open_issues()?.rows, config))
}

/// The closed lock that explains an `escalate` or a failure declared at `at`
/// (`lock::explains`), from the locks GitHub says have closed. `None` of one
/// declared at no known time: nothing is run again on a guess.
pub fn explaining_lock(gh: &GitHub, config: &Config, at: Option<u64>) -> Result<Option<u64>> {
    let Some(at) = at else { return Ok(None) };
    let closed = gh.closed_with(&config.lock_label)?;
    let closed: Vec<Closed> =
        closed.into_iter().map(|(number, at)| Closed { number, at }).collect();
    Ok(explains(&closed, at))
}

/// `afkconvoy lock --issue <n>`: raise a fleet lock on an open issue — leave
/// the record of the `role` that did, and put the lock label on it; the fleet
/// is woken as the command ends (`crate::acting`). An issue that is a lock
/// already gets no second record.
///
///   {"issue", "label", "role", "raised": true, "locks"}
///   {…, "raised": false, …}
///
/// The record goes first: a lock nobody is on record for never stands.
pub fn lock(config: &Config, gh: &GitHub, issue: u64, role: Option<&str>) -> Result<Value> {
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
        "locks": standing_locks(gh, config)?}))
}

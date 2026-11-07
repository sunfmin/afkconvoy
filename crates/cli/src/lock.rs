//! Raising a fleet lock (docs/adr/0011), and the read of the locks standing
//! that a join asks before it puts a PR on the base branch. What a lock is
//! and what it keeps the fleet off is the core's (`afkconvoy_core::lock`).

use afkconvoy_core::config::Config;
use afkconvoy_core::lock::{Closed, explains, raised_comment};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::gh::GitHub;

/// The fleet locks standing on GitHub now, lowest number first.
pub fn standing_locks(gh: &GitHub, config: &Config) -> Result<Vec<u64>> {
    gh.open_with(&config.lock_label)
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
/// the record of the `role` that did, and put the lock label on it, with the
/// repo label before it where the tracker is another repo and the issue
/// lacks it: a lock there holds the fleet of the repo whose issue it is; the
/// fleet is woken as the command ends (`crate::acting`). An issue that is a
/// lock already gets no second record; one of another repo is refused.
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
    if let Err(why) = gh.tracker.to_take(&read.labels) {
        bail!("issue {} {why}: a lock holds the fleet whose issue it is", gh.tracker.issue(issue));
    }
    let label = &config.lock_label;
    let raised = !read.labels.contains(label);
    if raised {
        gh.comment(issue, &raised_comment(role))?;
    }
    gh.take(issue, &read.labels)?;
    if raised {
        gh.add_label(issue, label)?;
    }
    Ok(json!({"issue": issue, "label": label, "role": role, "raised": raised,
        "locks": standing_locks(gh, config)?}))
}

/// What `afkconvoy lock --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When the fault is one every issue shares — the base branch is broken, so every gate is red whatever its PR holds — and one issue carries the fix. Raise the lock on that issue:

```
afkconvoy lock --issue 7
```

From the next tick the fleet works lock issues only, until the last of them closes. For anything that is wrong with one issue alone, declare a verdict instead.

## Who runs it

A worker, in its own worktree. In a bound repo, a patrol or a stage that finds the base branch red. In an unbound repo, a worker or a watch that proves the base branch red the same way gated alone in a clean checkout: no role is needed. The repair worker does the same in either repo. Each finds the open issue for the fix or opens one, makes it dispatchable — the ready label in an unbound repo, intake to a workflow in a bound one — then runs this command on it from its worktree. The operator may run it too, in a checkout of the repo; a human may as well put the lock label on the issue on GitHub.

## What it reads and writes

It reads the issue and the config's `lock_label` (default fleet-lock). On an open issue that does not carry the label it writes one comment — the record of the role that raised the lock, read from `--role` or from the worker's environment (AFKCONVOY_ROLE); a worker of an unbound repo has none — and then puts the label on the issue. Then it wakes the fleet, as `afkconvoy wake` does.

The issue is the tracker's: the checkout's own repo, or the one its host says (`afkconvoy run --tracker`). There a lock holds the fleets of the repos whose repo label it carries, `repo:<owner>/<repo>`, and no other repo's fleet that works the same tracker: before the lock label it puts this repo's repo label on an issue that lacks it.

It claims nothing and starts nothing. The lock issue is worked as any issue is: in a bound repo once it carries a flow label, in an unbound one once it carries the ready label. Nothing here takes a lock off: closing the issue ends it — landing the PR that closes it does.

## What it answers

{"issue", "label", "role", "raised", "locks", "woke"}

- `label`: the lock label the issue now carries
- `role`: the role on record as having raised it, or null
- raised: true: the record was written and the label put on
- raised: false: the issue was a lock already: nothing was written
- `locks`: every fleet lock standing now, lowest number first
- `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers

Errors, as {"error": …} with a non-zero exit: the issue is closed; there is no such issue; the issue carries the repo label of another repo — or, in a repo that is its own tracker, any repo label; the config cannot be read; "is not in a git checkout", or git's own words for a checkout it refuses.

## What trips you up

- A lock stops everything else, on every host: no other issue is dispatched, no stale claim of another issue is taken, no issue is handed to a human, and no PR of another issue lands in a turn (`afkconvoy land` answers `locked`).
- Workers already running are left alone, and nobody asks after them: what they declare — a verdict, a PR — is acted on once no lock stands. Their claims take no slot meanwhile, so a lock issue is started even by a fleet at its bound.
- Once the last lock has closed, a bound repo's stage that declared `escalate` or stayed silent past its nudge before then runs again, on the base branch as it is now: a broken base branch is what such a lock is raised for, and a human could only answer "run the gate again". Every other verdict stands.
- What was on the base branch before the lock is not taken off it. The lock issue's PR lands after it.
- A lock issue nothing makes dispatchable — no ready label, no flow label, an epic label, an open blocker — is worked by nobody, and the lock stands all the same. `afkconvoy plan` shows the lock under `locks` and the issue under `excluded`, with why.
- It does not check that the base branch is broken. It is refused to nobody: the guard recognises the label being moved by hand, not this command."#;

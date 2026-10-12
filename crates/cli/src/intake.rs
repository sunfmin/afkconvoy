//! Intake by a worker (docs/adr/0011): `afkconvoy intake`, which puts a flow
//! label on an issue on the record, under the repo's `intake_limit`; and the
//! count the limit is held against, read off the intake records of the open
//! issues. What a record is and which issue it counts for is the core's
//! (`afkconvoy_core::intake`).

use afkconvoy_core::intake::{IntakeRecord, has_room, taken_in};
use afkconvoy_core::plan::Issue;
use afkconvoy_core::role::flows;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::config;
use crate::copies::Copies;
use crate::gh::{Comment, GitHub};
use crate::git::Repo;
use crate::team::keep;
use crate::wake::wake;

/// What a worker takes in, and who it is.
pub struct Taking {
    pub issue: u64,
    /// The workflow to take it in to, by name.
    pub workflow: String,
    /// The worker's role, and the issue it is on.
    pub role: String,
    pub from: u64,
}

/// How an issue carrying `labels` came to follow its workflow, read off its
/// `comments`: by a worker's intake, with the record it left, or — a flow
/// label no record is for — by a human's.
pub(crate) fn came_in(labels: &[String], comments: &[Comment]) -> Value {
    match taken_in(labels, comments.iter().map(|comment| comment.body.as_str())) {
        Some((at, record)) => json!({"by": "worker", "role": record.role, "from": record.from,
            "url": comments.get(at).map(|comment| comment.url.as_str())}),
        None => json!({"by": "human"}),
    }
}

/// How many of `issues` — the open ones — a worker took in, counted from
/// their intake records.
fn open_taken_in(gh: &GitHub, issues: &[Issue]) -> Result<usize> {
    let mut open = 0;
    for issue in issues.iter().filter(|issue| !flows(&issue.labels).is_empty()) {
        let comments = gh.comments(issue.number)?;
        if taken_in(&issue.labels, comments.iter().map(|comment| comment.body.as_str())).is_some() {
            open += 1;
        }
    }
    Ok(open)
}

/// `afkconvoy intake`: take the issue `taking` names in to its workflow — the
/// intake record, then the flow label — and wake the fleet. An issue that
/// follows that workflow already is left as it is, whoever took it in.
///
///   {"issue", "workflow", "taken": true, "role", "from", "open", "limit", "woke"}
///   {"issue", "workflow", "taken": false, "standing": <how it came in>}
///
/// `open` is the open issues taken in by workers, this one among them.
pub fn intake(repo: &Repo, taking: Taking) -> Result<Value> {
    let Taking { issue: number, workflow: name, role, from } = taking;
    let config = config::load(&repo.dir.join(config::FILE))?;
    let Some(bound) = config.bound else {
        bail!("this repo is unbound: its config names no team, so it has no workflow to take in to")
    };
    let gh = GitHub { dir: repo.dir.clone() };
    let kept = keep(&gh, &Copies::here()?, &bound.team)?;
    let Some(workflow) = kept.team.workflow(&name) else {
        let has: Vec<&str> = kept.team.workflows.iter().map(|w| w.name.as_str()).collect();
        bail!("the team {} has no workflow {name:?}: it has {}", bound.team, has.join(", "));
    };
    if kept.team.role(&role).is_none() {
        bail!(
            "the team {} has no role {role:?}: an intake names the role that made it",
            bound.team
        );
    }
    let issue = gh.issue(number)?;
    if issue.pull_request {
        bail!("#{number} is a pull request: only an issue is taken in");
    }
    if issue.is_closed() {
        bail!("issue #{number} is closed: only an open issue is taken in");
    }
    match flows(&issue.labels).as_slice() {
        [] => {}
        [carried] if *carried == name => {
            let standing = came_in(&issue.labels, &gh.comments(number)?);
            return Ok(json!({"issue": number, "workflow": name, "taken": false,
                "standing": standing}));
        }
        carried => bail!(
            "issue #{number} already follows a workflow ({}): an issue follows one workflow",
            carried.join(", ")
        ),
    }
    let (open, limit) = (open_taken_in(&gh, &gh.open_issues()?)?, bound.intake_limit);
    if !has_room(open, limit) {
        bail!(
            "the intake limit is reached: {open} open issues were taken in by workers, and \
             `intake_limit` is {limit}. One has to close before a worker takes another in"
        );
    }
    let record = IntakeRecord { workflow: name, role, from };
    gh.comment(number, &record.comment())?;
    gh.add_label(number, &workflow.flow_label())?;
    Ok(json!({"issue": number, "workflow": record.workflow, "taken": true,
        "role": record.role, "from": record.from, "open": open + 1, "limit": limit,
        "woke": wake(repo)?["woke"]}))
}

//! Intake by a worker (docs/adr/0011): `afkconvoy intake`, which puts a flow
//! label on an issue on the record, under the repo's `intake_limit`; and the
//! count the limit is held against, read off the intake records of the open
//! issues. What a record is and which issue it counts for is the core's
//! (`afkconvoy_core::intake`).

use afkconvoy_core::intake::{By, IntakeRecord, On, has_room, taken_in};
use afkconvoy_core::plan::Issue;
use afkconvoy_core::team::Team;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config;
use crate::copies::Copies;
use crate::gh::{Comment, GitHub, OpenIssues};
use crate::git::Repo;
use crate::team::keep;

/// What a worker takes in, and who it is.
pub struct Taking {
    pub issue: u64,
    /// The workflow to take it in to, by name.
    pub workflow: String,
    /// The worker: one in its role, on an issue or a patrol, or the train
    /// worker.
    pub by: By,
    /// The issues it is to wait for: what a stage that splits its issue says
    /// of the order of the children.
    pub blocked_by: Vec<u64>,
}

/// The worker that took an issue in, as an answer says it: its `role`, with
/// `from`, the issue it was on, or `patrol`, the patrol — or `train`.
fn who(by: &By) -> Value {
    match by {
        By::Role { role, on: On::Issue(issue) } => json!({"role": role, "from": issue}),
        By::Role { role, on: On::Patrol(patrol) } => json!({"role": role, "patrol": patrol}),
        By::Train => json!({"train": true}),
    }
}

/// `object` with the fields of `more` added.
fn with(mut object: Value, more: Value) -> Value {
    if let (Some(object), Value::Object(more)) = (object.as_object_mut(), more) {
        object.extend(more);
    }
    object
}

/// How an issue carrying `labels` came to follow its workflow of `team`,
/// read off its `comments`: by a worker's intake, with the record it left,
/// or — a flow label no record is for — by a human's.
pub(crate) fn came_in(team: &Team, labels: &[String], comments: &[Comment]) -> Value {
    match taken_in(team, labels, comments.iter().map(|comment| comment.body.as_str())) {
        Some((at, record)) => with(
            json!({"by": "worker", "url": comments.get(at).map(|comment| comment.url.as_str())}),
            who(&record.by),
        ),
        None => json!({"by": "human"}),
    }
}

/// How many of `issues` — the open ones — a worker took in to a workflow of
/// `team`, counted from their intake records.
pub(crate) fn open_taken_in(team: &Team, open: &OpenIssues) -> usize {
    let taken = |issue: &&Issue| {
        let bodies = open.comments(issue.number).iter().map(|comment| comment.body.as_str());
        taken_in(team, &issue.labels, bodies).is_some()
    };
    open.rows.iter().filter(taken).count()
}

/// The database id of each issue `blocked_by` names, as a blocker of issue
/// `number`: an edge is written by id. Refused: the issue itself, a pull
/// request, and one that cannot be read.
fn blockers(gh: &GitHub, number: u64, blocked_by: &[u64]) -> Result<Vec<(u64, u64)>> {
    let mut found = vec![];
    for &blocker in blocked_by {
        if blocker == number {
            bail!("issue #{number} cannot wait for itself: --blocked-by names it");
        }
        let read = gh.issue(blocker).map_err(|error| {
            anyhow!("--blocked-by names #{blocker}, which cannot be read: {error:#}")
        })?;
        if read.pull_request {
            bail!("--blocked-by names #{blocker}, a pull request: an issue waits for issues");
        }
        found.push((blocker, read.id));
    }
    Ok(found)
}

/// `afkconvoy intake`: take the issue `taking` names in to its workflow — a
/// blocked-by edge to each issue it is to wait for, the intake record, then,
/// in a tracker that is another repo, the repo label where the issue lacks
/// it, and the flow label; the fleet is woken as the command ends
/// (`crate::acting`). The edges come first: the flow label is what puts the
/// issue on the frontier, and it is never there ahead of what blocks it. An
/// issue that follows that workflow already is left as it is, whoever took
/// it in, but for the repo label. One that is another repo's is refused.
///
///   {"issue", "workflow", "taken": true, "role", "from" | "patrol" — or "train",
///    "blocked_by", "open", "limit", "woke"}
///   {"issue", "workflow", "taken": false, "standing": <how it came in>}
///
/// `open` is the open issues taken in by workers, this one among them.
pub fn intake(repo: &Repo, taking: Taking) -> Result<Value> {
    let Taking { issue: number, workflow: name, by, blocked_by } = taking;
    let config = config::load(&repo.dir.join(config::FILE))?;
    let labels = config.labels();
    let Some(bound) = config.bound else {
        bail!("this repo is unbound: its config names no team, so it has no workflow to take in to")
    };
    let gh = GitHub::of(repo)?;
    let kept = keep(&gh, &Copies::of_host()?, &bound.team, &labels)?;
    let Some(workflow) = kept.team.workflow(&name) else {
        let has: Vec<&str> = kept.team.workflows.iter().map(|w| w.name.as_str()).collect();
        bail!("the team {} has no workflow {name:?}: it has {}", bound.team, has.join(", "));
    };
    if let By::Role { role, .. } = &by
        && kept.team.role(role).is_none()
    {
        bail!(
            "the team {} has no role {role:?}: an intake names the role that made it",
            bound.team
        );
    }
    if let By::Role { on: On::Patrol(patrol), .. } = &by
        && kept.team.patrol(patrol).is_none()
    {
        bail!(
            "the team {} has no patrol {patrol:?}: an intake names the patrol that made it",
            bound.team
        );
    }
    let issue = gh.issue(number).map_err(|error| missing_issue(repo, &gh, number, error))?;
    if issue.pull_request {
        bail!("#{number} is a pull request: only an issue is taken in");
    }
    if issue.is_closed() {
        bail!("issue #{number} is closed: only an open issue is taken in");
    }
    if let Err(why) = gh.tracker.to_take(&issue.labels) {
        bail!("issue {} {why}", gh.tracker.issue(number));
    }
    match kept.team.flows(&issue.labels).as_slice() {
        [] => {}
        [carried] if *carried == workflow.flow_label() => {
            gh.take(number, &issue.labels)?;
            let standing = came_in(&kept.team, &issue.labels, &gh.comments(number)?);
            return Ok(json!({"issue": number, "workflow": name, "taken": false,
                "standing": standing}));
        }
        carried => bail!(
            "issue #{number} already follows a workflow ({}): an issue follows one workflow",
            carried.join(", ")
        ),
    }
    let blockers = blockers(&gh, number, &blocked_by)?;
    let (open, limit) = (open_taken_in(&kept.team, &gh.open_issues()?), bound.intake_limit);
    if !has_room(open, limit) {
        bail!(
            "the intake limit is reached: {open} open issues were taken in by workers, and \
             `intake_limit` is {limit}. One has to close before a worker takes another in"
        );
    }
    let recorded = gh.blocked_by(number)?;
    for (blocker, id) in blockers {
        if !recorded.iter().any(|edge| edge.number == blocker) {
            gh.add_blocker(number, id)?;
        }
    }
    let record = IntakeRecord { workflow: name, by };
    gh.comment(number, &record.comment())?;
    gh.take(number, &issue.labels)?;
    gh.add_label(number, workflow.flow_label())?;
    let taken = json!({"issue": number, "workflow": record.workflow, "taken": true,
        "blocked_by": blocked_by, "open": open + 1, "limit": limit});
    Ok(with(taken, who(&record.by)))
}

/// A number in the checkout's repo is often mistaken for one in its tracker.
/// Ask `gh` for its URL (following transfers); a failed lookup is not a claim
/// that the issue is anywhere in particular.
fn missing_issue(repo: &Repo, gh: &GitHub, number: u64, error: anyhow::Error) -> anyhow::Error {
    if !crate::gh::gone(&error).is_some_and(|gone| !gone.deleted) {
        return error;
    }
    let own = crate::checkout_list::repo_of(repo).unwrap_or_else(|_| "<its repo>".to_string());
    let tracker = gh.tracker.named().map_or(own.as_str(), |name| name.as_str());
    let found = gh
        .read(&["issue", "view", &number.to_string(), "--repo", &own, "--json", "url"])
        .ok()
        .and_then(|out| serde_json::from_str::<Value>(&out).ok())
        .and_then(|out| out["url"].as_str().map(str::to_string));
    let location = found.as_deref().and_then(|url| {
        let parts: Vec<_> = url.trim_end_matches('/').rsplit('/').take(4).collect();
        match parts.as_slice() {
            [number, "issues", repo, owner] => {
                Some((format!("{owner}/{repo}"), number.to_string()))
            }
            _ => None,
        }
    });
    let (source, source_number) =
        location.clone().unwrap_or_else(|| ("<its repo>".to_string(), number.to_string()));
    let transfer = afkconvoy_core::line::Line::new()
        .fixed("gh issue transfer ")
        .number(source_number.parse().unwrap_or(number))
        .fixed(" ")
        .word(tracker)
        .fixed(" --repo ")
        .word(&source)
        .own();
    let where_found = location.map_or_else(
        || "gh could not locate it in the checkout's repo".to_string(),
        |(repo, n)| format!("gh found it in {repo} as #{n}"),
    );
    anyhow!(
        "issue #{number} was not found in tracker {tracker}; {where_found}. Transfer it: {transfer}; then run afkconvoy flows in the target repo's checkout and take the transferred issue in there"
    )
}

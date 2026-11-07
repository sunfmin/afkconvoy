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
    pub workflow: Option<String>,
    /// The worker: one in its role, on an issue or a patrol, or the train
    /// worker.
    pub by: Option<By>,
    /// Explicit delivery wins over inheritance from the worker's source task.
    pub delivery: Option<String>,
    pub from: Option<u64>,
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
        By::Repair => json!({"repair": true}),
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
    let Taking { issue: number, workflow: name, by, blocked_by, delivery, from } = taking;
    let config = config::load(&repo.dir.join(config::FILE))?;
    let gh = GitHub::of(repo)?;
    // Only this supported operation's source task supplies inheritance. Edges,
    // parent links, checkout branches and ambient discussion supply none.
    let delivery = match delivery {
        Some(id) => Some(id),
        None => from.map(|source| crate::target::delivery(repo, source)).transpose()?.flatten(),
    };
    if let Some(id) = &delivery {
        crate::delivery::reference(repo, id)?;
    }
    let labels = config.labels();
    let Some(bound) = config.bound else {
        if name.is_some() {
            bail!(
                "this repo is unbound: its config names no team, so it has no workflow to take in to"
            )
        }
        let read = gh.issue(number)?;
        anyhow::ensure!(
            !read.pull_request && !read.is_closed(),
            "intake requires an open task issue"
        );
        gh.tracker.to_take(&read.labels).map_err(anyhow::Error::msg)?;
        let blockers = blockers(&gh, number, &blocked_by)?;
        if let Some(id) = &delivery {
            crate::target::attach(repo, number, id)?;
        }
        let recorded = gh.blocked_by(number)?;
        for (blocker, id) in blockers {
            if !recorded.iter().any(|edge| edge.number == blocker) {
                gh.add_blocker(number, id)?;
            }
        }
        gh.take(number, &read.labels)?;
        crate::flows::create_missing(
            &gh,
            std::slice::from_ref(&config.ready_label),
            crate::flows::READY_LABEL_SAYS,
        )?;
        gh.add_label(number, &config.ready_label)?;
        return Ok(with(
            json!({"issue": number, "taken": true, "blocked_by": blocked_by}),
            crate::target::select(repo, number, None)?,
        ));
    };
    let name = name.ok_or_else(|| {
        anyhow!("a bound repo requires --workflow <name>; choose with afkconvoy flows")
    })?;
    if by.is_none() && delivery.is_none() {
        bail!(
            "an intake names the role of its worker (--role and --from, or their worker environment) — or an explicit --delivery for discussion intake"
        );
    }
    let kept = keep(&gh, &Copies::of_host()?, &bound.team, &labels)?;
    let Some(workflow) = kept.team.workflow(&name) else {
        let has: Vec<&str> = kept.team.workflows.iter().map(|w| w.name.as_str()).collect();
        bail!("the team {} has no workflow {name:?}: it has {}", bound.team, has.join(", "));
    };
    if let Some(By::Role { role, .. }) = &by
        && kept.team.role(role).is_none()
    {
        bail!(
            "the team {} has no role {role:?}: an intake names the role that made it",
            bound.team
        );
    }
    if let Some(By::Role { on: On::Patrol(patrol), .. }) = &by
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
            if let Some(id) = &delivery {
                crate::target::attach(repo, number, id)?;
            }
            gh.take(number, &issue.labels)?;
            let standing = came_in(&kept.team, &issue.labels, &gh.comments(number)?);
            let answer =
                json!({"issue": number, "workflow": name, "taken": false, "standing": standing});
            return if delivery.is_some() {
                Ok(with(answer, crate::target::select(repo, number, None)?))
            } else {
                Ok(answer)
            };
        }
        carried => bail!(
            "issue #{number} already follows a workflow ({}): an issue follows one workflow",
            carried.join(", ")
        ),
    }
    let blockers = blockers(&gh, number, &blocked_by)?;
    let (open, limit) = (open_taken_in(&kept.team, &gh.open_issues()?), bound.intake_limit);
    if by.is_some() && !has_room(open, limit) {
        bail!(
            "the intake limit is reached: {open} open issues were taken in by workers, and \
             `intake_limit` is {limit}. One has to close before a worker takes another in"
        );
    }
    let recorded = gh.blocked_by(number)?;
    if let Some(id) = &delivery {
        crate::target::attach(repo, number, id)?;
    }
    for (blocker, id) in blockers {
        if !recorded.iter().any(|edge| edge.number == blocker) {
            gh.add_blocker(number, id)?;
        }
    }
    let record = by.map(|by| IntakeRecord { workflow: name.clone(), by });
    if let Some(record) = &record {
        gh.comment(number, &record.comment())?;
    }
    gh.take(number, &issue.labels)?;
    gh.add_label(number, workflow.flow_label())?;
    let mut taken = json!({"issue": number, "workflow": name, "taken": true,
        "blocked_by": blocked_by});
    if let Some(record) = record {
        taken = with(taken, with(json!({"open": open + 1, "limit": limit}), who(&record.by)));
    }
    if delivery.is_some() {
        taken = with(taken, crate::target::select(repo, number, None)?);
    }
    Ok(taken)
}

/// A number in the checkout's repo is often mistaken for one in its tracker.
/// Ask `gh` for its URL (following transfers); a failed lookup is not a claim
/// that the issue is anywhere in particular.
fn missing_issue(repo: &Repo, gh: &GitHub, number: u64, error: anyhow::Error) -> anyhow::Error {
    if !crate::gh::gone(&error).is_some_and(|gone| gone.how != crate::gh::Went::Deleted) {
        return error;
    }
    let own = crate::checkout_list::named_repo(repo)
        .map(|name| name.to_string())
        .or_else(|_| crate::checkout_list::repo_of(repo))
        .unwrap_or_else(|_| "<its repo>".to_string());
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

/// What `afkconvoy intake --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When a worker has an issue the fleet is to work with no human labelling it: one a patrol opened, or one a stage split off its own. It is the intake a worker makes — a human takes an issue in by putting its flow label on, on GitHub.

```
afkconvoy intake --issue 12 --workflow feature
```

A stage that splits its issue runs it once per child, saying with `--blocked-by` which of the others a child has to wait for, and then declares `blocked` on all of them (`afkconvoy verdict --help`):

```
afkconvoy intake --issue 13 --workflow fix --blocked-by 11,12
```

Fleet repairs are dispatched automatically outside product workflows; no intake is needed.

## Who runs it

A worker, in its own worktree: it starts in its role's directory, so it changes to the worktree first, as the line its brief gives for `afkconvoy verdict` does. The fleet put its role and the issue it is on in its environment (AFKCONVOY_ROLE, AFKCONVOY_ISSUE), and the command reads them from there; `--role` and `--from` say them by hand. A patrol's worker is on no issue: it has its patrol's name there instead (AFKCONVOY_PATROL), which `--patrol` says by hand. The repair worker has no role and is on no issue and no patrol: it says `--repair`, in its own checkout, and nothing else of who it is.

## What it reads and writes

It reads the config of the checkout, the team it names, the issue, and — to count — the comments of every open issue that carries a flow label. It writes on the issue, in this order: a blocked-by edge to each issue `--blocked-by` names that GitHub does not record yet; one comment, the intake record, whose marker carries the workflow, the role and the issue the worker was on — or the patrol it ran, or, of the repair worker's, `by=repair` and nothing else; then, where the checkout's tracker is another repo (`afkconvoy run --tracker`) and the issue lacks it, this repo's repo label, `repo:<owner>/<repo>`; then the workflow's flow label, `flow:<workflow>`. Nothing but this command writes that format. Then it wakes the fleet, as `afkconvoy wake` does: the issue is on the frontier at the next tick — or, with an open issue to wait for, once that has closed. The edges go first, so the issue is never on the frontier ahead of what blocks it.

For a Jira delivery, run `afkconvoy flows --delivery <identity>`, open with its line (no runnable label), then run its intake_line. `--delivery <identity>` attaches the existing delivery before any runnable label is added. In a bound repo select --workflow; in an unbound repo omit it and intake adds the ready label. An operator or discussion agent may use explicit --delivery without a worker role: this follows human intake and consumes no worker intake limit. Workers still name their role and source task or patrol and count against the existing limit.

A worker's --from (or its worker issue environment) inherits that source task's delivery automatically. Explicit --delivery selects the named delivery instead of inheritance for work genuinely belonging to another delivery. A dependency, parent link, checkout branch or discussion title supplies no delivery. Keep --blocked-by separate: it orders tasks and never assigns membership. The delivery reference lives once in the task's remote target record; stages, restart, rework and harness/model switches retain it. Missing, contradictory and cross-repo delivery context refuses without a runnable label being added. Existing pinned targets cannot be reassigned; claims, retained work and unaccounted PRs/candidates are preserved and reported.

The issue, the issues it waits for and every issue it counts are the tracker's: the checkout's own repo, or the one its host says, where it counts only the issues that carry this repo's repo label.

An issue not found there is refused naming the tracker, where `gh` finds it in the checkout's repo (following a transfer), and `gh issue transfer <n> <tracker> --repo <its repo>`. Use the number GitHub assigns after transfer. For another repo's blocker, run `afkconvoy flows` in that repo's checkout, open it on the tracker with that repo's repo label using its line (omit the flow label until intake), and take it in there.

The limit. Before it writes anything it counts the open issues taken in by workers: those that carry a flow label and an intake record for it. With that count at the repo's `intake_limit` (see `afkconvoy config --help`) it refuses and writes nothing. The count is taken from the records each time and stored nowhere, so closing such an issue makes room. An issue a human labelled has no record: it is never counted, and nothing here refuses a human's label.

## What it answers

{"issue", "workflow", "taken", "role", "from", "blocked_by", "open", "limit", "woke"}

Delivery intake also answers `delivery` and resolved `target_branch`. Human delivery intake omits worker identity, open and limit. Unbound intake omits workflow and answers target_branch for ordinary tasks too.

- taken: true: the record and the flow label were written
- `role`, `from`: who took it in: the worker's role, and the issue it was on. A patrol's worker answers `patrol`, the patrol's name, in place of `from`. The repair worker answers "repair": true in place of both
- `blocked_by`: the issues it was said to wait for
- `open`: the open issues taken in by workers, this one among them
- `limit`: the repo's `intake_limit`
- `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers

{"issue", "workflow", "taken": false, "standing"}

- taken: false: the issue follows that workflow already: nothing was written
- `standing`: how it came in: {"by": "worker", "role", "from", "url"} with the comment of its intake record — `patrol` in place of `from` where a patrol's worker took it in, "repair": true in place of both where the repair worker did — or {"by": "human"}

Errors, as {"error": …} with a non-zero exit: the intake limit is reached — the error says the count and the limit; the team has no workflow of that name — the error lists the ones it has; the team has no such role, or no such patrol; the repo is unbound; the issue is closed, is a pull request, already carries the flow label of another workflow, or carries the repo label of another repo — it is that repo's to work — or, in a repo that is its own tracker, any repo label; the role, or both the issue the worker is on and its patrol, is neither in the environment nor given, and `--repair` is not given; `--repair` with a role, an issue or a patrol, given or in the environment; both `--from` and `--patrol` are given; `--blocked-by` names the issue itself, a pull request, or an issue that cannot be read; the team has a fault; "is not in a git checkout", or git's own words for a checkout it refuses.

## What trips you up

- It is the only way a worker's intake is on the record. A flow label a worker puts on by itself would read as a human's and escape the limit: the guard refuses that, and an intake record written by hand (`afkconvoy guard --help`).
- An issue a patrol's worker takes in counts against two limits: the repo's `intake_limit`, which refuses here, and the patrol's own `max_open`, which refuses nothing here — the fleet does not run the patrol again while that many are open.
- The limit bounds the work the fleet gives itself, not a day's work: it is set high, to stop a runaway. `afkconvoy plan` reports the count against it.
- The record counts only while the issue carries the flow label it names. A human who moves the issue to another workflow has taken it in themselves.
- A split is nothing but this and a `blocked` verdict: the parent is parked on its children, and the stage that split it runs again when all of them have closed. A child counts against the limit like any issue a worker takes in, and may be split in turn.
- An issue the repair worker takes in counts against the limit too. This compatibility intake is distinct from the fleet's automatic repair issue: that issue carries the ready and convoy-repair labels and runs outside product workflows.
- A child has to close for its parent to return. One whose workflow ends in `done` stays open until somebody closes it.
- `--blocked-by` is read only as the issue is taken in. On one that follows the workflow already, nothing is written.
- Two workers taking in at the same moment can both pass the count: the limit can be overshot by as many as ran at once."#;

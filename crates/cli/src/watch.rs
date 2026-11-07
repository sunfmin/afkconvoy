//! A claim whose PR is open and not on the base branch yet: asking after
//! its worker, and the one restart a silent one gets.

use afkconvoy_core::claims::Claim;
use afkconvoy_core::ending::Ending;
use afkconvoy_core::landing_worker::{
    Answered, LandingRecord, LandingRoute, RestartStep, Unrestarted, join_comment, join_route,
    read_join, stalled_reason,
};
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::nudge_line;
use afkconvoy_core::stopped::{Route, past_grace, settled_by_state};
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::claims::Me;
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::gh::Comment;
use crate::judge::{
    Handed, Look, escalate_claim, mine, nudge_worker, own_background, progress, where_it_stopped,
};
use crate::orca::{Source, Worktree};

/// A landing worker of mine that stopped, and what was gathered to route it.
struct Stopped {
    claim: Claim,
    pr: u64,
    worktree: Worktree,
    /// The PR's join record, and the comments it was read from.
    record: LandingRecord,
    comments: Vec<Comment>,
    route: LandingRoute,
}

/// What asking after the worker of a landing claim of mine came to.
enum Asked {
    /// At work: nothing on GitHub was read.
    AtWork,
    /// Gone, its terminal or its whole worktree: nothing on GitHub was read.
    Gone,
    /// Its PR is not open any more: what the claim is now is the next
    /// rebuild's to say.
    NoPr,
    Stopped(Box<Stopped>),
}

/// Asks after the worker of my claim on `issue`; `None` when I hold none.
fn ask_after(fleet: &Fleet, me: &Me, issue: u64) -> Result<Option<Asked>> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(None) };
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    let state = match &worktree {
        Some(worktree) => Some(worktree.state(fleet.now)?),
        None => None,
    };
    match settled_by_state(state) {
        Some(Route::Leave) => return Ok(Some(Asked::AtWork)),
        Some(_) => return Ok(Some(Asked::Gone)),
        None => {}
    }
    let (Some(worktree), Some(state)) = (worktree, state) else {
        bail!("issue #{issue}: a worker that stopped has a worktree and a state");
    };
    let prs = fleet.gh.open_prs()?;
    let Some(pr) = closing_pr(&prs, issue).map(|pr| pr.number) else {
        return Ok(Some(Asked::NoPr));
    };
    let (record, comments) = join_record(fleet, pr)?;
    let progress = progress(&worktree, &fleet.task_tip(issue)?)?;
    let (nudged, handed) = (worktree.nudged()?, worktree.handed(record.restarted)?);
    let past_grace = past_grace(state, &progress, nudged, handed, fleet.now);
    let background = own_background(fleet, &worktree, past_grace, Look::Acting)?;
    let route =
        join_route(state, &progress, nudged, record.restarted, (handed, background), fleet.now);
    Ok(Some(Asked::Stopped(Box::new(Stopped { claim, pr, worktree, record, comments, route }))))
}

/// Handles a landing claim's turn before worker health can hold its harness.
/// A stopped or gone worker whose turn answered a wait needs no harness or
/// judgment to ask again, unless an unchanged blocked answer holds the turn.
/// An answer needing tree changes is handed to the
/// worker before silence can mistake the fleet's merge for worker progress;
/// that handoff takes a free slot. An open harness turn asking for a wait is
/// left to its worker.
/// `None` leaves ordinary harness recovery to the tick.
pub(crate) fn answer_turn(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    issue: u64,
    start: bool,
    dependencies: &[crate::gh::BlockedBy],
) -> Result<Option<Outcome>> {
    if mine(fleet, me, issue)?.is_none() {
        return Ok(None);
    }
    let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)? else {
        return Ok(None);
    };
    let answered = crate::turn_answer::read(&worktree)?;
    if answered == Answered::Asking {
        return Ok(Some(Outcome::Nothing));
    }
    if let Some(handoff) = crate::turn_answer::pending(&worktree)? {
        if !start {
            return Ok(Some(Outcome::Nothing));
        }
        if worktree.state(fleet.now)? == afkconvoy_core::worker::WorkerState::Gone
            || crate::dispatch::incomplete(fleet, issue)?
        {
            let started = dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            if started["started"] != true {
                return Ok(Some(Outcome::Nothing));
            }
        } else {
            let terminal = worktree.terminal()?.ok_or_else(|| {
                anyhow::anyhow!("issue #{issue}: no terminal for its turn's answer")
            })?;
            worktree.hand_note(&terminal, &handoff.line, &handoff.mark, 0)?;
        }
        handoff.taken(&worktree, fleet.now)?;
        return Ok(Some(Outcome::Told));
    }
    if !answered.waits() || worktree.state(fleet.now)? == afkconvoy_core::worker::WorkerState::Busy
    {
        return Ok(None);
    }
    if matches!(&answered, Answered::Said(outcome) if outcome == Ending::Blocked.word())
        && crate::turn_answer::blocked_unchanged(&worktree, dependencies)?
    {
        return Ok(Some(Outcome::Nothing));
    }
    // A pass asks after no landing a standing lock keeps it off: one that
    // stopped at a lock is asked here only once that lock is gone.
    let locks = crate::turn_answer::stopped_at_locks(&worktree)?;
    crate::turn_answer::ask_again(&worktree, issue)?;
    if matches!(&answered, Answered::Said(outcome) if outcome == Ending::Locked.word()) {
        fleet.after_locks.push((issue, locks));
    }
    Ok(Some(Outcome::Asked))
}

/// The join record of `pr`, and the PR's comments it was read from.
pub(crate) fn join_record(fleet: &Fleet, pr: u64) -> Result<(LandingRecord, Vec<Comment>)> {
    let comments = fleet.gh.pr_comments(pr)?;
    Ok((read_join(comments.iter().map(|c| c.body.as_str())), comments))
}

/// Writes the join record of `pr` into its one marker comment, posted when the
/// PR has none. `comments` are the ones `record` was read from.
pub(crate) fn record_join(
    fleet: &Fleet,
    pr: u64,
    record: &LandingRecord,
    comments: &[Comment],
) -> Result<()> {
    let body = join_comment(record);
    match record.comment.and_then(|at| comments.get(at)) {
        Some(comment) => fleet.gh.edit_pr_comment(comment.id, &body),
        None => fleet.gh.comment_on_pr(pr, &body),
    }
}

/// Replaces the silent worker joining `pr`: the restart is recorded on the PR,
/// then a worker is started in the same worktree — which closes the idle
/// session — briefed only to land the PR.
fn restart_worker(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    issue: u64,
    found: &Stopped,
    unrestarted: Unrestarted,
) -> Result<Value> {
    let mut started = Value::Null;
    afkconvoy_core::landing_worker::restart(unrestarted, |step| {
        match step {
            RestartStep::Record => {
                let record = LandingRecord { restarted: Some(fleet.now), ..found.record.clone() };
                record_join(fleet, found.pr, &record, &found.comments)?;
            }
            RestartStep::Replace => {
                started = dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(started)
}

/// `afkconvoy restart`: replace the silent worker of one of my claims whose
/// PR is waiting to land. Refused for any worker the tick would not restart
/// — at work, heard from within grace, not nudged yet — and for a PR whose
/// join already had its restart.
pub fn restart(fleet: &mut Fleet, me: &Me, agent: &Agent, issue: u64) -> Result<Value> {
    let Some(Asked::Stopped(found)) = ask_after(fleet, me, issue)? else {
        bail!(
            "issue #{issue} has no stopped worker of {} on this host with a PR joining the \
             train; nothing was restarted",
            me.instance
        );
    };
    let mut found = *found;
    let pr = found.pr;
    let route = std::mem::replace(&mut found.route, LandingRoute::Leave);
    let LandingRoute::Restart(unrestarted) = route else {
        bail!(
            "the worker on issue #{issue} is not restarted: {}; nothing was changed",
            match route {
                LandingRoute::Stalled(_) =>
                    "PR's join already had its one restart, and a silence past it escalates",
                LandingRoute::Nudge(_) => "it was not nudged yet",
                LandingRoute::Waiting(_) => "it waits on background work of its own, still running",
                _ => "it showed a sign of life within the grace period",
            }
        );
    };
    let started = restart_worker(fleet, me, agent, issue, &found, unrestarted)?;
    Ok(json!({"issue": issue, "pr": pr, "action": "restarted", "restarted": fleet.now,
        "terminal": started["terminal"], "prompt": started["prompt"]}))
}

/// The tick's look at one claim of mine whose PR is joining, after
/// `answer_turn` has handled the last turn's answer. A worker gone is
/// continued onto the join, one that stopped climbs the ladder (`joining::join_route`) →
/// `Done` when that ended the claim. `start` false: no worker is started —
/// the claim takes no slot, and none is free. The PR and the branch are never
/// touched; the worktree goes only with an escalation, and only when the
/// remote holds all of it.
pub fn watch(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    issue: u64,
    start: bool,
) -> Result<Outcome> {
    let found = match ask_after(fleet, me, issue)? {
        None | Some(Asked::AtWork | Asked::NoPr) => return Ok(Outcome::Nothing),
        Some(Asked::Gone) if !start => return Ok(Outcome::Nothing),
        Some(Asked::Gone) => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        Some(Asked::Stopped(found)) => *found,
    };
    let mut found = found;
    match std::mem::replace(&mut found.route, LandingRoute::Leave) {
        LandingRoute::Leave => Ok(Outcome::Nothing),
        LandingRoute::Waiting(since) => Ok(Outcome::Waiting(since)),
        LandingRoute::Continue | LandingRoute::Restart(_) if !start => Ok(Outcome::Nothing),
        LandingRoute::Continue => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            Ok(Outcome::Continued)
        }
        LandingRoute::Nudge(unnudged) => {
            nudge_worker(&found.worktree, unnudged, nudge_line, fleet.now)?;
            Ok(Outcome::Nudged)
        }
        LandingRoute::Restart(unrestarted) => {
            restart_worker(fleet, me, agent, issue, &found, unrestarted)?;
            Ok(Outcome::Restarted)
        }
        LandingRoute::Stalled(silent) => {
            let tail = where_it_stopped(&found.worktree, &silent);
            let reason = stalled_reason(found.pr, &silent, &tail);
            let reason = Handed::new("its worker went silent waiting to land", reason);
            let pr = Some(found.pr);
            escalate_claim(fleet, &found.claim, &reason, pr, Some(&found.worktree))?;
            Ok(Outcome::Done)
        }
    }
}

/// What `afkconvoy restart --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

To replace a silent worker by hand. A tick does this on its own, once, for a claim whose PR is open and not on the base branch, and whose worker stopped, was nudged, and stayed silent. Do it yourself when a plan keeps showing such a claim under `mine` as `landing`.

## Who runs it

The operator, in a checkout of the repo, on the host the worker runs on.

## What it reads and writes

It records the restart in the fleet's comment on the PR, then starts a new worker in the same worktree, briefed only to land in a turn — in a bound repo, in the landing role of the issue's workflow. The PR, the branch and the worktree are untouched.

## What it answers

{"issue", "pr", "action": "restarted", "restarted", "terminal", "prompt"} — restarted is the time, in epoch seconds; prompt is the brief the new worker got.

Errors, each with nothing changed: "has no stopped worker" — the instance holds no claim on the issue, its worker is at work or gone, or the issue has no open PR; "is not restarted" — the PR's landing worker already had its restart, the worker was not nudged yet, or it showed a sign of life within the grace period; the claim is another instance's.

## What trips you up

- `--instance` is required, and must be the instance that holds the claim.
- One restart per PR. A worker silent again after it is escalated by the next tick.
- It is not `dispatch`: for a claim with no PR, continue it with `dispatch`."#;

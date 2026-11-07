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
use crate::found::Found;
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

/// The last turn of a landing claim of mine, as the tick reads it just
/// before the `Watch` step on it (`turn`).
pub(crate) struct Turn {
    worktree: Worktree,
    answered: Answered,
    /// An answer needing tree changes, still to hand to the worker.
    handoff: Option<crate::turn_answer::Handoff>,
}

/// The last turn of my landing claim on `issue`, where it answered what the
/// tick handles before worker health can hold its harness: a turn still
/// asking, an answer to hand to the worker, or a wait to ask again for
/// while its worker is not at work. `None` leaves the claim to the step's
/// own look at its worker. Reads only.
pub(crate) fn turn(fleet: &Fleet, me: &Me, issue: u64) -> Result<Option<Turn>> {
    if mine(fleet, me, issue)?.is_none() {
        return Ok(None);
    }
    let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)? else {
        return Ok(None);
    };
    let answered = crate::turn_answer::read(&worktree)?;
    if answered == Answered::Asking {
        return Ok(Some(Turn { worktree, answered, handoff: None }));
    }
    let handoff = crate::turn_answer::pending(&worktree)?;
    if handoff.is_none()
        && (!answered.waits()
            || worktree.state(fleet.now)? == afkconvoy_core::worker::WorkerState::Busy)
    {
        return Ok(None);
    }
    Ok(Some(Turn { worktree, answered, handoff }))
}

/// Handles `turn`, the last turn of my landing claim on `issue`, before
/// worker health can hold its harness (`afkconvoy_core::tick::First::Answer`).
/// A stopped or gone worker whose turn answered a wait needs no harness or
/// judgment to ask again, unless an unchanged blocked answer holds the turn.
/// An answer needing tree changes is handed to the
/// worker before silence can mistake the fleet's merge for worker progress;
/// that handoff takes a free slot. An open harness turn asking for a wait is
/// left to its worker.
pub(crate) fn answer_turn(
    fleet: &mut Fleet,
    found: &mut Found,
    (me, agent): (&Me, &Agent),
    issue: u64,
    turn: Turn,
    start: bool,
    dependencies: &[crate::gh::BlockedBy],
) -> Result<Outcome> {
    let Turn { worktree, answered, handoff } = turn;
    if answered == Answered::Asking {
        return Ok(Outcome::Nothing);
    }
    if let Some(handoff) = handoff {
        if !start {
            return Ok(Outcome::Nothing);
        }
        if worktree.state(fleet.now)? == afkconvoy_core::worker::WorkerState::Gone
            || crate::dispatch::incomplete(fleet, issue)?
        {
            let started = dispatch(fleet, found, me, agent, issue, StartFrom::Auto)?;
            if started["started"] != true {
                return Ok(Outcome::Nothing);
            }
        } else {
            let terminal = worktree.terminal()?.ok_or_else(|| {
                anyhow::anyhow!("issue #{issue}: no terminal for its turn's answer")
            })?;
            worktree.hand_note(&terminal, &handoff.line, &handoff.mark, 0)?;
        }
        handoff.taken(&worktree, fleet.now)?;
        return Ok(Outcome::Told);
    }
    if matches!(&answered, Answered::Said(outcome) if outcome == Ending::Blocked.word())
        && crate::turn_answer::blocked_unchanged(&worktree, dependencies)?
    {
        return Ok(Outcome::Nothing);
    }
    // A pass asks after no landing a standing lock keeps it off: one that
    // stopped at a lock is asked here only once that lock is gone.
    let locks = crate::turn_answer::stopped_at_locks(&worktree)?;
    crate::turn_answer::ask_again(&worktree, issue)?;
    if matches!(&answered, Answered::Said(outcome) if outcome == Ending::Locked.word()) {
        found.after_locks.push((issue, locks));
    }
    Ok(Outcome::Asked)
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
    found: &mut Found,
    me: &Me,
    agent: &Agent,
    issue: u64,
    stopped: &Stopped,
    unrestarted: Unrestarted,
) -> Result<Value> {
    let mut started = Value::Null;
    afkconvoy_core::landing_worker::restart(unrestarted, |step| {
        match step {
            RestartStep::Record => {
                let record = LandingRecord { restarted: Some(fleet.now), ..stopped.record.clone() };
                record_join(fleet, stopped.pr, &record, &stopped.comments)?;
            }
            RestartStep::Replace => {
                started = dispatch(fleet, found, me, agent, issue, StartFrom::Auto)?;
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
    let Some(Asked::Stopped(stopped)) = ask_after(fleet, me, issue)? else {
        bail!(
            "issue #{issue} has no stopped worker of {} on this host with a PR joining the \
             train; nothing was restarted",
            me.instance
        );
    };
    let mut stopped = *stopped;
    let pr = stopped.pr;
    let route = std::mem::replace(&mut stopped.route, LandingRoute::Leave);
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
    // A command run by hand has no tick's line to say what it found.
    let found = &mut Found::default();
    let started = restart_worker(fleet, found, me, agent, issue, &stopped, unrestarted)?;
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
    found: &mut Found,
    me: &Me,
    agent: &Agent,
    issue: u64,
    start: bool,
) -> Result<Outcome> {
    let stopped = match ask_after(fleet, me, issue)? {
        None | Some(Asked::AtWork | Asked::NoPr) => return Ok(Outcome::Nothing),
        Some(Asked::Gone) if !start => return Ok(Outcome::Nothing),
        Some(Asked::Gone) => {
            dispatch(fleet, found, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        Some(Asked::Stopped(stopped)) => *stopped,
    };
    let mut stopped = stopped;
    match std::mem::replace(&mut stopped.route, LandingRoute::Leave) {
        LandingRoute::Leave => Ok(Outcome::Nothing),
        LandingRoute::Waiting(since) => Ok(Outcome::Waiting(since)),
        LandingRoute::Continue | LandingRoute::Restart(_) if !start => Ok(Outcome::Nothing),
        LandingRoute::Continue => {
            dispatch(fleet, found, me, agent, issue, StartFrom::Auto)?;
            Ok(Outcome::Continued)
        }
        LandingRoute::Nudge(unnudged) => {
            nudge_worker(&stopped.worktree, unnudged, nudge_line, fleet.now)?;
            Ok(Outcome::Nudged)
        }
        LandingRoute::Restart(unrestarted) => {
            restart_worker(fleet, found, me, agent, issue, &stopped, unrestarted)?;
            Ok(Outcome::Restarted)
        }
        LandingRoute::Stalled(silent) => {
            let tail = where_it_stopped(&stopped.worktree, &silent);
            let reason = stalled_reason(stopped.pr, &silent, &tail);
            let reason = Handed::new("its worker went silent waiting to land", reason);
            let pr = Some(stopped.pr);
            escalate_claim(fleet, found, &stopped.claim, &reason, pr, Some(&stopped.worktree))?;
            Ok(Outcome::Done)
        }
    }
}

//! A claim whose PR is open and not on the landing train yet: asking after
//! its worker, and the one restart a silent one gets.

use afkconvoy_core::claims::Claim;
use afkconvoy_core::joining::{
    JoinRoute, RestartStep, Unrestarted, join_comment, join_route, restarted_at, stalled_reason,
};
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::stopped::{Route, settled_by_state};
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::claims::Me;
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::judge::{escalate_claim, mine, nudge_worker, progress, where_it_stopped};
use crate::orca::{Source, Worktree};

/// A joining worker of mine that stopped, and what was gathered to route it.
struct Stopped {
    claim: Claim,
    pr: u64,
    worktree: Worktree,
    route: JoinRoute,
}

/// What asking after the worker of a joining claim of mine came to.
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
    let comments = fleet.gh.comments(pr)?;
    let restarted = restarted_at(comments.iter().map(|c| c.body.as_str()));
    let progress = progress(&worktree, &fleet.base_tip()?)?;
    let route = join_route(state, &progress, worktree.nudged()?, restarted, fleet.now);
    Ok(Some(Asked::Stopped(Box::new(Stopped { claim, pr, worktree, route }))))
}

/// Replaces the silent worker joining `pr`: the restart is recorded on the PR,
/// then a worker is started in the same worktree — which closes the idle
/// session — briefed only to join the train.
fn restart_worker(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    issue: u64,
    pr: u64,
    unrestarted: Unrestarted,
) -> Result<Value> {
    let mut started = Value::Null;
    afkconvoy_core::joining::restart(unrestarted, |step| {
        match step {
            RestartStep::Record => fleet.gh.comment(pr, &join_comment(fleet.now))?,
            RestartStep::Replace => {
                started = dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(started)
}

/// `afkconvoy restart`: replace the silent worker of one of my claims whose
/// PR is joining the train. Refused for any worker the tick would not restart
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
    let Stopped { pr, route, .. } = *found;
    let JoinRoute::Restart(unrestarted) = route else {
        bail!(
            "the worker on issue #{issue} is not restarted: {}; nothing was changed",
            match route {
                JoinRoute::Stalled(_) =>
                    "PR's join already had its one restart, and a silence past it escalates",
                JoinRoute::Nudge(_) => "it was not nudged yet",
                _ => "it showed a sign of life within the grace period",
            }
        );
    };
    let started = restart_worker(fleet, me, agent, issue, pr, unrestarted)?;
    Ok(json!({"issue": issue, "pr": pr, "action": "restarted", "restarted": fleet.now,
        "terminal": started["terminal"], "prompt": started["prompt"]}))
}

/// The tick's look at one claim of mine whose PR is joining: a worker that is
/// gone is continued onto the join, one that stopped climbs the ladder
/// (`joining::join_route`) → `Done` when that ended the claim. The PR, the
/// branch and the worktree are never touched.
pub fn watch(fleet: &mut Fleet, me: &Me, agent: &Agent, issue: u64) -> Result<Outcome> {
    let found = match ask_after(fleet, me, issue)? {
        None | Some(Asked::AtWork | Asked::NoPr) => return Ok(Outcome::Nothing),
        Some(Asked::Gone) => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        Some(Asked::Stopped(found)) => *found,
    };
    let Stopped { claim, pr, worktree, route } = found;
    match route {
        JoinRoute::Leave => Ok(Outcome::Nothing),
        JoinRoute::Continue => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            Ok(Outcome::Continued)
        }
        JoinRoute::Nudge(unnudged) => {
            nudge_worker(&worktree, unnudged, fleet.now)?;
            Ok(Outcome::Nudged)
        }
        JoinRoute::Restart(unrestarted) => {
            restart_worker(fleet, me, agent, issue, pr, unrestarted)?;
            Ok(Outcome::Restarted)
        }
        JoinRoute::Stalled(silent) => {
            let reason = stalled_reason(pr, &silent, &where_it_stopped(&worktree, &silent));
            // No proof of a workless branch is offered: the worktree is kept.
            escalate_claim(fleet, &claim, &reason, Some(&worktree), None)?;
            Ok(Outcome::Done)
        }
    }
}

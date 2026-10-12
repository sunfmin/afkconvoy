//! A bound repo's claim at a stage (docs/adr/0010): carrying out what the
//! stage declared, asking after its worker while it has declared nothing,
//! and handing to a human an issue nobody holds. What the stage records call
//! for is the core's to say (`afkconvoy_core::stage_record::advance`).

use afkconvoy_core::blocked::{Blocked, blocked_route};
use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::stage_nudge_line;
use afkconvoy_core::stage_record::Advance;
use afkconvoy_core::stopped::{
    Route, Silence, Why, Workless, settled_by_state, silence, stage_stalled_reason,
};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition::{self, HandOnStep, Released};
use anyhow::{Result, anyhow, bail};

use crate::board::last_word;
use crate::claims::{Me, Take, delete, take};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::judge::{
    close_claim, escalate_claim, mine, nudge_worker, progress, where_it_stopped, worktree_and_work,
};
use crate::orca::{Source, Worktree};
use crate::park::{park_claim, standings};
use crate::stage::Flows;
use crate::wake::wake;

/// The issue's open PR, when it has one: the status board keeps it.
fn pr_of(fleet: &Fleet, issue: u64) -> Result<Option<u64>> {
    Ok(closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number))
}

/// A reason as the comment of an escalation opens: one sentence, and where
/// the stage said it when it did.
fn stated(reason: &str, said: Option<&str>) -> String {
    let mut letters = reason.chars();
    let first = letters.next().map(|first| first.to_uppercase().to_string()).unwrap_or_default();
    let sentence = format!("{first}{}.", letters.as_str());
    match said {
        Some(url) => format!("{sentence}\n\nIts stage record: {url}"),
        None => sentence,
    }
}

/// Ends `claim` once its stage has declared, the issue left where its stage
/// records put it. `ended` is given when its workflow has ended: a worktree
/// whose branch then holds no work goes too.
fn hand_on(
    fleet: &mut Fleet,
    claim: &Claim,
    ended: Option<(&Worktree, &Workless)>,
) -> Result<Released> {
    let issue = claim.number;
    let pr = pr_of(fleet, issue)?;
    transition::hand_on(issue, ended.map(|(_, workless)| workless), |step| {
        match step {
            HandOnStep::Board => last_word(fleet, claim, Stage::Declared, pr, &[])?,
            HandOnStep::Release => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            HandOnStep::RemoveWorktree => {
                if let Some((worktree, _)) = ended {
                    worktree.remove()?;
                }
            }
        }
        Ok(())
    })
}

/// Asks after the worker of `claim`, whose stage `stage` has declared
/// nothing: one that is gone is continued, one that stopped is nudged once,
/// and one silent past its nudge is escalated, as a silent worker ever is.
fn ask_after(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    claim: &Claim,
    stage: &str,
) -> Result<Outcome> {
    let issue = claim.number;
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    let state = match &worktree {
        Some(worktree) => Some(worktree.state(fleet.now)?),
        None => None,
    };
    match settled_by_state(state) {
        Some(Route::Leave) => return Ok(Outcome::Nothing),
        Some(_) => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        None => {}
    }
    let (Some(worktree), Some(state)) = (worktree, state) else {
        bail!("issue #{issue}: a worker that stopped has a worktree and a state");
    };
    let progress = progress(&worktree, &fleet.base_tip()?)?;
    match silence(state, &progress, worktree.nudged()?, None, fleet.now) {
        Silence::Lively => Ok(Outcome::Nothing),
        Silence::Gone => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            Ok(Outcome::Continued)
        }
        Silence::Nudge(unnudged) => {
            nudge_worker(&worktree, unnudged, stage_nudge_line, fleet.now)?;
            Ok(Outcome::Nudged)
        }
        Silence::Stalled(silent) => {
            let tail = where_it_stopped(&worktree, &silent);
            let reason = stage_stalled_reason(stage, &silent, &tail);
            let (pr, workless) = (pr_of(fleet, issue)?, Workless::of(&progress));
            escalate_claim(fleet, claim, &reason, pr, Some(&worktree), workless.as_ref())?;
            Ok(Outcome::Done)
        }
    }
}

/// Why the stage that declared under `claim` failed for what it left behind,
/// when it did: marked as writing nothing, it left a commit on the branch. A
/// guard refuses the commit as it is made; this is the check of what a guard
/// missed (docs/adr/0012).
fn left_behind(fleet: &Fleet, claim: &Claim) -> Result<Option<String>> {
    let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, claim.number)? else {
        return Ok(None);
    };
    let Some(started) = worktree.writes_nothing()? else { return Ok(None) };
    Ok(started.broken(&claim.sha, &worktree.tip()?))
}

/// The tick's look at one claim of mine at a stage → `Done` when that ended
/// the claim. A verdict is carried out as soon as it is read, whatever its
/// worker is doing: it was declared through the fleet, and a claim declares
/// once. A stage that left what it may not have is escalated instead,
/// whatever it declared. The PR, the branch and the worktree are the issue's next stage's.
pub fn advance(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    flows: &Flows,
    issue: u64,
) -> Result<Outcome> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(Outcome::Nothing) };
    let called = flows.called(issue, &claim.sha)?;
    let said = called.said.as_deref();
    let declared = !matches!(called.advance, Advance::AskAfter);
    if declared && let Some(failed) = left_behind(fleet, &claim)? {
        let (worktree, workless) = worktree_and_work(fleet, issue)?;
        let (reason, pr) = (stated(&failed, said), pr_of(fleet, issue)?);
        escalate_claim(fleet, &claim, &reason, pr, worktree.as_ref(), workless.as_ref())?;
        return Ok(Outcome::Done);
    }
    let reason = match called.advance {
        Advance::AskAfter => {
            return ask_after(fleet, me, agent, &claim, called.stage.as_deref().unwrap_or("?"));
        }
        Advance::HandOn => {
            hand_on(fleet, &claim, None)?;
            // What follows is the next tick's to start: it need not wait
            // out a sleep. A wake nobody hears costs only that wait.
            let _ = wake(&fleet.repo);
            return Ok(Outcome::Done);
        }
        Advance::End => {
            let (worktree, workless) = worktree_and_work(fleet, issue)?;
            hand_on(fleet, &claim, worktree.as_ref().zip(workless.as_ref()))?;
            return Ok(Outcome::Done);
        }
        Advance::Close => {
            let (worktree, workless) = worktree_and_work(fleet, issue)?;
            if let Some(workless) = workless {
                close_claim(fleet, &claim, worktree.as_ref(), &workless)?;
                return Ok(Outcome::Done);
            }
            Why::Refuted.reason(said.unwrap_or("(its stage record)"))
        }
        Advance::Blocked(named) => match blocked_route(&standings(fleet, issue, &named)?) {
            Blocked::Park(parkable) => {
                let (worktree, workless) = worktree_and_work(fleet, issue)?;
                park_claim(fleet, &claim, &parkable, worktree.as_ref(), workless.as_ref())?;
                return Ok(Outcome::Done);
            }
            // Every blocker closed: the frontier holds the issue at the
            // stage that parked it.
            Blocked::Redispatch => {
                hand_on(fleet, &claim, None)?;
                return Ok(Outcome::Done);
            }
            Blocked::Escalate(reason) => stated(&reason.to_string(), said),
        },
        Advance::Escalate(reason) => stated(&reason, said),
    };
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    let pr = pr_of(fleet, issue)?;
    escalate_claim(fleet, &claim, &reason, pr, worktree.as_ref(), workless.as_ref())?;
    Ok(Outcome::Done)
}

/// The tick's step for an unclaimed issue that calls for a human: claim it,
/// so that one fleet instance says so once, and escalate it for `reason`.
pub fn hand_to_human(fleet: &mut Fleet, me: &Me, issue: u64, reason: &str) -> Result<Outcome> {
    if take(&fleet.repo, &mut fleet.scan, me, issue, Take::Fresh)?["won"] != true {
        return Ok(Outcome::Lost);
    }
    let claim = mine(fleet, me, issue)?.ok_or_else(|| anyhow!("issue #{issue}: no claim"))?;
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    let pr = pr_of(fleet, issue)?;
    escalate_claim(fleet, &claim, &stated(reason, None), pr, worktree.as_ref(), workless.as_ref())?;
    Ok(Outcome::Done)
}

//! A bound repo's claim at a stage (docs/adr/0010): carrying out what the
//! stage declared, asking after its worker while it has declared nothing,
//! and handing to a human an issue nobody holds. What the stage records call
//! for is the core's to say (`afkconvoy_core::stage_record::advance`).

use afkconvoy_core::account::End;
use afkconvoy_core::blocked::Blocked;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::position::Failure;
use afkconvoy_core::prompt::stage_nudge_line;
use afkconvoy_core::stage_record::{Advance, StageRecord};
use afkconvoy_core::stopped::{
    Nudged, Route, Silence, Why, settled_by_state, stage_stalled_reason,
};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition::{self, HandOnStep, Released};
use anyhow::{Result, anyhow, bail};

use crate::board::declared;
use crate::claims::{Me, Take, delete, take};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::found::Found;
use crate::judge::{
    BLOCKED_BRIEF, Handed, Look, close_claim, escalate_claim, mine, nudge_worker, refuted_work,
    silence_of, where_it_stopped, worktree_and_work, worktree_of,
};
use crate::lock::explaining_lock;
use crate::orca::{Source, Worktree};
use crate::park::{park_claim, routed};
use crate::stage::Flows;
use crate::wake::wake;

/// The issue's open PR, when it has one: the status board keeps it.
fn pr_of(fleet: &Fleet, issue: u64) -> Result<Option<u64>> {
    Ok(closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number))
}

/// A reason as the comment of an escalation opens: one sentence, where the
/// stage said it when it did, and every round of the rework a breaker
/// stopped when one did.
pub(crate) fn stated(reason: &str, said: Option<&str>, rounds: Option<&str>) -> String {
    let mut letters = reason.chars();
    let first = letters.next().map(|first| first.to_uppercase().to_string()).unwrap_or_default();
    let mut stated = format!("{first}{}.", letters.as_str());
    if let Some(url) = said {
        stated.push_str(&format!("\n\nIts stage record: {url}"));
    }
    if let Some(rounds) = rounds {
        stated.push_str(&format!("\n\n{rounds}"));
    }
    stated
}

/// Ends `claim` once its stage has declared, the issue left where its stage
/// records put it — at `waits_at`, when that is a human's stage — and its
/// worker with it: its terminal is closed, and at `End::Ended`, its workflow
/// having ended, the worktree goes too. An `end` that leaves the work account
/// leaves it before the claim goes.
fn hand_on(fleet: &mut Fleet, claim: &Claim, waits_at: Option<&str>, end: End) -> Result<Released> {
    let issue = claim.number;
    let pr = pr_of(fleet, issue)?;
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    transition::hand_on(issue, end, |step| {
        match (step, &worktree) {
            (HandOnStep::Board, _) => declared(fleet, claim, pr, waits_at)?,
            (HandOnStep::Account, _) => {
                crate::account::leave(&fleet.repo, &fleet.gh, issue, end, fleet.now)?
            }
            (HandOnStep::Release, _) => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            (HandOnStep::RemoveWorktree, Some(worktree)) => worktree.remove()?,
            (HandOnStep::CloseTerminal, Some(worktree)) => worktree.close_terminals()?,
            // No worktree on this host: no worker of mine to end.
            (HandOnStep::RemoveWorktree | HandOnStep::CloseTerminal, None) => {}
        }
        Ok(())
    })
}

/// Hands `claim` on for `failure`, said as `why`: to its stage's consult,
/// where the stage names one that has not ruled on that kind of failure
/// since a human last put the issue at a stage (docs/adr/0018) — the fleet
/// leaves the record of the failure, with what a human handed it would be
/// told, and ends the claim, the worktree kept for the consult — and else to
/// a human, as ever.
fn hand_off(
    fleet: &mut Fleet,
    found: &mut Found,
    flows: &Flows,
    claim: &Claim,
    failure: Failure,
    why: &Handed,
    worktree: Option<&Worktree>,
) -> Result<Outcome> {
    let issue = claim.number;
    let Some(stage) = flows.consults(issue, failure) else {
        escalate_claim(fleet, found, claim, why, pr_of(fleet, issue)?, worktree)?;
        return Ok(Outcome::Done);
    };
    let record = StageRecord::failed(&stage, failure, &why.reason);
    if !flows.ends_with(issue, &record) {
        fleet.gh.comment(issue, &record.comment())?;
    }
    hand_on(fleet, claim, None, End::HandedOn)?;
    // The consult is the next tick's to start.
    let _ = wake(&fleet.repo, "");
    Ok(Outcome::Done)
}

/// Leaves on `issue` the record that its stage `stage` runs again, the
/// closed lock `lock` having explained what stopped it — once: a tick cut
/// short after it leaves no second.
fn leave_lock_closed(
    fleet: &Fleet,
    flows: &Flows,
    issue: u64,
    stage: &str,
    lock: u64,
) -> Result<()> {
    let record = StageRecord::lock_closed(stage, lock);
    if !flows.ends_with(issue, &record) {
        fleet.gh.comment(issue, &record.comment())?;
    }
    Ok(())
}

/// Ends `claim` with its stage `stage` to run again, not escalated: the
/// closed lock `lock` explains what stopped it. The frontier holds the issue
/// at that stage, for a worker whose brief says a lock stood.
fn run_again(
    fleet: &mut Fleet,
    flows: &Flows,
    claim: &Claim,
    stage: &str,
    lock: u64,
) -> Result<Outcome> {
    leave_lock_closed(fleet, flows, claim.number, stage, lock)?;
    hand_on(fleet, claim, None, End::HandedOn)?;
    // As after any verdict: the stage is the next tick's to start.
    let _ = wake(&fleet.repo, "");
    Ok(Outcome::Done)
}

/// The tick's step for an unclaimed issue that calls for a human, when a
/// closed lock explains the `escalate` that put it there: its stage runs
/// again instead → whether one did. Nothing is claimed: the record is all
/// there is to leave, and the frontier holds the issue from the next tick.
pub fn run_again_unclaimed(fleet: &Fleet, flows: &Flows, issue: u64) -> Result<bool> {
    let Some((stage, at)) = flows.stage_escalate(issue) else { return Ok(false) };
    let Some(lock) = explaining_lock(&fleet.gh, &fleet.config, at)? else { return Ok(false) };
    leave_lock_closed(fleet, flows, issue, &stage, lock)?;
    let _ = wake(&fleet.repo, "");
    Ok(true)
}

/// Asks after the worker of `claim`, whose stage `stage` has declared
/// nothing — or, `consult`, whose consult has neither ruled nor let its
/// `escalate` through: one that is gone is continued, one that stopped is nudged once,
/// and one silent past its nudge is escalated, as a silent worker ever is —
/// unless a lock that closed since the nudge explains the silence of a
/// stage's own worker: that stage runs again.
fn ask_after(
    fleet: &mut Fleet,
    found: &mut Found,
    me: &Me,
    agent: &Agent,
    flows: &Flows,
    claim: &Claim,
    (stage, consult): (&str, bool),
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
            dispatch(fleet, found, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        None => {}
    }
    let (Some(worktree), Some(state)) = (worktree, state) else {
        bail!("issue #{issue}: a worker that stopped has a worktree and a state");
    };
    match silence_of(fleet, issue, &worktree, state, None, Look::Acting)? {
        Silence::Lively => Ok(Outcome::Nothing),
        Silence::Waiting(since) => Ok(Outcome::Waiting(since)),
        Silence::Gone => {
            dispatch(fleet, found, me, agent, issue, StartFrom::Auto)?;
            Ok(Outcome::Continued)
        }
        Silence::Nudge(unnudged) => {
            nudge_worker(&worktree, unnudged, stage_nudge_line, fleet.now)?;
            Ok(Outcome::Nudged)
        }
        Silence::Stalled(silent) => {
            let nudged = match worktree.nudged()? {
                Nudged::At(at) if !consult => Some(at),
                _ => None,
            };
            if let Some(lock) = explaining_lock(&fleet.gh, &fleet.config, nudged)? {
                return run_again(fleet, flows, claim, stage, lock);
            }
            let tail = where_it_stopped(&worktree, &silent);
            let reason = stage_stalled_reason(stage, consult, &silent, &tail);
            let brief = if consult { "its consult went silent" } else { "its worker went silent" };
            let reason = Handed::at(Some(stage), brief, reason);
            if consult {
                // A consult's own failure is a human's. Its record, left
                // first, has the issue with that human: no tick puts it to
                // the consult again.
                let record = StageRecord::failed(stage, Failure::Silent, &reason.reason);
                if !flows.ends_with(issue, &record) {
                    fleet.gh.comment(issue, &record.comment())?;
                }
                escalate_claim(
                    fleet,
                    found,
                    claim,
                    &reason,
                    pr_of(fleet, issue)?,
                    Some(&worktree),
                )?;
                return Ok(Outcome::Done);
            }
            hand_off(fleet, found, flows, claim, Failure::Silent, &reason, Some(&worktree))
        }
    }
}

/// Why the stage that declared under `claim` failed for what it left behind,
/// when it did: marked as writing nothing, it left a commit on the branch. A
/// guard refuses the commit as it is made; this is the check of what a guard
/// missed (docs/adr/0012). With it, whether the stage's consult left it.
fn left_behind(fleet: &Fleet, claim: &Claim) -> Result<Option<(String, bool)>> {
    let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, claim.number)? else {
        return Ok(None);
    };
    let Some(started) = worktree.writes_nothing()? else { return Ok(None) };
    Ok(started.broken(&claim.id, &worktree.tip()?).map(|why| (why, started.consult)))
}

/// The tick's look at one claim of mine at a stage → `Done` when that ended
/// the claim. A verdict is carried out as soon as it is read, whatever its
/// worker is doing: it was declared through the fleet, and a claim declares
/// once. A stage that left what it may not have — and a consult that left a
/// commit — is escalated instead, whatever it declared. The PR, the branch
/// and the worktree are the issue's next stage's; the terminal is not: the
/// worker that declared is ended with its claim.
/// `HandedOn` when the issue was left where its stage records put it, for
/// whoever is due there: starting that worker is the pass's.
pub fn advance(
    fleet: &mut Fleet,
    found: &mut Found,
    me: &Me,
    agent: &Agent,
    flows: &Flows,
    issue: u64,
) -> Result<Outcome> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(Outcome::Nothing) };
    let called = flows.called(issue, &claim.id)?;
    let said = called.said.as_deref();
    let stage = called.stage.as_deref();
    let declared = !matches!(called.advance, Advance::AskAfter);
    if declared && let Some((failed, consult)) = left_behind(fleet, &claim)? {
        let worktree = worktree_of(fleet, issue)?;
        let reason = Handed::at(stage, "left a commit it may not", stated(&failed, said, None));
        // A consult's own failure is a human's: what it declared has it
        // seen the issue, and no tick puts it to the consult again.
        if consult {
            let pr = pr_of(fleet, issue)?;
            escalate_claim(fleet, found, &claim, &reason, pr, worktree.as_ref())?;
            return Ok(Outcome::Done);
        }
        return hand_off(
            fleet,
            found,
            flows,
            &claim,
            Failure::LeftCommit,
            &reason,
            worktree.as_ref(),
        );
    }
    // What a fleet lock explains is run again, not carried out: its stage's
    // `escalate` goes neither to a consult nor to a human.
    if let Some((stage, at)) = flows.stage_escalate(issue).filter(|_| declared)
        && let Some(lock) = explaining_lock(&fleet.gh, &fleet.config, at)?
    {
        return run_again(fleet, flows, &claim, &stage, lock);
    }
    let reason = match called.advance {
        Advance::AskAfter => {
            let asked = (called.stage.as_deref().unwrap_or("?"), called.consulted);
            return ask_after(fleet, found, me, agent, flows, &claim, asked);
        }
        Advance::HandOn => {
            hand_on(fleet, &claim, called.waits_at.as_deref(), End::HandedOn)?;
            // What follows that this tick does not start is the next one's:
            // it need not wait out a sleep. A wake nobody hears costs only
            // that wait.
            let _ = wake(&fleet.repo, "");
            return Ok(Outcome::HandedOn);
        }
        Advance::End => {
            hand_on(fleet, &claim, None, End::Ended)?;
            return Ok(Outcome::Done);
        }
        Advance::ConsultClose => {
            fleet.gh.close_issue(issue)?;
            hand_on(fleet, &claim, None, End::Closed)?;
            return Ok(Outcome::Done);
        }
        Advance::Close => {
            let (worktree, workless) = worktree_and_work(fleet, issue)?;
            if let Some(workless) = workless {
                close_claim(fleet, &claim, worktree.as_ref(), &workless)?;
                return Ok(Outcome::Done);
            }
            let reason = format!(
                "{}\n\n{}",
                Why::Refuted.reason(said.unwrap_or("(its stage record)")),
                refuted_work(fleet, issue, worktree.as_ref())?
            );
            let why = Handed::at(stage, Why::Refuted.brief(), reason);
            return hand_off(
                fleet,
                found,
                flows,
                &claim,
                Failure::Refuted,
                &why,
                worktree.as_ref(),
            );
        }
        Advance::Blocked(named) => match routed(fleet, issue, &named)? {
            Blocked::Park(parkable) => {
                let worktree = worktree_of(fleet, issue)?;
                park_claim(fleet, &claim, &parkable, worktree.as_ref())?;
                return Ok(Outcome::Done);
            }
            // Every blocker closed: the frontier holds the issue at the
            // stage that parked it.
            Blocked::Redispatch => {
                hand_on(fleet, &claim, None, End::HandedOn)?;
                return Ok(Outcome::HandedOn);
            }
            Blocked::Escalate(reason) => {
                let why = Handed::at(stage, BLOCKED_BRIEF, stated(&reason, said, None));
                let worktree = worktree_of(fleet, issue)?;
                return hand_off(
                    fleet,
                    found,
                    flows,
                    &claim,
                    Failure::Unblockable,
                    &why,
                    worktree.as_ref(),
                );
            }
        },
        Advance::Escalate(reason) => {
            Handed::new(&flows.brief(issue), stated(&reason, said, called.rounds.as_deref()))
        }
    };
    let worktree = worktree_of(fleet, issue)?;
    let pr = pr_of(fleet, issue)?;
    escalate_claim(fleet, found, &claim, &reason, pr, worktree.as_ref())?;
    Ok(Outcome::Done)
}

/// The tick's step for an unclaimed issue that calls for a human: claim it,
/// so that one fleet instance says so once, and escalate it for `reason` —
/// `brief`, in a few words — with `rounds`: every round of the rework a
/// breaker stopped, where one did.
pub fn hand_to_human(
    fleet: &mut Fleet,
    found: &mut Found,
    me: &Me,
    issue: u64,
    (brief, reason): (&str, &str),
    rounds: Option<&str>,
) -> Result<Outcome> {
    if take(&fleet.repo, &mut fleet.scan, me, issue, Take::Fresh)?["won"] != true {
        return Ok(Outcome::Lost);
    }
    let claim = mine(fleet, me, issue)?.ok_or_else(|| anyhow!("issue #{issue}: no claim"))?;
    let worktree = worktree_of(fleet, issue)?;
    let pr = pr_of(fleet, issue)?;
    let reason = Handed::new(brief, stated(reason, None, rounds));
    escalate_claim(fleet, found, &claim, &reason, pr, worktree.as_ref())?;
    Ok(Outcome::Done)
}

//! Keeping a train worker on the landing train.

use afkconvoy_core::prompt::{Enters, TrainFields, brief_pointer, train_pointer, train_prompt};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::train::{Built, TRAIN_WORKTREE, Tend, tend};
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, anyhow};

use crate::abandon::{StoppedOn, stopped_train};
use crate::claims::Me;
use crate::copies::Copies;
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::orca::{Source, Worktree};
use crate::team::keep;
use crate::train::Train;

/// Keeps the train worker on the train: starts one in the train's worktree —
/// cut first when this machine has none — or tells the one that stopped that
/// the train holds something it was not told about.
///
/// The train's worktree is the repo's, not a run's: it is found by its name,
/// kept when the fleet stops, and used again, with what its gate runs built,
/// by whichever fleet instance tends the train next — until it has outgrown
/// its build (`cut_train`).
///
/// A worker that stopped on the tip it was told about is `stopped_train`'s.
pub fn tend_train(fleet: &mut Fleet, me: &Me, agent: &Agent) -> Result<Outcome> {
    let train = Train::read(&fleet.repo, &fleet.config)?;
    if train.tip == train.base {
        return Ok(Outcome::Nothing);
    }
    let source = Source::of(&fleet.repo)?;
    let found = Worktree::of_train(&source)?;
    let (state, told) = match &found {
        Some(worktree) => (worktree.state(fleet.now)?, worktree.told()?),
        None => (WorkerState::Gone, None),
    };
    let decided = tend(state, told.as_ref().map(|told| told.tip.as_str()), &train.tip);
    let worktree = match (decided, found) {
        (Tend::Leave, _) => return Ok(Outcome::Nothing),
        (Tend::Stopped, Some(worktree)) => {
            let told_at = told.and_then(|told| told.at);
            let stopped = StoppedOn { train: &train, worktree: &worktree, state, told_at };
            return stopped_train(fleet, me, &stopped);
        }
        (_, Some(worktree)) => worktree,
        (_, None) => Worktree::cut(&source, TRAIN_WORKTREE, &train.base, None)?,
    };
    let path = worktree.path.display().to_string();
    // In a bound repo only a flow label dispatches: a defect's issue is
    // taken in to one of the team's workflows as they stand now.
    let workflows: Option<Vec<String>> = match &fleet.config.bound {
        Some(bound) => {
            let kept = keep(&fleet.gh, &Copies::here()?, &bound.team, &fleet.config.labels())?;
            Some(kept.team.workflows.iter().map(|workflow| workflow.name.clone()).collect())
        }
        None => None,
    };
    let intake_command = crate::own_command().fixed(" intake --train").own();
    let enters = match &workflows {
        Some(workflows) => Enters::Intake { command: &intake_command, workflows },
        None => Enters::Ready(&fleet.config.ready_label),
    };
    let prompt = train_prompt(&TrainFields {
        worktree: &path,
        base_branch: &fleet.config.base_branch,
        land_command: &crate::own_command().fixed(" land --train").own(),
        gate_alone_command: &crate::own_command().fixed(" gate").own(),
        enters,
    })
    .map_err(|e| anyhow!(e))?;
    let brief = worktree.write_brief(&prompt)?;
    let brief = brief.display().to_string();
    // The worker that stopped is told; with none to tell, one is started.
    let terminal = if decided == Tend::Tell { worktree.terminal()? } else { None };
    match terminal {
        Some(terminal) => worktree.tell(&terminal, &train_pointer(&brief))?,
        None => {
            // The train worker has no role, and no stage to name its
            // harness: it is a session of the default one.
            let launch = agent.command(fleet.harness())?;
            let terminal = worktree.start(launch.as_str(), fleet.now)?;
            worktree.await_ready(&terminal, agent.ready_timeout)?;
            worktree.tell(&terminal, &brief_pointer(&brief))?;
        }
    }
    // Last: a delivery that failed leaves a worker nobody told, and the next tick starts over.
    worktree.record_told(&train.tip, fleet.now)?;
    Ok(Outcome::Done)
}

/// Removes this machine's train's worktree and cuts it again at the base
/// branch's tip, once it has outgrown its build (`train::outgrown`) while the
/// train is idle: what its gate runs built is gone with it, and so is the
/// note of its fresh build — the next gate run there records it anew. A train
/// worker at work is left to it. What the worktree held is kept in
/// `Fleet::cut_train`, for the tick's line.
///
/// Nothing is lost: with nothing on the train a landing resets the worktree
/// to the tip anyway, and the next PR to join has a train worker started in
/// the worktree cut now.
pub fn cut_train(fleet: &mut Fleet, built: Built) -> Result<Outcome> {
    let source = Source::of(&fleet.repo)?;
    let Some(worktree) = Worktree::of_train(&source)? else { return Ok(Outcome::Nothing) };
    if worktree.state(fleet.now)? == WorkerState::Busy {
        return Ok(Outcome::Nothing);
    }
    let train = Train::read(&fleet.repo, &fleet.config)?;
    if train.tip != train.base {
        // a PR joined since the rebuild
        return Ok(Outcome::Nothing);
    }
    worktree.remove()?;
    Worktree::cut(&source, TRAIN_WORKTREE, &train.base, None)?;
    fleet.cut_train = Some(built);
    Ok(Outcome::Done)
}

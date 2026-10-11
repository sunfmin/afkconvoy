//! Keeping a train worker on the landing train.

use afkconvoy_core::prompt::{TrainFields, brief_pointer, train_pointer, train_prompt};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::train::{TRAIN_WORKTREE, Tend, tend};
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, anyhow};

use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::orca::{Source, Worktree};
use crate::train::Train;

/// Keeps the train worker on the train: starts one in the train's worktree —
/// cut first when this machine has none — or tells the one that stopped that
/// the train holds something it was not told about.
///
/// The train's worktree is the repo's, not a run's: it is found by its name,
/// kept when the fleet stops, and used again, with what its gate runs built,
/// by whichever fleet instance tends the train next.
pub fn tend_train(fleet: &Fleet, agent: &Agent) -> Result<Outcome> {
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
    let decided = tend(state, told.as_deref(), &train.tip);
    if decided == Tend::Leave {
        return Ok(Outcome::Nothing);
    }
    let worktree = match found {
        Some(worktree) => worktree,
        None => Worktree::cut(&source, TRAIN_WORKTREE, &train.base, None)?,
    };
    let path = worktree.path.display().to_string();
    let prompt = train_prompt(&TrainFields {
        worktree: &path,
        base_branch: &fleet.config.base_branch,
        land_command: &crate::own_command("land --train"),
    })
    .map_err(|e| anyhow!(e))?;
    let brief = worktree.write_brief(&prompt)?;
    let brief = brief.display().to_string();
    // The worker that stopped is told; with none to tell, one is started.
    let terminal = if decided == Tend::Tell { worktree.terminal()? } else { None };
    match terminal {
        Some(terminal) => worktree.tell(&terminal, &train_pointer(&brief))?,
        None => {
            let terminal = worktree.start(&agent.command)?;
            worktree.await_ready(&terminal, agent.ready_timeout)?;
            worktree.tell(&terminal, &brief_pointer(&brief))?;
        }
    }
    // Last: a delivery that failed leaves a worker nobody told, and the next tick starts over.
    worktree.record_told(&train.tip)?;
    Ok(Outcome::Done)
}

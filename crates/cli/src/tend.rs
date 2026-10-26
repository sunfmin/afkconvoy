//! Keeping a train worker on the landing train.

use afkconvoy_core::prompt::{Enters, TrainFields, brief_pointer, train_pointer, train_prompt};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::train::{Built, TRAIN_WORKTREE, Tend, tend};
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, anyhow};
use serde_json::json;

use crate::abandon::{StoppedOn, stopped_train};
use crate::claims::Me;
use crate::copies::Copies;
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::orca::{Source, Worktree};
use crate::team::keep;
use crate::train::Train;

/// Keeps the train worker on the train: starts one in the train's worktree —
/// cut first when this machine has none — or tells a healthy worker on the
/// first eligible harness that the train holds something new. A failed or
/// outdated worker is replaced, as is one held past the health bound.
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
    let new_tip = told.as_ref().is_none_or(|told| told.tip != train.tip);
    let mut replacement = None;
    if let Some(worktree) = &found {
        if crate::health::train_hold_expired(worktree, fleet.now)? {
            replacement = Some(format!(
                "worker health held the train worker for {} seconds",
                crate::health::RECOVERY_SECONDS
            ));
        } else if new_tip && state != WorkerState::Gone {
            let report = worktree.agent_report()?;
            if report["state"] == "done" && report["outcome"] == "failure" {
                replacement = Some("orca reports that the train worker's agent failed".into());
            } else if worktree.harness()? != Some(fleet.first()?.harness) {
                replacement =
                    Some("the train worker is not on the host kit's first eligible harness".into());
            }
        }
        if replacement.is_none()
            && state != WorkerState::Gone
            && (crate::health::has_signal(worktree)?
                || (crate::health::holding(worktree) && !new_tip))
        {
            return Ok(Outcome::Nothing);
        }
    }
    let decided = if replacement.is_some() {
        Tend::Start
    } else {
        tend(state, told.as_ref().map(|told| told.tip.as_str()), &train.tip)
    };
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
            let kept = keep(&fleet.gh, &Copies::of_host()?, &bound.team, &fleet.config.labels())?;
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
        lock_command: &crate::own_command().fixed(" lock").own(),
        enters,
        tracker: &fleet.gh.tracker,
    })
    .map_err(|e| anyhow!(e))?;
    // Validate the successor before ending the old writer. End it before
    // rewriting its brief, with its worktree and the train kept intact.
    let start = if decided == Tend::Tell { None } else { Some(fleet.first()?) };
    let previous = worktree.harness()?;
    if let Some(first) = &start {
        crate::role::startable(&agent.launches, first.harness, "train worker")?;
        worktree.stop_for_continuation()?;
    }
    let brief = worktree.write_brief(&prompt)?;
    let brief = brief.display().to_string();
    // The worker that stopped is told; with none to tell, one is started.
    let terminal = if decided == Tend::Tell { worktree.terminal()? } else { None };
    match terminal {
        Some(terminal) => worktree.tell(&terminal, &train_pointer(&brief))?,
        None => {
            // The train worker is in no role, and nothing casts it: it runs
            // the host kit's first.
            let first = start.unwrap_or(fleet.first()?);
            let launch = agent.command(first.harness)?;
            let terminal = agent.start_worker(
                &worktree,
                (first.harness, first.model.as_deref()),
                &first.command(launch.as_str()),
                false,
                None,
                fleet.now,
            )?;
            worktree.tell(&terminal, &brief_pointer(&brief))?;
        }
    }
    // Last: a delivery that failed leaves a worker nobody told, and the next tick starts over.
    worktree.record_told(&train.tip, fleet.now)?;
    if let Some(reason) = replacement {
        crate::log::Log::of(&fleet.repo)?.append(&crate::log::Entry {
            at: fleet.now,
            instance: me.instance.as_str().into(),
            host: me.host.clone(),
            what: crate::log::What::Command {
                command: "train_worker".into(),
                on: None,
                by: reason,
                outcome: json!({"action": "replaced", "from": previous,
                    "to": worktree.harness()?, "worktree": worktree.path, "tip": train.tip})
                .to_string(),
                failed: false,
            },
        })?;
    }
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

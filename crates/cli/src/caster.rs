//! What each open issue's worker runs: its one `model:` label of the selected
//! harness, or that harness's user default. Automatic casting is gone, so
//! nothing is held and no caster is started.

use afkconvoy_core::casting::{ModelRun, ModelStage, ModelStatus, Why};
use afkconvoy_core::role::workflow_of;
use afkconvoy_core::selection::Unsafe;
use afkconvoy_core::team::{Performer, Role, Team};
use afkconvoy_core::worker::WorkerState;

use crate::fleet::Fleet;
use crate::orca::{Source, Worktree};

/// Each stage's role, by the stage's name and its role's: one `worker` of no
/// role in an unbound repo.
fn roles<'t>(team: Option<&'t Team>, labels: &[String]) -> Vec<(String, Option<&'t Role>)> {
    let Some(team) = team else { return vec![("worker".into(), None)] };
    let Ok(workflow) = workflow_of(team, labels) else { return vec![] };
    workflow
        .stages()
        .iter()
        .filter_map(|stage| {
            let Performer::Role(work) = &stage.by else { return None };
            let role = team.role_of(work);
            Some((format!("{} ({})", stage.name, role.name), Some(role)))
        })
        .collect()
}

/// What a stage's worker runs, as its worker selection says: the one
/// dispatch makes, from the same facts.
fn stage_of(
    fleet: &Fleet,
    stage: String,
    role: Option<&Role>,
    labels: &[String],
    number: u64,
) -> ModelStage {
    let unsafe_stage = |why: String| ModelStage {
        stage: stage.clone(),
        harness: None,
        model: None,
        followed: false,
        why,
        skipped: vec![],
    };
    let (cast, model, refused) = match fleet.select(role, labels, Some(number)) {
        Ok(Ok(chosen)) => (chosen.cast, chosen.model, None),
        Ok(Err(Unsafe::Refused { cast, why })) => (cast, None, Some(why)),
        Ok(Err(why)) => return unsafe_stage(why.said().to_string()),
        Err(error) => return unsafe_stage(format!("{error:#}")),
    };
    let followed = cast.why == Why::ModelLabel;
    let mut why = match &cast.notice {
        Some(notice) => format!("{}: {notice}", cast.why.said()),
        None => cast.why.said().to_string(),
    };
    if let Some(refused) = refused {
        why = format!("{why}, which refuses the start: {refused}");
    }
    ModelStage { stage, harness: Some(cast.harness), model, followed, why, skipped: vec![] }
}

/// What a worker of each open issue runs. `needed` is never set: automatic
/// casting is gone. A stage says whether its model label was followed.
pub fn status(
    fleet: &Fleet,
    open: &crate::gh::OpenIssues,
    team: Option<&Team>,
) -> Vec<ModelStatus> {
    let source = Source::of(&fleet.repo).ok();
    let mut statuses: Vec<ModelStatus> = open
        .rows
        .iter()
        .map(|issue| {
            let labels = &issue.labels;
            let stages: Vec<ModelStage> = roles(team, labels)
                .into_iter()
                .map(|(stage, role)| stage_of(fleet, stage, role, labels, issue.number))
                .collect();
            let named: Vec<_> = stages.iter().filter(|stage| stage.harness.is_some()).collect();
            let why = if named.is_empty() {
                stages
                    .first()
                    .map(|stage| stage.why.clone())
                    .unwrap_or_else(|| "no eligible harness".to_string())
            } else if named.iter().all(|stage| stage.followed) {
                Why::ModelLabel.said().to_string()
            } else if named.iter().all(|stage| !stage.followed) {
                Why::UserDefault.said().to_string()
            } else {
                "its model label for a stage that follows one, and the user's default for the rest"
                    .to_string()
            };
            let running = source
                .as_ref()
                .filter(|_| fleet.scan.claim_of(issue.number).is_some())
                .and_then(|source| Worktree::of_issue(source, issue.number).ok().flatten())
                .filter(|worktree| {
                    worktree.state(fleet.now).is_ok_and(|state| state != WorkerState::Gone)
                })
                .and_then(|worktree| {
                    let harness = worktree.harness().ok().flatten()?;
                    let model = std::fs::read_to_string(worktree.note("convoy-worker-model").ok()?)
                        .ok()
                        .map(|text| text.trim().to_string())
                        .filter(|text| !text.is_empty());
                    Some(ModelRun { harness, model })
                });
            ModelStatus { number: issue.number, needed: false, why, stages, running }
        })
        .collect();
    statuses.sort_by_key(|status| status.number);
    statuses
}

/// What `afkconvoy cast --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Do not. `afkconvoy cast` fails and says automatic casting is gone. So does `afkconvoy kit --cast`. Nothing is chosen, no prompt is sent, and no label is added or removed.

## Who runs it

The operator, in a checkout. A tick never runs it, and no issue waits.

## What it reads and writes

Nothing. It fails before it reads an issue or writes a label, a comment, or the host kit.

## What a worker runs

The host kit's order and the role's range choose the harness. `switch --harness` reorders the kit. A `harness:` label never selects a harness. `switch --model` moves a model first inside its harness and does not choose the model a worker starts on.

A worker's model is the issue's one `model:` label when that label names exactly one model of the harness already selected, matched by its id or any name, whatever the case, or with the provider prefix left off where that leaves one match. The worker starts on that model's id. The label is left as written.

Otherwise the worker runs that harness's user default, and the label stays: no `model:` label, a name that matches nothing the harness holds, a name that matches two, two `model:` labels, or a name that matches a model the role's range does not hold. A label that is not followed is said once on the issue, with the nearest names the harness holds, or the two ids. The worker starts on the user default in that same dispatch. Nothing is taken off the issue.

Claude Code, pi, Kimi and Grok start with no model flag. Kimi then uses `default_model` from the user's Kimi config. DSH starts with no model in the worker environment, so the profile's model runs. Codex reads `model` from the user's Codex config when that value is a nonempty string the host kit lists for Codex and passes it; with no such key, Codex starts with no model flag. A Codex `model` that is not a nonempty string, or that the kit does not list, refuses the start. A Codex model named that way and outside the role's range refuses too. An unnamed default cannot be checked against a range, and the worker starts.

A watch, a cut and an inspection have no model label and use the user default. A landing worker follows its issue's label by the same rule. A range that holds one model does not name it.

## What it answers

An error: automatic casting is gone. `plan` and `doctor` say, under `casting`, whether a model label was followed (`model_label`) or the user default was used (`user_default`). `needed` is false. `skipped` is empty. They report no caster and no host cast flag. A launch record that already says `casting` is still read as the issue's model label.

## What trips you up

An old host-kit `cast` key and a roster `cast` key do not change a start and do not fail `check` or `kit`. A label that is not followed is said once and left in place. Switch alone leaves healthy workers on their current model; --all explicitly continues them."#;

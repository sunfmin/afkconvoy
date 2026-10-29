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

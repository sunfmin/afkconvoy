//! What each open issue's worker runs: its one `model:` label of the selected
//! harness, or that harness's user default. Automatic casting is gone, so
//! nothing is held and no caster is started.

use afkconvoy_core::casting::{self, Held, ModelRun, ModelStage, ModelStatus, Why};
use afkconvoy_core::role::workflow_of;
use afkconvoy_core::team::{Performer, Team};
use afkconvoy_core::worker::WorkerState;

use crate::fleet::Fleet;
use crate::orca::{Source, Worktree};

/// Each stage's eligible range in kit order. Permissions are cut against the
/// whole kit before temporary health exclusions, exactly as at dispatch.
fn ranges(
    fleet: &Fleet,
    team: Option<&Team>,
    labels: &[String],
    number: u64,
) -> Vec<(String, Vec<Held>)> {
    let Some(team) = team else {
        return vec![(
            "worker".into(),
            fleet.runnable_for(Some(number)).map(|kit| kit.range()).unwrap_or_default(),
        )];
    };
    let Ok(workflow) = workflow_of(team, labels) else { return vec![] };
    workflow
        .stages()
        .iter()
        .filter_map(|stage| {
            let Performer::Role(work) = &stage.by else { return None };
            let role = team.role_of(work);
            Some((
                format!("{} ({})", stage.name, role.name),
                fleet.range_in(role, Some(number)).unwrap_or_default(),
            ))
        })
        .collect()
}

/// What a worker of each open issue runs. `needed` is never set: automatic
/// casting is gone. A stage says whether its model label was followed.
pub fn status(
    fleet: &Fleet,
    open: &crate::gh::OpenIssues,
    team: Option<&Team>,
) -> Vec<ModelStatus> {
    let whole = crate::kit::load().map(|kit| kit.range()).unwrap_or_default();
    let source = Source::of(&fleet.repo).ok();
    let mut statuses: Vec<ModelStatus> = open
        .rows
        .iter()
        .map(|issue| {
            let labels = &issue.labels;
            let ranges = ranges(fleet, team, labels, issue.number);
            let stages: Vec<ModelStage> = ranges
                .iter()
                .map(|(stage, range)| match casting::preferred(labels, range, &whole) {
                    Ok(cast) => {
                        let followed = cast.why == Why::ModelLabel;
                        let why = match &cast.notice {
                            Some(notice) => format!("{}: {notice}", cast.why.said()),
                            None => cast.why.said().to_string(),
                        };
                        ModelStage {
                            stage: stage.clone(),
                            harness: Some(cast.harness),
                            model: cast.model,
                            followed,
                            why,
                            skipped: vec![],
                        }
                    }
                    Err(why) => ModelStage {
                        stage: stage.clone(),
                        harness: None,
                        model: None,
                        followed: false,
                        why,
                        skipped: vec![],
                    },
                })
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

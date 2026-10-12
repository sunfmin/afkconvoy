//! Stage records on GitHub (docs/adr/0010): `verdict`, which writes the one
//! record a stage leaves; `show --issue`, which reads an issue's position off
//! its records; and the tick's step that brings its `stage:` label to that
//! position. What a record is and what the records come to is the core's
//! (`afkconvoy_core::stage_record`).

use afkconvoy_core::plan::Issue;
use afkconvoy_core::position::Declared;
use afkconvoy_core::role::workflow_of;
use afkconvoy_core::stage_record::{History, StageRecord, history, label_moves, stage_records};
use afkconvoy_core::team::{Team, Workflow};
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::claims::Scan;
use crate::copies::Copies;
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::team::{Kept, keep};
use crate::wake::wake;
use crate::workflow::position;

/// What a stage declares as it ends.
pub struct Declaring {
    pub issue: u64,
    pub stage: String,
    pub declared: Declared,
    pub blocked_by: Vec<u64>,
    /// What the stage produced.
    pub text: String,
    /// The commits of the role's repo and of the team's it ran under.
    pub role_commit: String,
    pub team_commit: String,
}

/// `afkconvoy verdict`: write the stage record of the stage `declaring` ends,
/// under the claim the issue is held by, and wake the fleet. A stage that has
/// a record under that claim already gets no second one.
///
///   {"issue", "stage", "verdict", "claim", "declared": true, "woke"}
///   {…, "declared": false, "standing": <the verdict already on record>, "woke"}
///
/// Whether the record is a verdict is not decided here: the fleet reads it
/// against the stage it dispatched under that claim.
pub fn verdict(repo: &Repo, declaring: Declaring) -> Result<Value> {
    let Declaring { issue, stage, declared, blocked_by, text, role_commit, team_commit } =
        declaring;
    if stage.trim().is_empty() {
        bail!("--stage names no stage");
    }
    if text.trim().is_empty() {
        bail!("a stage record carries the text its stage produced: there is none");
    }
    if !blocked_by.is_empty() && declared != Declared::Blocked {
        bail!("--blocked-by goes with `blocked` only, not with `{}`", declared.word());
    }
    let scan = Scan::read(repo)?;
    let Some(claim) = scan.claim_of(issue) else {
        bail!(
            "issue #{issue} is not claimed: a verdict is declared under the claim its stage was \
             dispatched under"
        );
    };
    let record = StageRecord {
        stage,
        declared,
        claim: claim.sha.clone(),
        role_commit,
        team_commit,
        blocked_by,
        text,
    };
    let gh = GitHub { dir: repo.dir.clone() };
    let comments = gh.comments(issue)?;
    let standing = stage_records(comments.iter().map(|comment| comment.body.as_str()))
        .into_iter()
        .map(|(_, standing)| standing)
        .find(|standing| standing.stage == record.stage && standing.claim == record.claim);
    if standing.is_none() {
        gh.comment(issue, &record.comment())?;
    }
    let mut answer = json!({"issue": issue, "stage": record.stage,
        "verdict": record.declared.word(), "claim": record.claim,
        "declared": standing.is_none(), "woke": wake(repo)?["woke"]});
    if let Some(standing) = standing {
        answer["standing"] = json!(standing.declared.word());
    }
    Ok(answer)
}

/// An issue's stage records, and what they come to in the workflow it follows.
struct Read<'t> {
    workflow: &'t Workflow,
    /// Each record with the URL of its comment.
    records: Vec<(StageRecord, String)>,
    history: History<'t>,
}

/// Reads the stage records of issue `number`, which follows `workflow`.
fn read<'t>(gh: &GitHub, workflow: &'t Workflow, number: u64) -> Result<Read<'t>> {
    let comments = gh.comments(number)?;
    let found = stage_records(comments.iter().map(|comment| comment.body.as_str()));
    let (urls, records): (Vec<String>, Vec<StageRecord>) = found
        .into_iter()
        .map(|(at, record)| (comments.get(at).map(|c| c.url.clone()).unwrap_or_default(), record))
        .unzip();
    let history = history(workflow, &records);
    Ok(Read { workflow, records: records.into_iter().zip(urls).collect(), history })
}

/// `afkconvoy show --issue`: where issue `number` is in the workflow of
/// `team` its flow label names, read off its stage records.
///
///   {"issue", "workflow", "position", "follows", "rework": {"had", "left"},
///    "consecutive_rework": {<stage>: {"had", "left"}}, "records": [{"stage",
///    "verdict", "reads", "claim", "role_commit", "team_commit", "url"}]}
///
/// `follows` is the position a `pass` declared now leads to; null where none
/// can be declared.
pub fn show_issue(gh: &GitHub, team: &Team, number: u64) -> Result<Value> {
    let issue = gh.issue(number)?;
    let workflow = workflow_of(team, &issue.labels)
        .map_err(|why| anyhow!("issue #{number} has no position: {why}"))?;
    let Read { workflow, records, history } = read(gh, workflow, number)?;
    let place = &history.place;
    let follows = place
        .takes(Declared::Pass)
        .then(|| position(workflow, place.clone().after(Declared::Pass).position));
    let mut in_a_row = Map::new();
    for stage in workflow.stages().iter().filter(|stage| stage.reject().is_some()) {
        let had = place.consecutive_rework(stage);
        let left = place.consecutive_rework_left(stage);
        in_a_row.insert(stage.name.clone(), json!({"had": had, "left": left}));
    }
    let records: Vec<Value> = records
        .iter()
        .zip(&history.readings)
        .map(|((record, url), reading)| {
            json!({"stage": record.stage, "verdict": record.declared.word(),
                "reads": reading.word(), "claim": record.claim,
                "role_commit": record.role_commit, "team_commit": record.team_commit,
                "url": url})
        })
        .collect();
    Ok(json!({
        "issue": number,
        "workflow": workflow.name,
        "position": position(workflow, place.position),
        "follows": follows,
        "rework": {"had": place.rework(), "left": place.rework_left()},
        "consecutive_rework": in_a_row,
        "records": records,
    }))
}

/// One tick's `stage:` labels: the issues that follow a workflow, as the
/// rebuild read them, and the team their positions are derived under — read
/// at the first issue that needs it, once a tick.
pub struct Labels<'s> {
    flowing: &'s [Issue],
    team: Option<Result<Kept, String>>,
}

impl<'s> Labels<'s> {
    pub fn of(flowing: &'s [Issue]) -> Labels<'s> {
        Labels { flowing, team: None }
    }

    /// Brings the `stage:` label of issue `number` to the position its stage
    /// records give it → whether a label was moved. The labels it carries
    /// are read only to say what to write. An issue whose labels name no one
    /// workflow of the team has no position, and is left as it is.
    pub fn bring(&mut self, fleet: &Fleet, number: u64) -> Result<Outcome> {
        let issue = self.flowing.iter().find(|issue| issue.number == number);
        let (Some(issue), Some(bound)) = (issue, &fleet.config.bound) else {
            return Ok(Outcome::Nothing);
        };
        let kept = self.team.get_or_insert_with(|| {
            Copies::here()
                .and_then(|copies| keep(&fleet.gh, &copies, &bound.team))
                .map_err(|error| format!("{error:#}"))
        });
        let kept = kept.as_ref().map_err(|why| anyhow!("{why}"))?;
        let Ok(workflow) = workflow_of(&kept.team, &issue.labels) else {
            return Ok(Outcome::Nothing);
        };
        let position = read(&fleet.gh, workflow, issue.number)?.history.place.position;
        let moves = label_moves(&issue.labels, position);
        if let Some(label) = &moves.add {
            fleet.gh.add_label(issue.number, label)?;
        }
        for label in &moves.remove {
            fleet.gh.remove_label(issue.number, label)?;
        }
        let moved = moves.add.is_some() || !moves.remove.is_empty();
        Ok(if moved { Outcome::Done } else { Outcome::Nothing })
    }
}

//! A human steers an issue nobody holds (docs/adr/0010, docs/adr/0011): they
//! declare the verdict of the stage it is at, rule on it where it was handed
//! to them, or send it to a stage. Each leaves one stage record that says a
//! human left it, and moves nothing else but the labels that hand the issue
//! back to the fleet: where the issue then is, the next tick reads off its
//! records.

use afkconvoy_core::position::{Declared, Position};
use afkconvoy_core::role::{followed, workflow_of};
use afkconvoy_core::stage_record::{Author, StageRecord, Stated, history, stage_records};
use afkconvoy_core::team::{Team, Workflow};
use afkconvoy_core::transition::Relabel;
use afkconvoy_core::waiting::{Waits, back};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::Scan;
use crate::gh::GitHub;
use crate::git::Repo;

/// An issue nobody holds, as a human's command reads it.
struct Unheld {
    gh: GitHub,
    number: u64,
    labels: Vec<String>,
    /// The bodies of its comments, oldest first.
    bodies: Vec<String>,
}

impl Unheld {
    /// Reads issue `number`. One a fleet instance holds is refused: its
    /// stage is its worker's, and a record beside the one the claim was
    /// dispatched for would leave the two at odds. `instead` says what a
    /// human does about a held issue.
    fn read(repo: &Repo, number: u64, instead: &str) -> Result<Unheld> {
        if Scan::read(repo)?.claim_of(number).is_some() {
            bail!(
                "issue #{number} is claimed: a fleet instance holds it, and its stage is its \
                 worker's. {instead}; nothing was written"
            );
        }
        let gh = GitHub::of(repo)?;
        let labels = gh.issue(number)?.labels;
        let bodies = gh.comments(number)?.into_iter().map(|comment| comment.body).collect();
        Ok(Unheld { gh, number, labels, bodies })
    }

    fn bodies(&self) -> impl Iterator<Item = &str> {
        self.bodies.iter().map(String::as_str)
    }

    fn records(&self) -> Vec<StageRecord> {
        stage_records(self.bodies()).into_iter().map(|(_, record)| record).collect()
    }

    /// Finds the workflow this issue follows. An escalation from before it
    /// entered one needs intake first, with the same answer every view gives.
    fn workflow<'t>(&self, team: &'t Team) -> Result<&'t Workflow> {
        followed(team, &self.labels, self.bodies()).map_err(|why| {
            if self.labels.contains(&team.escalate_label) && team.flows(&self.labels).is_empty() {
                let waits = Waits::Escalated(back(team, None, &self.records()));
                anyhow!(
                    "{why}; {}. Run: {}",
                    waits.why(),
                    waits.answer(self.number, &self.gh.tracker)
                )
            } else {
                anyhow!(why)
            }
        })
    }

    /// Writes the record a human leaves about `stage`, with what they said.
    fn leave(&self, stage: &str, stated: Stated, text: String) -> Result<()> {
        let record = StageRecord {
            stage: stage.to_string(),
            stated,
            by: Author::Human,
            blocked_by: vec![],
            text,
        };
        self.gh.comment(self.number, &record.comment())
    }

    /// The labels that hand the issue back to the fleet as one that follows
    /// `workflow`: nothing to swap unless it was handed to a human.
    fn back(&self, team: &Team, workflow: &Workflow) -> Relabel {
        Relabel::back(&self.labels, workflow.flow_label(), &team.escalate_label)
    }

    /// Hands the issue back to the fleet as one that follows `workflow` →
    /// whether a label was moved. The record is written first: an issue
    /// that carries its flow label is the fleet's to act on at once.
    fn hand_back(&self, team: &Team, workflow: &Workflow) -> Result<bool> {
        let back = self.back(team, workflow);
        if let Some(label) = &back.add {
            self.gh.add_label(self.number, label)?;
        }
        for label in &back.remove {
            self.gh.remove_label(self.number, label)?;
        }
        Ok(!back.done())
    }
}

/// What a human says with a record: it is never empty.
fn said(text: String, what: &str) -> Result<String> {
    if text.trim().is_empty() {
        bail!("a stage record carries {what}: there is none");
    }
    Ok(text)
}

/// `afkconvoy rule`: answer issue `number`, handed to a human, with the
/// ruling `text`. The stage it stopped at runs again under it, with both
/// rework counts started over, and the escalate label is swapped back for the
/// flow label.
///
///   {"issue", "workflow", "stage", "ruled": true, "relabelled", "woke"}
pub fn rule(repo: &Repo, team: &Team, number: u64, text: String) -> Result<Value> {
    let text = said(text, "the ruling")?;
    let instead = "Rule once its escalation is done and its claim released";
    let issue = Unheld::read(repo, number, instead)?;
    let workflow =
        issue.workflow(team).map_err(|why| anyhow!("issue #{number} cannot be ruled on: {why}"))?;
    let position = history(workflow, issue.records()).place.position;
    let stage = if position.landing() {
        afkconvoy_core::team::LAND
    } else if let Some(stage) = position.stage() {
        &stage.name
    } else {
        bail!(
            "issue #{number} cannot be ruled on: it has left the stages of its workflow, so \
             none runs again under a ruling. `afkconvoy send` puts it at one"
        );
    };
    // Handed over means the records say so, or the labels do: an issue whose
    // worker went silent was escalated with no record of it.
    let handed_over = matches!(position, Position::Escalating(..) | Position::LandingEscalating)
        || !issue.back(team, workflow).done();
    if !handed_over {
        bail!(
            "issue #{number} is not escalated: it is at the stage `{}` and carries its flow \
             label; nothing was written",
            stage
        );
    }
    issue.leave(stage, Stated::Declared(Declared::Ruling), text)?;
    let relabelled = issue.hand_back(team, workflow)?;
    Ok(json!({"issue": number, "workflow": workflow.name, "stage": stage, "ruled": true,
        "relabelled": relabelled}))
}

/// `afkconvoy send`: put issue `number` at the stage `stage` of its workflow,
/// wherever it was, with both rework counts started over. `text` is why. An
/// escalated issue is handed back to the fleet as by a ruling.
///
///   {"issue", "workflow", "stage", "sent": true, "relabelled", "woke"}
pub fn send(repo: &Repo, team: &Team, number: u64, stage: &str, text: String) -> Result<Value> {
    let text = said(text, "why the issue is sent")?;
    let instead = "Send it once its stage has declared, or `afkconvoy escalate` it first";
    let issue = Unheld::read(repo, number, instead)?;
    let workflow = issue
        .workflow(team)
        .map_err(|why| anyhow!("issue #{number} cannot be sent to a stage: {why}"))?;
    if !workflow.stages().iter().any(|has| has.name == stage) {
        let has: Vec<&str> = workflow.stages().iter().map(|stage| stage.name.as_str()).collect();
        bail!(
            "the workflow `{}` issue #{number} follows has no stage `{stage}`; it has: {}",
            workflow.name,
            has.join(", ")
        );
    }
    issue.leave(stage, Stated::Sent, text)?;
    let relabelled = issue.hand_back(team, workflow)?;
    Ok(json!({"issue": number, "workflow": workflow.name, "stage": stage, "sent": true,
        "relabelled": relabelled}))
}

/// A human's `afkconvoy verdict`: declare `declared` for the stage `stage`
/// issue `number` is at — a `by: human` stage, which waits for exactly this,
/// or any other nobody holds. Unlike a worker's, it is checked before it is
/// written: a human reads the refusal, and nobody would ask after the stage.
///
///   {"issue", "stage", "verdict", "by": "human", "declared": true, "woke"}
pub fn declare(
    repo: &Repo,
    team: &Team,
    number: u64,
    stage: &str,
    declared: Declared,
    text: String,
) -> Result<Value> {
    let instead = "A worker declares with the commits of its role and of its team \
                   (`--role-commit` and `--team-commit`, or its environment); a human declares \
                   on an issue nobody holds";
    let issue = Unheld::read(repo, number, instead)?;
    let workflow = workflow_of(team, &issue.labels)
        .map_err(|why| anyhow!("issue #{number} has no stage to declare for: {why}"))?;
    let place = history(workflow, issue.records()).place;
    let at = place.position.stage().map(|stage| stage.name.as_str());
    if at != Some(stage) {
        let is = at.map_or("at no stage: it has left them".to_string(), |at| {
            format!("at the stage `{at}`")
        });
        bail!("issue #{number} is {is}, not at `{stage}`; nothing was written");
    }
    if !place.takes(declared) {
        let why = match place.position {
            Position::Escalating(..) => "the issue is escalated: `afkconvoy rule` answers that",
            Position::Consult(..) => "the issue is with the stage's consult",
            _ => "its workflow gives the stage no such verdict",
        };
        bail!(
            "`{}` cannot be declared for the stage `{stage}` of issue #{number}: {why}; nothing \
             was written",
            declared.word()
        );
    }
    issue.leave(stage, Stated::Declared(declared), text)?;
    Ok(json!({"issue": number, "stage": stage, "verdict": declared.word(), "by": "human",
        "declared": true}))
}

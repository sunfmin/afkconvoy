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
            blocked_by: Default::default(),
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

/// What `afkconvoy send --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const SEND_HELP: &str = r#"## When to run it

To put an issue of a bound repo at a stage of your choosing, whatever its records say: back to planning after a review that missed the point, past a stage you have done by hand, or out of an escalation to another stage than the one that asked.

```
afkconvoy send --issue 7 --stage plan --text "The approach changed: see #12."
```

The issue is then at that stage, to be performed, with both rework counts started over. To answer an escalated issue where it stopped, `afkconvoy rule` is the command.

## Who runs it

The operator, or whichever human steers the issue: in a checkout of the bound repo — the host's watch among them, whatever harness its environment names, as with `afkconvoy rule`. Or a consult answering in a human's place, under its claim, as with `afkconvoy rule`; its send starts no count over. No other worker runs it: where an issue goes is its workflow's to say, or a human's.

## What it reads and writes

It reads the team the config names, the claims on the remote, and the issue's labels and comments. The workflow is the one the issue's flow label names; for an escalated issue, the one its escalation's comment says it left.

It writes one comment on the issue — a stage record that names the stage, says a human sent the issue there and carries the text — and wakes the fleet. An escalated issue also gets its flow label back and loses the team's `escalate_label`, after the record is written. Nothing else moves: the next tick reads the issue's position off its records, starts a worker on the stage if a role performs it, and waits for a human's verdict if the stage is `by: human`.

The text is `--text`, or the contents of `--text-file`: one of the two, not empty. The worker started on the stage is shown it in its brief.

## What it answers

{"issue", "workflow", "stage", "sent": true, "relabelled", "woke"}

- `relabelled`: whether a label was moved: true for an issue that was escalated
- `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers

Errors, as {"error": …} with a non-zero exit, with nothing written: the workflow "has no stage" of that name — the error lists the ones it has; the issue "is claimed" — a worker is at a stage of it; it "cannot be sent to a stage" — no flow label and no escalation says which workflow it follows; no text, or an empty one; the repo names no team; and every error of `afkconvoy check`.

## What trips you up

- It is refused while the issue is claimed: a claim is dispatched to one stage, and its worker would go on performing it. Wait for the stage to declare, or hand the claim to a human with `afkconvoy escalate` first.
- It reopens nothing and closes nothing. An issue sent back from an ended workflow must be open to be worked.
- The stage is named as its workflow's file names it. `afkconvoy show` draws them.
- An escalation with no position cannot be sent to a stage before it follows a workflow. The error gives the same line as `afkconvoy doctor` to put a flow label on it (`afkconvoy help humans`).
- Run twice, it leaves two records; the issue is at the stage either way."#;

/// What `afkconvoy rule --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const RULE_HELP: &str = r#"## When to run it

When an issue of a bound repo was escalated and you have decided what it asked: the issue carries the team's escalate label in place of its flow label, and the comment above the label says why. Answer it with the ruling:

```
afkconvoy rule 7 --text "Keep the v1 schema; migrate in a later issue."
```

The issue's number is an argument, or `--issue 7`: one of the two. The comment of the escalation ends with this line, the number filled in, and `afkconvoy doctor` lists it for every issue that waits (`waiting_on_human`).

The stage the issue stopped at runs again under the ruling, and both rework counts start over from it. `afkconvoy show --issue 7` says which stage that is before you rule. To put the issue at another stage instead, see `afkconvoy send --help`.

An issue escalated at Landing returns to Landing under the ruling: its existing PR is kept and no stage runs again. Its answer has `stage: land`.

Or as a consult: the issue was at a stage that names one (`escalate: { to, do }` in its workflow), and would have gone to a human — the stage declared `escalate`, a breaker tripped, it failed, or its workflow changed — so the fleet started you in that role to answer it first. The stage comes from your environment:

```
afkconvoy rule --issue 7 --text-file /tmp/ruling.md
```

The stage runs again under your ruling. What is not yours to rule on you let through to a human instead, with `afkconvoy verdict escalate` — your brief gives that line too.

## Who runs it

The operator, or whichever human the issue was handed to: in a checkout of the bound repo, with no commit named and none in the environment. The host's watch, and the asking session beside it, are the operator: the harness and model their environment names (AFKCONVOY_HARNESS, AFKCONVOY_MODEL) are not read, and their ruling is a human's.

Or a consult, in the issue's worktree: the fleet put the commits of its role and of its team in its environment, and those are what make the ruling a consult's; the harness it is a session of is there too, and its record names it. No other worker runs it: a stage does not rule on itself.

## What it reads and writes

It reads the team the config names, the claims on the remote, and the issue's labels and comments. The workflow is the one the issue's flow label names; with the label swapped out, the one its escalation's comment says it left.

It writes one comment on the issue — a stage record that carries the ruling, says a human left it and names the stage — then puts the flow label back on and takes the team's `escalate_label` off, and wakes the fleet. The record is written first: once the flow label is back the issue is the fleet's again, and its next tick starts a worker on the stage, with the ruling in its brief.

A consult's ruling is one comment too, and nothing else: a stage record that says the stage's consult left it, under the claim the consult holds on the issue, with the two commits. `--stage` is AFKCONVOY_STAGE where not given. It moves no label — the issue never left the fleet — and the next tick ends the consult's claim; the one after starts the stage that asked, with the ruling in its brief. It is checked no more than a worker's verdict is.

The ruling is `--text`, or the contents of `--text-file`: one of the two, not empty.

## What it answers

{"issue", "workflow", "stage", "ruled": true, "relabelled", "woke"} {"issue", "stage", "verdict": "ruling", "claim", "declared", "woke"} — a consult's, as `afkconvoy verdict` answers a worker

- `stage`: the stage that runs again under the ruling
- `relabelled`: whether a label was moved; false when the issue already carried its flow label and no escalate label
- `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers

Errors, as {"error": …} with a non-zero exit, with nothing written: the issue "is claimed" — its escalation is still under way, or a worker is at it; it "is not escalated" — its records put it at a stage and it carries its flow label; it "cannot be ruled on" — it has left the stages of its workflow, or no flow label and no escalation says which workflow it follows; no text, or an empty one; the repo names no team; and every error of `afkconvoy check`. A consult's: `--stage` is missing, or given with no commits; the issue "is not claimed".

## What trips you up

- A ruling runs the same stage again. It does not decide the stage's verdict: the worker started under it does.
- An issue escalated because its worker went silent, or its gate stayed red, is at the stage that worker was performing: a ruling starts that stage anew.
- An escalation with no position cannot be ruled on: no flow label and no escalation comment says which workflow it follows. The error gives the line to put a flow label on it, as `afkconvoy doctor` does. With one workflow the label is filled in; with several, choose from the flow labels listed. This starts it at that workflow's first stage (`afkconvoy help humans`).
- Moving the labels by hand hands the issue back with no ruling on record: an issue its records show as escalating is escalated again at the next tick.
- Run twice, it leaves two rulings. Both count: the stage runs again once. A consult's second changes nothing: one claim declares once.
- A human's ruling starts both rework counts over. A consult's does not. On an `escalate` it is one more of the issue's rework in total, and one that would pass `max_rework` goes to a human instead; on anything else it counts nothing, and the same hand-off again goes to a human.
- A consult writes no code. The guard refuses its commits and pushes, and a commit it left on the branch all the same is a failure: the issue escalates, whatever it ruled."#;

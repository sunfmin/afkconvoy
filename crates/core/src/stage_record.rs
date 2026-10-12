//! The stage record: the one comment a stage leaves on its issue, and what an
//! issue's records say of where it is (docs/adr/0010). The comment's format
//! has this one home: only the fleet's commands write it, and nothing else
//! reads it.

use crate::position::{Declared, Place, Position};
use crate::record;
use crate::team::{LAND, Workflow};

/// The kind of record a stage record's comment opens with.
pub const STAGE_RECORD: &str = "convoy:stage";
/// What the label that shows an issue's position starts with. It is written
/// for humans to filter by and never read back.
pub const STAGE_LABEL: &str = "stage:";

/// What one stage left on its issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRecord {
    /// The name of the stage that declared it.
    pub stage: String,
    pub declared: Declared,
    /// The claim it was declared under: the commit of the claim's ref, which
    /// is written once.
    pub claim: String,
    /// The commits of the role's repo and of the team's the stage ran under.
    pub role_commit: String,
    pub team_commit: String,
    /// The issues a `blocked` verdict says it waits on.
    pub blocked_by: Vec<u64>,
    /// What the stage produced, as its worker wrote it.
    pub text: String,
}

impl StageRecord {
    /// The comment that carries the record: its marker, a line for a human,
    /// then the text.
    pub fn comment(&self) -> String {
        let blocked_by: Vec<String> = self.blocked_by.iter().map(u64::to_string).collect();
        let marker = record::marker(
            STAGE_RECORD,
            &[
                ("stage", &self.stage),
                ("verdict", self.declared.word()),
                ("claim", &self.claim),
                ("role_commit", &self.role_commit),
                ("team_commit", &self.team_commit),
                ("blocked_by", &blocked_by.join(",")),
            ],
        );
        format!(
            "{marker}\n**afkconvoy** — the stage `{}` declared `{}`.\n\n{}",
            self.stage,
            self.declared.word(),
            self.text.trim()
        )
    }

    /// The record a comment carries; `None` when it is no stage record: it
    /// does not open with the marker, or the marker leaves out what every
    /// record states.
    pub fn read(body: &str) -> Option<StageRecord> {
        let (marker, rest) = body.strip_prefix("<!--")?.split_once("-->")?;
        let fields = record::read(STAGE_RECORD, marker)?;
        let text = rest.split_once("\n\n").map_or("", |(_, text)| text);
        Some(StageRecord {
            stage: fields.get("stage")?.to_string(),
            declared: Declared::read(fields.get("verdict")?)?,
            claim: fields.get("claim")?.to_string(),
            role_commit: fields.get("role_commit")?.to_string(),
            team_commit: fields.get("team_commit")?.to_string(),
            blocked_by: fields.numbers("blocked_by"),
            text: text.to_string(),
        })
    }
}

/// The stage records among an issue's comments, oldest first, each with the
/// place of its comment.
pub fn stage_records<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Vec<(usize, StageRecord)> {
    let mut found = vec![];
    for (at, body) in bodies.into_iter().enumerate() {
        if let Some(record) = StageRecord::read(body) {
            found.push((at, record));
        }
    }
    found
}

/// How one stage record reads among those before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// It is the verdict of the stage its claim was dispatched to.
    Verdict,
    /// Its stage is not the one the issue was at: not the stage dispatched
    /// under its claim, so not a verdict.
    OtherStage,
    /// A verdict was already declared under its claim. A claim is dispatched
    /// to one stage, and that stage has ended.
    ClaimSpent,
}

impl Reading {
    pub fn word(self) -> &'static str {
        match self {
            Reading::Verdict => "verdict",
            Reading::OtherStage => "other-stage",
            Reading::ClaimSpent => "claim-spent",
        }
    }
}

/// What an issue's stage records come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History<'w> {
    /// Where the issue is, with its rework.
    pub place: Place<'w>,
    /// How each record read, in the records' order.
    pub readings: Vec<Reading>,
}

/// Where an issue of `workflow` is, given its stage records, oldest first. A
/// claim is dispatched to the stage the issue is at when it is taken, so a
/// record is a verdict only when it names that stage and is the first under
/// its claim; every other record changes nothing.
pub fn history<'w>(workflow: &'w Workflow, records: &[StageRecord]) -> History<'w> {
    let mut place = Place::enter(workflow);
    let mut spent: Vec<&str> = vec![];
    let mut readings = vec![];
    for record in records {
        let at = place.position.stage().map(|stage| stage.name.as_str());
        let reading = if spent.contains(&record.claim.as_str()) {
            Reading::ClaimSpent
        } else if at != Some(record.stage.as_str()) {
            Reading::OtherStage
        } else {
            spent.push(&record.claim);
            place = place.after(record.declared);
            Reading::Verdict
        };
        readings.push(reading);
    }
    History { place, readings }
}

/// The label that shows `position`: the stage the issue is at, `land` while
/// it is landing, and none once it has ended.
pub fn stage_label(position: Position) -> Option<String> {
    match position {
        Position::Landing => Some(format!("{STAGE_LABEL}{LAND}")),
        Position::Ended => None,
        at => at.stage().map(|stage| format!("{STAGE_LABEL}{}", stage.name)),
    }
}

/// What brings an issue's `stage:` labels to the one its position has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelMoves {
    /// The label to put on, when the issue lacks it.
    pub add: Option<String>,
    /// Every other `stage:` label it carries.
    pub remove: Vec<String>,
}

/// The moves that leave an issue carrying `labels` with the `stage:` label of
/// `position` and no other. The labels are read only to say what to write:
/// one moved by hand is moved back.
pub fn label_moves(labels: &[String], position: Position) -> LabelMoves {
    let wanted = stage_label(position);
    let mut remove = vec![];
    for label in labels {
        if label.starts_with(STAGE_LABEL) && Some(label) != wanted.as_ref() {
            remove.push(label.clone());
        }
    }
    let add = wanted.filter(|wanted| !labels.contains(wanted));
    LabelMoves { add, remove }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Declared::*;
    use crate::team::tests::{FEATURE, one};

    fn record(stage: &str, declared: Declared, claim: &str) -> StageRecord {
        StageRecord {
            stage: stage.to_string(),
            declared,
            claim: claim.to_string(),
            role_commit: "r1".to_string(),
            team_commit: "t1".to_string(),
            blocked_by: vec![],
            text: format!("{stage} is done"),
        }
    }

    fn labels(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| label.to_string()).collect()
    }

    #[test]
    fn a_record_reads_back_from_its_comment() {
        for declared in Declared::ALL {
            let written = StageRecord {
                stage: "sign off".to_string(),
                declared,
                claim: "c0ffee".to_string(),
                role_commit: "abc".to_string(),
                team_commit: "def".to_string(),
                blocked_by: vec![41, 42],
                text: "## Plan\n\nTwo steps.\n\n<!--convoy:stage stage=x-->\nand `-->`".to_string(),
            };
            let comment = written.comment();
            assert_eq!(StageRecord::read(&comment), Some(written), "{comment}");
        }
    }

    #[test]
    fn the_comment_opens_with_the_marker_and_says_the_verdict_to_a_human() {
        let comment = record("build", Pass, "c1").comment();
        assert_eq!(
            comment,
            "<!--convoy:stage stage=build verdict=pass claim=c1 role_commit=r1 team_commit=t1-->\n\
             **afkconvoy** — the stage `build` declared `pass`.\n\nbuild is done"
        );
    }

    #[test]
    fn only_a_comment_that_opens_with_a_whole_marker_is_a_record() {
        let comment = record("build", Pass, "c1").comment();
        // Quoted, or below anything else, it is prose.
        assert_eq!(StageRecord::read(&format!("> {comment}")), None);
        assert_eq!(StageRecord::read(&format!("<!--convoy:status-->\n{comment}")), None);
        assert_eq!(StageRecord::read("<!--convoy:verdict phase=blocked-->"), None);
        for field in ["stage", "verdict", "claim", "role_commit", "team_commit"] {
            let without = comment.replacen(&format!(" {field}="), " x=", 1);
            assert_eq!(StageRecord::read(&without), None, "{field}");
        }
        assert_eq!(StageRecord::read(&comment.replace("verdict=pass", "verdict=done")), None);
        // What a human reads is not what is parsed.
        let reworded = comment.replace("declared `pass`", "declared `reject`");
        assert_eq!(StageRecord::read(&reworded).unwrap().declared, Pass);
        let found = stage_records(["chat", comment.as_str(), "more chat", comment.as_str()]);
        assert_eq!(found.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [1, 3]);
    }

    /// The position `records` leave an issue of the `feature` workflow at, in
    /// a few words, with how each read.
    fn after(records: &[StageRecord]) -> (String, Vec<&'static str>) {
        let team = one(FEATURE);
        let history = history(team.workflow("feature").unwrap(), records);
        let at = match history.place.position {
            Position::Escalating(stage, reason) => {
                format!("escalating at {}: {reason:?}", stage.name)
            }
            position => stage_label(position).unwrap_or_else(|| "ended".to_string()),
        };
        (at, history.readings.iter().map(|reading| reading.word()).collect())
    }

    #[test]
    fn the_position_is_read_off_the_records() {
        assert_eq!(after(&[]).0, "stage:plan");
        let passed = [record("plan", Pass, "c1"), record("build", Pass, "c2")];
        assert_eq!(after(&passed), ("stage:review".to_string(), vec!["verdict", "verdict"]));
        let back = [&passed[..], &[record("review", Reject, "c3")]].concat();
        assert_eq!(after(&back).0, "stage:build");
        let team = one(FEATURE);
        let place = history(team.workflow("feature").unwrap(), &back).place;
        assert_eq!((place.rework(), place.rework_left()), (1, 2));
        let review = &team.workflow("feature").unwrap().stages()[2];
        assert_eq!(
            (place.consecutive_rework(review), place.consecutive_rework_left(review)),
            (1, 1)
        );
        // A verdict the stage may not declare is still that stage's verdict.
        assert_eq!(
            after(&[record("plan", Reject, "c1")]),
            ("escalating at plan: NoValidVerdict".to_string(), vec!["verdict"])
        );
    }

    #[test]
    fn a_record_of_a_stage_that_was_not_dispatched_is_not_a_verdict() {
        // The issue is at `plan`: a record of `build`, or of no stage of the
        // workflow, moves nothing.
        let stray = [record("build", Pass, "c1"), record("deploy", Pass, "c1")];
        assert_eq!(after(&stray), ("stage:plan".to_string(), vec!["other-stage", "other-stage"]));
        // …and does not spend the claim: the stage it was dispatched to still declares.
        let then = [&stray[..], &[record("plan", Pass, "c1")]].concat();
        assert_eq!(after(&then).0, "stage:build");
        // A claim is dispatched to one stage: once that has declared, the
        // claim declares nothing more, not even for the stage that follows.
        let on = [record("plan", Pass, "c1"), record("build", Pass, "c1")];
        assert_eq!(after(&on), ("stage:build".to_string(), vec!["verdict", "claim-spent"]));
    }

    #[test]
    fn a_second_record_for_one_stage_under_one_claim_counts_once() {
        let twice = [record("plan", Pass, "c1"), record("plan", Pass, "c1")];
        assert_eq!(after(&twice), ("stage:build".to_string(), vec!["verdict", "claim-spent"]));
        let changed = [record("plan", Pass, "c1"), record("plan", Escalate, "c1")];
        assert_eq!(after(&changed).0, "stage:build", "the first one stands");
    }

    #[test]
    fn the_label_names_the_stage_and_the_landing_and_nothing_once_ended() {
        assert_eq!(after(&[record("plan", Blocked, "c1")]).0, "stage:plan");
        let landed: Vec<StageRecord> = ["plan", "build", "review", "sign-off"]
            .iter()
            .enumerate()
            .map(|(at, stage)| record(stage, Pass, &format!("c{at}")))
            .collect();
        assert_eq!(after(&landed).0, "stage:land");
        assert_eq!(after(&[record("plan", AlreadySatisfied, "c1")]).0, "ended");
    }

    #[test]
    fn the_moves_leave_one_stage_label_and_a_second_look_moves_nothing() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let at_build = history(workflow, &[record("plan", Pass, "c1")]).place.position;
        let carried = labels(&["flow:feature", "stage:plan", "stage:review", "bug"]);
        let moves = label_moves(&carried, at_build);
        assert_eq!(moves.add.as_deref(), Some("stage:build"));
        assert_eq!(moves.remove, ["stage:plan", "stage:review"]);
        let settled = label_moves(&labels(&["flow:feature", "stage:build"]), at_build);
        assert_eq!(settled, LabelMoves { add: None, remove: vec![] });
        let ended = history(workflow, &[record("plan", AlreadySatisfied, "c1")]).place.position;
        let moves = label_moves(&labels(&["stage:plan"]), ended);
        assert_eq!(moves, LabelMoves { add: None, remove: vec!["stage:plan".to_string()] });
    }
}

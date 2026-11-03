//! The stage record: the one comment a stage — or a human who steers the
//! issue — leaves on it, what an issue's records say of where it is, and what
//! they call for of the claim a stage was dispatched under (docs/adr/0010).
//! The comment's format has this one home: only the fleet's commands write
//! it, and nothing else reads it.

use flux_rs::attrs::*;
use flux_rs::macros::qualifier;

use crate::blocked::BlockedOn;
use crate::harness::Harness;
use crate::lock;
use crate::position::{ConsultOn, Declared, Failure, LANDING_ESCALATION, Place, Position};
use crate::prompt::quoted;
use crate::record;
use crate::team::{HUMAN, LAND, Workflow};

/// The kind of record a stage record's comment opens with.
pub const STAGE_RECORD: &str = "convoy:stage";
/// What the label that shows an issue's position starts with. It is written
/// for humans to filter by and never read back.
pub const STAGE_LABEL: &str = "stage:";
/// Who a stage record says left it, when a consult did.
const CONSULT: &str = "consult";
/// Who a stage record says left it, when the fleet did.
const FLEET: &str = "fleet";

/// What a stage record states of its stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stated {
    /// What ended the stage, or answered its escalation.
    Declared(Declared),
    /// A human sent the issue to the stage.
    Sent,
    /// A fleet lock that explains what stopped the stage has closed: the
    /// stage runs again.
    LockClosed,
    /// The fleet saw the stage's worker fail so, and hands the issue on: to
    /// the stage's consult, where it names one (docs/adr/0018).
    Failed(Failure),
}

impl Stated {
    pub fn word(self) -> &'static str {
        match self {
            Stated::Declared(declared) => declared.word(),
            Stated::Sent => "sent",
            Stated::LockClosed => "lock-closed",
            Stated::Failed(failure) => failure.word(),
        }
    }

    pub fn read(word: &str) -> Option<Stated> {
        for stated in [Stated::Sent, Stated::LockClosed] {
            if word == stated.word() {
                return Some(stated);
            }
        }
        if let Some(failure) = Failure::read(word) {
            return Some(Stated::Failed(failure));
        }
        Declared::read(word).map(Stated::Declared)
    }
}

/// Who left a stage record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    /// The worker of the stage, or the consult asked to rule on its `escalate`.
    Worker {
        /// Whether it is the stage's consult.
        consult: bool,
        /// The claim it declared under: the commit the claim was created
        /// at, which it keeps through every takeover (`Claim::id`).
        claim: String,
        /// The commits of the role's repo and of the team's the stage ran under.
        role_commit: String,
        team_commit: String,
        /// The harness it was a session of. The commits do not say: the
        /// default is the project's.
        harness: Harness,
        /// The model it ran, where it was started on one: its harness's own
        /// name for it.
        model: Option<String>,
    },
    /// A human, through the fleet, on an issue nobody held.
    Human,
    /// The fleet itself, as a fleet lock closed.
    Fleet,
}

impl Author {
    pub fn word(&self) -> &'static str {
        match self {
            Author::Worker { consult: true, .. } => CONSULT,
            Author::Worker { consult: false, .. } => "worker",
            Author::Human => HUMAN,
            Author::Fleet => FLEET,
        }
    }
}

/// What one stage, or a human who steered the issue, left on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRecord {
    /// The name of the stage it is about: the one that declared, was ruled
    /// on, or was sent to.
    pub stage: String,
    pub stated: Stated,
    pub by: Author,
    /// What a `blocked` verdict says it waits on.
    pub blocked_by: BlockedOn,
    /// What the stage produced, as its worker wrote it; of a human, what
    /// they ruled or why they sent the issue.
    pub text: String,
}

impl StageRecord {
    /// The record the fleet leaves as `stage` runs again, the closed lock
    /// `lock` having explained what stopped it (`lock::explains`).
    pub fn lock_closed(stage: &str, lock: u64) -> StageRecord {
        StageRecord {
            stage: stage.to_string(),
            stated: Stated::LockClosed,
            by: Author::Fleet,
            blocked_by: BlockedOn::default(),
            text: lock::closed_text(lock),
        }
    }

    /// The record the fleet leaves as it hands the issue at `stage` on for
    /// `failure`, with `text`: the reason and the evidence a human handed it
    /// would be told.
    pub fn failed(stage: &str, failure: Failure, text: &str) -> StageRecord {
        StageRecord {
            stage: stage.to_string(),
            stated: Stated::Failed(failure),
            by: Author::Fleet,
            blocked_by: BlockedOn::default(),
            text: text.to_string(),
        }
    }

    /// The claim a worker declared it under; `None` of a human's and of the
    /// fleet's.
    pub fn claim(&self) -> Option<&str> {
        match &self.by {
            Author::Worker { claim, .. } => Some(claim),
            Author::Human | Author::Fleet => None,
        }
    }

    /// Whether a human ruled or sent with it: what puts the issue at its
    /// stage, to be performed under what it says.
    pub fn puts(&self) -> bool {
        match (&self.by, self.stated) {
            (Author::Worker { consult: true, .. }, Stated::Sent) => false,
            (Author::Human, Stated::Declared(Declared::Ruling)) | (_, Stated::Sent) => true,
            (..) => false,
        }
    }

    /// The record in a few words, as a human and a later stage's worker read it.
    pub fn headline(&self) -> String {
        let (stage, word) = (&self.stage, self.stated.word());
        match (&self.by, self.stated) {
            (_, Stated::Failed(failure)) => format!(
                "the stage `{stage}` failed — {} — and the fleet hands the issue on",
                failure.brief()
            ),
            (Author::Worker { consult: true, .. }, Stated::Sent) => {
                format!("the stage's consult sent this issue to the stage `{stage}`")
            }
            (_, Stated::Sent) => format!("a human sent this issue to the stage `{stage}`"),
            (_, Stated::LockClosed) | (Author::Fleet, _) => {
                format!("a fleet lock has closed, and the stage `{stage}` runs again")
            }
            (Author::Human, Stated::Declared(Declared::Ruling)) => {
                format!("a human ruled on the stage `{stage}`, which runs again under the ruling")
            }
            (Author::Human, Stated::Declared(_)) => {
                format!("a human declared `{word}` for the stage `{stage}`")
            }
            (Author::Worker { consult: true, .. }, Stated::Declared(Declared::Ruling)) => {
                format!(
                    "the consult of the stage `{stage}` ruled: the stage runs again under the ruling"
                )
            }
            (Author::Worker { consult: true, .. }, Stated::Declared(_)) => {
                format!("the consult of the stage `{stage}` declared `{word}`")
            }
            (Author::Worker { consult: false, .. }, Stated::Declared(_)) => {
                format!("the stage `{stage}` declared `{word}`")
            }
        }
    }

    /// The comment that carries the record: its marker, a line for a human,
    /// then the text.
    pub fn comment(&self) -> String {
        let blocked_by = self.blocked_by.word();
        let (claim, role_commit, team_commit, harness, model, by) = match &self.by {
            Author::Worker { consult, claim, role_commit, team_commit, harness, model } => {
                let by = if *consult { CONSULT } else { "" };
                let model = model.as_deref().unwrap_or("");
                (
                    claim.as_str(),
                    role_commit.as_str(),
                    team_commit.as_str(),
                    harness.key(),
                    model,
                    by,
                )
            }
            Author::Human => ("", "", "", "", "", HUMAN),
            Author::Fleet => ("", "", "", "", "", FLEET),
        };
        let marker = record::marker(
            STAGE_RECORD,
            &[
                ("stage", &self.stage),
                ("verdict", self.stated.word()),
                ("by", by),
                ("claim", claim),
                ("role_commit", role_commit),
                ("team_commit", team_commit),
                ("harness", harness),
                ("model", model),
                ("blocked_by", &blocked_by),
            ],
        );
        format!("{marker}\n**afkconvoy** — {}.\n\n{}", self.headline(), self.text.trim())
    }

    /// The record a comment carries; `None` when it is no stage record: it
    /// does not open with the marker, or the marker leaves out what every
    /// record states. A worker's that names no harness was written before
    /// one was named (`Harness::UNNAMED`).
    pub fn read(body: &str) -> Option<StageRecord> {
        let (marker, rest) = body.strip_prefix("<!--")?.split_once("-->")?;
        let fields = record::read(STAGE_RECORD, marker)?;
        let text = rest.split_once("\n\n").map_or("", |(_, text)| text);
        let by = match fields.get("by") {
            Some(HUMAN) => Author::Human,
            Some(FLEET) => Author::Fleet,
            Some(by) if by != CONSULT => return None,
            by => Author::Worker {
                consult: by.is_some(),
                claim: fields.get("claim")?.to_string(),
                role_commit: fields.get("role_commit")?.to_string(),
                team_commit: fields.get("team_commit")?.to_string(),
                harness: match fields.get("harness") {
                    Some(harness) => Harness::read(harness)?,
                    None => Harness::UNNAMED,
                },
                model: fields.get("model").map(str::to_string),
            },
        };
        Some(StageRecord {
            stage: fields.get("stage")?.to_string(),
            stated: Stated::read(fields.get("verdict")?)?,
            by,
            blocked_by: BlockedOn::read(fields.get("blocked_by")),
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
    /// It is the verdict of the stage its claim was dispatched to, or what a
    /// human declared or ruled at the stage the issue was at.
    Verdict,
    /// A human sent the issue to its stage.
    Sent,
    /// Its stage is not the one the issue was at: not the stage dispatched
    /// under its claim, so not a verdict.
    OtherStage,
    /// Its stage is none the workflow has: the workflow changed under the
    /// issue, which is handed to a human from the stage it was at.
    UnknownStage,
    /// A verdict was already declared under its claim. A claim is dispatched
    /// to one stage, and that stage has ended.
    ClaimSpent,
    /// The fleet's, as a fleet lock closed: the stage the issue was at runs
    /// again.
    LockClosed,
    /// The fleet's, as it saw the stage the issue was at fail: the issue is
    /// handed on.
    Failed,
}

impl Reading {
    pub fn word(self) -> &'static str {
        match self {
            Reading::Verdict => "verdict",
            Reading::Sent => "sent",
            Reading::OtherStage => "other-stage",
            Reading::UnknownStage => "unknown-stage",
            Reading::ClaimSpent => "claim-spent",
            Reading::LockClosed => "lock-closed",
            Reading::Failed => "failed",
        }
    }
}

/// What the stage records of an issue come to, in a workflow of `n` stages of
/// a team of `roles` roles.
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(n: int, roles: int)]
pub struct History<'w> {
    /// Where the issue is, with its rework.
    #[field(Place[n, roles])]
    pub place: Place<'w>,
    /// How each record read, in the records' order.
    pub readings: Vec<Reading>,
    /// Which records are the issue's rework since it entered or a human last
    /// put it at a stage, oldest first: each rejection and each consult's ruling the
    /// breakers counted, and the one a breaker refused.
    pub rounds: Vec<usize>,
    /// The stage records this was read from, oldest first: what every
    /// question asked of it reads.
    pub records: Vec<StageRecord>,
}

/// Where an issue of `workflow` is, given its stage records, oldest first. A
/// claim is dispatched to the stage the issue is at when it is taken, so a
/// worker's record is a verdict only when it names that stage and is the
/// first under its claim. A human declares and rules at the stage the issue
/// is at, and sends it to any stage; ruling and sending put the issue at the
/// stage, to be performed, with both rework counts started over. A record
/// naming a stage the workflow does not have escalates the issue: the
/// workflow changed, and nothing is guessed (docs/adr/0010). The fleet's
/// record of a lock that closed sets aside the `escalate` of the stage the
/// issue is at (`Place::again`). Every other record changes nothing.
#[spec(fn(&Workflow[@w], Vec<StageRecord>) -> History[w.n, w.roles])]
pub fn history<'w>(workflow: &'w Workflow, records: Vec<StageRecord>) -> History<'w> {
    let mut place = Place::enter(workflow);
    // The stage of the last verdict of a stage's own worker: a failure the
    // fleet saw of that verdict is handed on from that stage.
    let mut refuted: Option<usize> = None;
    let mut spent: Vec<&str> = vec![];
    let mut readings = vec![];
    let mut rounds = vec![];
    for (nth, record) in records.iter().enumerate() {
        // What stays true of the place on every turn, which Flux does not
        // find alone: the stage it is at, while at one, is the workflow's.
        qualifier!(at_stage: bool, of: int, n: int; at_stage => of == n);
        let at = place.position.stage();
        let reading = if record.claim().is_some_and(|claim| spent.contains(&claim)) {
            Reading::ClaimSpent
        } else if record.stage == LAND {
            if !place.position.landing() {
                Reading::OtherStage
            } else if record.puts() {
                place.position = Position::Landing;
                Reading::Verdict
            } else if let Stated::Declared(declared) = record.stated {
                if let Some(claim) = record.claim() {
                    spent.push(claim);
                }
                place = place.after(declared);
                Reading::Verdict
            } else {
                Reading::OtherStage
            }
        } else if let Some(named) = workflow.place_of(&record.stage) {
            let own = at.filter(|stage| stage.at() == named);
            let consulted = matches!(place.position, Position::Consult(..));
            if record.stated == Stated::Sent && !record.puts() {
                // A consult's sending: only while the issue is with it, and once
                // under its claim.
                if !consulted {
                    Reading::OtherStage
                } else {
                    if let Some(claim) = record.claim() {
                        spent.push(claim);
                    }
                    let had = place.rework();
                    place = place.sent_by_consult(workflow.stage(named));
                    if place.rework() > had || place.position.breaker().is_some() {
                        rounds.push(nth);
                    }
                    Reading::Verdict
                }
            } else if record.stated == Stated::Sent {
                place = place.put(workflow.stage(named));
                rounds.clear();
                Reading::Sent
            } else if let Stated::Failed(failure) = record.stated {
                if own.is_some() {
                    place = place.failed(failure);
                    Reading::Failed
                } else if refuted.take() == Some(named) {
                    place = place.refuted(workflow.stage(named), failure);
                    Reading::Failed
                } else {
                    Reading::OtherStage
                }
            } else if own.is_none() {
                Reading::OtherStage
            } else if let Some(stage) = own.filter(|_| record.puts()) {
                // A human rules wherever the issue stopped: an escalation is not
                // always one its records show.
                place = place.put(stage);
                rounds.clear();
                Reading::Verdict
            } else if record.stated == Stated::LockClosed {
                place = place.again();
                Reading::LockClosed
            } else if let Stated::Declared(declared) = record.stated {
                if let Some(claim) = record.claim() {
                    spent.push(claim);
                }
                let before = place.position;
                let had = place.rework();
                if matches!(record.by, Author::Worker { consult: false, .. }) {
                    refuted = Some(named);
                }
                place = place.after(declared);
                // The counts are the one account of what is rework: a round is
                // what raised the total, or what a breaker refused to count. A
                // human's ruling starts the rounds over with the counts.
                let escalated = matches!(place.position, Position::Escalating(..));
                if matches!(before, Position::Escalating(..)) {
                    if !escalated {
                        rounds.clear();
                    }
                } else if place.rework() > had || place.position.breaker().is_some() {
                    rounds.push(nth);
                }
                Reading::Verdict
            } else {
                // A sending: read above, whatever stage it names.
                Reading::Sent
            }
        } else {
            // Handed on from the stage it was at: an issue already with a
            // human, or past its stages, stays as it is.
            place = place.changed();
            Reading::UnknownStage
        };
        readings.push(reading);
    }
    History { place, readings, rounds, records }
}

impl History<'_> {
    /// Which of its records is the verdict of the stage dispatched under
    /// `claim`; `None` while that stage has declared nothing that reads as
    /// one.
    pub fn verdict_under(&self, claim: &str) -> Option<usize> {
        for (at, record) in self.records.iter().enumerate() {
            let verdict = self.readings.get(at) == Some(&Reading::Verdict);
            if verdict && record.claim() == Some(claim) {
                return Some(at);
            }
        }
        None
    }

    /// Which of its records is the `escalate` a fleet lock may explain: the one the worker of the stage the issue
    /// is at declared, which left it with that stage's consult or with a
    /// human. `None` anywhere else, and once a consult has had its word.
    pub fn stage_escalate(&self) -> Option<usize> {
        let asked = matches!(
            self.place.position,
            Position::Consult(_, _, ConsultOn::Escalate)
                | Position::Escalating(_, crate::position::Reason::Verdict)
        );
        let mut last = None;
        for (at, reading) in self.readings.iter().enumerate() {
            if *reading == Reading::Verdict {
                last = Some(at);
            }
        }
        let at = last.filter(|_| asked)?;
        let record = self.records.get(at)?;
        let own = matches!(record.by, Author::Worker { consult: false, .. });
        (own && record.stated == Stated::Declared(Declared::Escalate)).then_some(at)
    }

    /// The rework a breaker stopped, as the human it is handed to is told
    /// it: every round's stage and reason, oldest first, and the URL of each
    /// one's comment, `urls` being those of its records. `None` where no
    /// breaker tripped.
    pub fn rounds_said(&self, urls: &[String]) -> Option<String> {
        let breaker = self.place.position.breaker()?;
        let mut said = format!(
            "Every round of rework since this issue entered its workflow or a human last ruled \
             on it, oldest first. The last is the one `{}` refused.",
            breaker.key()
        );
        let mut round = 0u64;
        for nth in &self.rounds {
            let Some(record) = self.records.get(*nth) else { continue };
            round = round.saturating_add(1);
            let (stage, declared) = (&record.stage, record.stated.word());
            let who = match record.by {
                Author::Worker { consult: true, .. } => format!("the consult of `{stage}`"),
                _ => format!("`{stage}`"),
            };
            said.push_str(&format!("\n\n{round}. {who} declared `{declared}`"));
            if let Some(url) = urls.get(*nth) {
                said.push_str(&format!(" — {url}"));
            }
            said.push_str(&format!("\n\n{}", quoted(&record.text)));
        }
        Some(said)
    }
}

/// What an issue already handed to a human calls for, where no consult saw
/// what handed it over (docs/adr/0018).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unconsulted {
    /// Its records have it with its stage's consult: it goes back to the
    /// fleet as it is.
    Back,
    /// The fleet leaves the record of its escalation, at this stage, which
    /// puts it with the stage's consult, and it goes back to the fleet.
    Record(String),
}

/// What an issue handed to a human, whose stage records read as `history`,
/// calls for: to be put to its stage's consult once, where
/// that stage names one and no consult had its word on the issue — or was
/// handed it — since a human last put it at a stage, or its records have it
/// with the consult already. `None` where it stays with the human: a consult
/// let it through, or no consult is named. Once its labels are back it is
/// the fleet's, and nothing is put again.
pub fn unconsulted(history: &History) -> Option<Unconsulted> {
    let (place, records) = (&history.place, &history.records);
    // Whatever was left before: a put cut short after its record is
    // finished by its labels.
    if let Position::Consult(..) = place.position {
        return Some(Unconsulted::Back);
    }
    // From the last record that put the issue at a stage: a human's, so
    // neither a consult's nor the fleet's.
    let since = records.iter().rposition(StageRecord::puts).unwrap_or(0);
    let seen = records.get(since..).unwrap_or_default().iter().any(|record| {
        matches!(record.by, Author::Worker { consult: true, .. })
            || matches!(record.stated, Stated::Failed(_))
    });
    if seen {
        return None;
    }
    let stage = place.position.stage()?;
    let failed = crate::position::Reason::Failed(Failure::Escalated);
    place.consults(failed).then(|| Unconsulted::Record(stage.name.clone()))
}

/// What the tick does with a claim a stage was dispatched under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advance {
    /// Its stage has declared nothing yet, and is still to be performed —
    /// or its consult has not ruled yet: ask after its worker.
    AskAfter,
    /// The claim has no more to do and the issue stays as its records put
    /// it: release it. A worker's next stage is the frontier's to start,
    /// under a claim of its own.
    HandOn,
    /// Its stage's `pass` ended the workflow: release it, with nothing more
    /// to do on the issue.
    End,
    /// Its stage declared `already-satisfied`: close the issue, if its
    /// branch holds no work.
    Close,
    /// The consult closes with a reason, preserving any branch work.
    ConsultClose,
    /// Its stage declared `blocked` on these issues, or on the next cut:
    /// where they stand decides (`blocked::blocked_route`).
    Blocked(BlockedOn),
    /// The issue is handed to a human, for this reason.
    Escalate(String),
}

/// What `history` calls for of the claim `claim` on its issue. An escalation is carried out whoever declared what led to it;
/// anything else is acted on only once the claim's own stage has declared.
pub fn advance(history: &History, claim: &str) -> Advance {
    let position = history.place.position;
    if let Position::Escalating(stage, reason) = position {
        return Advance::Escalate(reason.said(stage));
    }
    if position == Position::LandingEscalating {
        return Advance::Escalate(LANDING_ESCALATION.to_string());
    }
    let verdict = history.verdict_under(claim).and_then(|at| history.records.get(at));
    let Some(verdict) = verdict else {
        // A claim on an issue no worker has a stage to perform on, or to
        // rule on, holds nothing.
        let asked = position.start().is_some() || matches!(position, Position::Consult(..));
        return if asked { Advance::AskAfter } else { Advance::HandOn };
    };
    match position {
        Position::ParkedForEvidence(_) => Advance::HandOn,
        Position::Ended if verdict.stated == Stated::Declared(Declared::Close) => {
            Advance::ConsultClose
        }
        Position::Parked(_) | Position::LandingParked => {
            Advance::Blocked(verdict.blocked_by.clone())
        }
        Position::Ended if verdict.stated == Stated::Declared(Declared::AlreadySatisfied) => {
            Advance::Close
        }
        Position::Ended => Advance::End,
        _ => Advance::HandOn,
    }
}

/// Whether `claim` is one a landing worker holds: the issue `history` is of
/// is landing, and no stage declared under the claim.
/// The claim of the stage whose `pass` led there is not: it is released, and
/// the join is started under a claim of its own.
pub fn lands(history: &History, claim: &str) -> bool {
    matches!(history.place.position, Position::Landing | Position::LandingParked)
        && history.verdict_under(claim).is_none()
}

/// The label that shows `position`: the stage the issue is at, `land` while
/// it is landing, and none once it has ended.
pub fn stage_label(position: Position) -> Option<String> {
    match position {
        at if at.landing() => Some(format!("{STAGE_LABEL}{LAND}")),
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
    use super::Author::Human;
    use super::*;
    use crate::position::Declared::*;
    use crate::team::tests::{FEATURE, one};

    fn record(stage: &str, declared: Declared, claim: &str) -> StageRecord {
        StageRecord {
            stage: stage.to_string(),
            stated: Stated::Declared(declared),
            by: Author::Worker {
                consult: false,
                claim: claim.to_string(),
                role_commit: "r1".to_string(),
                team_commit: "t1".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
            },
            blocked_by: BlockedOn::default(),
            text: format!("{stage} is done"),
        }
    }

    /// What a human left about `stage` with no claim: a verdict, a ruling,
    /// or a sending.
    fn human(stage: &str, stated: Stated) -> StageRecord {
        StageRecord { stated, by: Author::Human, ..record(stage, Pass, "") }
    }

    fn labels(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| label.to_string()).collect()
    }

    #[test]
    fn a_record_reads_back_from_its_comment() {
        let worker = Author::Worker {
            consult: false,
            claim: "c0ffee".to_string(),
            role_commit: "abc".to_string(),
            team_commit: "def".to_string(),
            harness: Harness::ClaudeCode,
            model: None,
        };
        let consult = Author::Worker {
            consult: true,
            claim: "c0ffee".to_string(),
            role_commit: "abc".to_string(),
            team_commit: "def".to_string(),
            harness: Harness::ClaudeCode,
            model: None,
        };
        let stated = Declared::ALL.into_iter().map(Stated::Declared).chain([Stated::Sent]);
        let by = [worker, consult, Human];
        for (stated, by) in stated.flat_map(|stated| by.clone().map(|by| (stated, by))) {
            let written = StageRecord {
                stage: "sign off".to_string(),
                stated,
                by,
                blocked_by: BlockedOn::issues(vec![41, 42]),
                text: "## Plan\n\nTwo steps.\n\n<!--convoy:stage stage=x-->\nand `-->`".to_string(),
            };
            let comment = written.comment();
            assert_eq!(StageRecord::read(&comment), Some(written), "{comment}");
        }
    }

    #[test]
    fn a_human_s_record_names_no_claim_and_says_so_to_a_human() {
        let ruled = human("build", Stated::Declared(Ruling)).comment();
        assert_eq!(
            ruled,
            "<!--convoy:stage stage=build verdict=ruling by=human-->\n**afkconvoy** — a human \
             ruled on the stage `build`, which runs again under the ruling.\n\nbuild is done"
        );
        let passed = human("sign-off", Stated::Declared(Pass)).headline();
        assert_eq!(passed, "a human declared `pass` for the stage `sign-off`");
        let sent = human("plan", Stated::Sent).comment();
        assert!(sent.starts_with("<!--convoy:stage stage=plan verdict=sent by=human-->\n"));
        assert!(sent.contains("a human sent this issue to the stage `plan`."), "{sent}");
        // Whoever else a marker names is nobody the fleet writes for.
        assert_eq!(StageRecord::read(&ruled.replace("by=human", "by=robot")), None);
        assert_eq!(human("plan", Stated::Sent).claim(), None);
        assert_eq!((Author::Human.word(), record("a", Pass, "c").by.word()), ("human", "worker"));
    }

    #[test]
    fn the_comment_opens_with_the_marker_and_says_the_verdict_to_a_human() {
        let comment = record("build", Pass, "c1").comment();
        assert_eq!(
            comment,
            "<!--convoy:stage stage=build verdict=pass claim=c1 role_commit=r1 team_commit=t1 \
             harness=claude-code-->\n**afkconvoy** — the stage `build` declared `pass`.\n\nbuild \
             is done"
        );
    }

    #[test]
    fn a_worker_s_record_names_its_harness_and_one_written_before_it_did_is_still_read() {
        let written = record("build", Pass, "c1");
        let comment = written.comment();
        // The record of a worker started before any named a harness.
        let before = comment.replace(" harness=claude-code", "");
        assert_ne!(before, comment);
        assert_eq!(StageRecord::read(&before), Some(written));
        // A name that is no harness is nothing the fleet wrote.
        assert_eq!(StageRecord::read(&comment.replace("=claude-code", "=vim")), None);
        // A human, and the fleet, are a session of none.
        assert!(!human("build", Stated::Declared(Pass)).comment().contains("harness"));
        assert!(!StageRecord::lock_closed("build", 3).comment().contains("harness"));
    }

    #[test]
    fn a_worker_s_record_names_the_model_it_ran_where_it_ran_one() {
        let mut written = record("build", Pass, "c1");
        if let Author::Worker { harness, model, .. } = &mut written.by {
            (*harness, *model) = (Harness::Pi, Some("openai/gpt-5.5".to_string()));
        }
        let comment = written.comment();
        assert!(comment.contains(" harness=pi model=openai/gpt-5.5-->"), "{comment}");
        assert_eq!(StageRecord::read(&comment), Some(written));
        // One started on its command as written names none.
        assert!(!record("build", Pass, "c1").comment().contains("model="));
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
        assert_eq!(StageRecord::read(&reworded).unwrap().stated, Stated::Declared(Pass));
        let found = stage_records(["chat", comment.as_str(), "more chat", comment.as_str()]);
        assert_eq!(found.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [1, 3]);
    }

    /// The position `records` leave an issue of the `feature` workflow at, in
    /// a few words, with how each read.
    fn after(records: &[StageRecord]) -> (String, Vec<&'static str>) {
        let team = one(FEATURE);
        let history = history(team.workflow("feature").unwrap(), records.to_vec());
        let at = match history.place.position {
            Position::Escalating(stage, reason) => {
                format!("escalating at {}: {reason:?}", stage.name)
            }
            Position::Consult(stage, _, on) => format!("consult at {}: {on:?}", stage.name),
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
        let place = history(team.workflow("feature").unwrap(), back.to_vec()).place;
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
        // The issue is at `plan`: a record of `build` or of `review` moves nothing.
        let stray = [record("build", Pass, "c1"), record("review", Pass, "c1")];
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
    fn a_record_of_a_stage_the_workflow_no_longer_has_hands_the_issue_on() {
        // `deploy` was a stage once: the workflow changed under the issue, at
        // `build`, which names a consult.
        let changed = [record("plan", Pass, "c1"), record("deploy", Pass, "c2")];
        assert_eq!(
            after(&changed),
            (
                "consult at build: HandOff(WorkflowChanged)".to_string(),
                vec!["verdict", "unknown-stage"]
            )
        );
        // Its consult rules, and `build` runs again; or lets it through to a human.
        let ruled = [&changed[..], &[consult("build", Ruling, "c3")]].concat();
        assert_eq!(after(&ruled).0, "stage:build");
        let through = [&changed[..], &[consult("build", Escalate, "c3")]].concat();
        assert_eq!(after(&through).0, "escalating at build: Consult");
        // At `plan`, which names none, it goes to a human, and nothing declared
        // after it moves the issue: a human rules first.
        let alone = [record("deploy", Pass, "c2")];
        assert_eq!(after(&alone).0, "escalating at plan: WorkflowChanged");
        let on = [&alone[..], &[record("plan", Pass, "c3")]].concat();
        assert_eq!(after(&on).0, "escalating at plan: WorkflowChanged");
        let ruled = [&alone[..], &[human("plan", Stated::Declared(Ruling))]].concat();
        assert_eq!(after(&ruled).0, "stage:plan");
        // An issue past its stages has none to be escalated from.
        let ended = [record("plan", AlreadySatisfied, "c1"), record("deploy", Pass, "c2")];
        assert_eq!(after(&ended), ("ended".to_string(), vec!["verdict", "unknown-stage"]));
        let team = one(FEATURE);
        let place = history(team.workflow("feature").unwrap(), alone.to_vec()).place;
        let Position::Escalating(stage, reason) = place.position else { panic!("escalating") };
        assert!(reason.said(stage).starts_with("the workflow changed under this issue"));
    }

    /// What the records call for of the claim `claim`, on the `feature` workflow.
    fn advanced(records: &[StageRecord], claim: &str) -> Advance {
        let team = one(FEATURE);
        advance(&history(team.workflow("feature").unwrap(), records.to_vec()), claim)
    }

    #[test]
    fn a_claim_is_asked_after_until_its_own_stage_declares() {
        let built = [record("plan", Pass, "c1"), record("build", Pass, "c2")];
        assert_eq!(advanced(&[], "c1"), Advance::AskAfter);
        // `review` is still to be performed under c3, whatever earlier claims declared…
        assert_eq!(advanced(&built, "c3"), Advance::AskAfter);
        // …and a record that is no verdict — another stage's — declares nothing.
        let stray = [&built[..], &[record("plan", Pass, "c3")]].concat();
        assert_eq!(advanced(&stray, "c3"), Advance::AskAfter);
        assert_eq!(
            history(one(FEATURE).workflow("feature").unwrap(), built.to_vec()).verdict_under("c2"),
            Some(1)
        );
    }

    /// What the consult of `stage` left under `claim`.
    fn consult(stage: &str, declared: Declared, claim: &str) -> StageRecord {
        let mut left = record(stage, declared, claim);
        if let Author::Worker { consult, .. } = &mut left.by {
            *consult = true;
        }
        left
    }

    #[test]
    fn a_consult_s_claim_is_asked_after_until_it_rules_or_lets_the_escalation_through() {
        // `build` names a consult: its `escalate` ends its own claim…
        let asked = [record("plan", Pass, "c1"), record("build", Escalate, "c2")];
        assert_eq!(advanced(&asked, "c2"), Advance::HandOn);
        // …and the consult's is asked after, as a stage's is.
        assert_eq!(advanced(&asked, "c3"), Advance::AskAfter);
        let ruled = [&asked[..], &[consult("build", Ruling, "c3")]].concat();
        assert_eq!(after(&ruled), ("stage:build".to_string(), vec!["verdict"; 3]));
        assert_eq!(advanced(&ruled, "c3"), Advance::HandOn);
        assert_eq!(advanced(&ruled, "c4"), Advance::AskAfter, "`build` runs again");
        let through = [&asked[..], &[consult("build", Escalate, "c3")]].concat();
        let let_through = "the consult of the stage `build` let it through";
        assert_eq!(advanced(&through, "c3"), Advance::Escalate(let_through.to_string()));
        // What it leaves says a consult did.
        assert_eq!(
            consult("build", Ruling, "c3").comment(),
            "<!--convoy:stage stage=build verdict=ruling by=consult claim=c3 role_commit=r1 \
             team_commit=t1 harness=claude-code-->\n**afkconvoy** — the consult of the stage \
             `build` ruled: the stage runs again under the ruling.\n\nbuild is done"
        );
        assert_eq!(
            consult("build", Escalate, "c3").headline(),
            "the consult of the stage `build` declared `escalate`"
        );
    }

    #[test]
    fn the_record_of_a_closed_lock_sets_the_escalate_of_its_stage_aside() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        // `plan` goes to a human: its `escalate` is a human's, until a lock explains it.
        let asked = [record("plan", Escalate, "c1")];
        assert_eq!(history(workflow, asked.to_vec()).stage_escalate(), Some(0));
        let closed = StageRecord::lock_closed("plan", 13);
        assert_eq!(
            closed.comment(),
            format!(
                "<!--convoy:stage stage=plan verdict=lock-closed by=fleet-->\n**afkconvoy** — a \
                 fleet lock has closed, and the stage `plan` runs again.\n\n{}",
                lock::closed_text(13)
            )
        );
        assert_eq!(StageRecord::read(&closed.comment()), Some(closed.clone()));
        let again = [&asked[..], &[closed]].concat();
        assert_eq!(after(&again), ("stage:plan".to_string(), vec!["verdict", "lock-closed"]));
        let read = history(workflow, again.to_vec());
        assert_eq!((read.place.rework(), read.stage_escalate()), (0, None));
        // The claim that declared is spent and released; the stage is a new one's.
        assert_eq!(advanced(&again, "c1"), Advance::HandOn);
        assert_eq!(advanced(&again, "c2"), Advance::AskAfter);
        // With the consult `build` names, the same: the consult is not asked.
        let consulted = [record("plan", Pass, "c1"), record("build", Escalate, "c2")];
        assert_eq!(history(workflow, consulted.to_vec()).stage_escalate(), Some(1));
        let again = [&consulted[..], &[StageRecord::lock_closed("build", 13)]].concat();
        assert_eq!(after(&again).0, "stage:build");
        assert_eq!(advanced(&again, "c3"), Advance::AskAfter);

        // No lock explains a consult's word, a breaker, a human's `escalate`, or
        // a verdict that is none.
        let through = [&consulted[..], &[consult("build", Escalate, "c3")]].concat();
        assert_eq!(history(workflow, through.to_vec()).stage_escalate(), None);
        let human = StageRecord { by: Author::Human, ..record("plan", Escalate, "") };
        for stands in [&[record("plan", Pass, "c1")][..], &[record("plan", Reject, "c1")], &[human]]
        {
            assert_eq!(history(workflow, stands.to_vec()).stage_escalate(), None, "{stands:?}");
        }
        // …and the record moves an issue nowhere it is not asked from.
        let passed = [record("plan", Pass, "c1"), StageRecord::lock_closed("build", 13)];
        assert_eq!(after(&passed), ("stage:build".to_string(), vec!["verdict", "lock-closed"]));
        let stray = [record("plan", Escalate, "c1"), StageRecord::lock_closed("build", 13)];
        assert_eq!(after(&stray).1, ["verdict", "other-stage"]);
    }

    #[test]
    fn an_issue_no_consult_saw_handed_to_a_human_is_put_to_its_consult_once() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let put = |records: &[StageRecord]| unconsulted(&history(workflow, records.to_vec()));
        // Its worker at `build` went silent, and it was escalated with no record.
        let silent = [record("plan", Pass, "c1")];
        assert_eq!(put(&silent), Some(Unconsulted::Record("build".to_string())));
        let recorded =
            [&silent[..], &[StageRecord::failed("build", Failure::Escalated, "x")]].concat();
        assert_eq!(after(&recorded).0, "consult at build: HandOff(Failed(Escalated))");
        // A put cut short after its record is finished by its labels: once
        // they are back, the issue is the fleet's, and nothing is put again.
        assert_eq!(put(&recorded), Some(Unconsulted::Back));
        // A breaker its records hand on now: only the labels go back.
        let mut tripped = silent.to_vec();
        for round in 0..3 {
            tripped.push(record("build", Pass, &format!("b{round}")));
            tripped.push(record("review", Reject, &format!("r{round}")));
        }
        assert_eq!(put(&tripped), Some(Unconsulted::Back));
        // One a consult let through stays, and so does one at a stage with none.
        let through =
            [&silent[..], &[record("build", Escalate, "c2"), consult("build", Escalate, "c3")]]
                .concat();
        assert_eq!(put(&through), None);
        assert_eq!(put(&[record("plan", Escalate, "c1")]), None);
        assert_eq!(put(&[]), None, "at `plan`, which goes to a human");
        // A human's ruling starts it over: what follows is unseen again.
        let ruled = [&through[..], &[human("build", Stated::Declared(Ruling))]].concat();
        assert_eq!(put(&ruled), Some(Unconsulted::Record("build".to_string())));
    }

    #[test]
    fn a_verdict_releases_the_claim_and_says_what_else_is_done() {
        // pass and reject hand the issue on: its next stage is the frontier's.
        let passed = [record("plan", Pass, "c1")];
        assert_eq!(advanced(&passed, "c1"), Advance::HandOn);
        let rejected =
            [&passed[..], &[record("build", Pass, "c2"), record("review", Reject, "c3")]].concat();
        assert_eq!(advanced(&rejected, "c3"), Advance::HandOn);
        // …as does a pass to a human's stage, which holds no claim.
        let reviewed =
            [&passed[..], &[record("build", Pass, "c2"), record("review", Pass, "c3")]].concat();
        assert_eq!(advanced(&reviewed, "c3"), Advance::HandOn);
        let mut waiting = record("plan", Blocked, "c1");
        waiting.blocked_by = BlockedOn::issues(vec![41, 42]);
        assert_eq!(advanced(&[waiting], "c1"), Advance::Blocked(BlockedOn::issues(vec![41, 42])));
        assert_eq!(advanced(&[record("plan", AlreadySatisfied, "c1")], "c1"), Advance::Close);
        let triage = one("stages:\n  - { name: a, by: dev, do: plan, on: { pass: done } }\n");
        let done = [record("a", Pass, "c1")];
        let ended = history(triage.workflow("feature").unwrap(), done.to_vec());
        assert_eq!(advance(&ended, "c1"), Advance::End);
    }

    #[test]
    fn a_landing_worker_s_claim_is_one_no_stage_declared_under() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let signed = [
            record("plan", Pass, "c1"),
            record("build", Pass, "c2"),
            record("review", Pass, "c3"),
            record("sign-off", Pass, "c4"),
        ];
        let landing = history(workflow, signed.to_vec());
        // The claim whose stage passed to the landing train is released…
        assert!(!lands(&landing, "c4"));
        assert_eq!(advance(&landing, "c4"), Advance::HandOn);
        // …and the join is another's.
        assert!(lands(&landing, "c5"));
        let reviewed = &signed[..3];
        assert!(!lands(&history(workflow, reviewed.to_vec()), "c5"), "not landing yet");
    }

    #[test]
    fn landing_verdicts_settle_the_claim_and_keep_the_landing_destination() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let signed: Vec<_> = ["plan", "build", "review", "sign-off"]
            .into_iter()
            .enumerate()
            .map(|(at, stage)| record(stage, Pass, &format!("c{at}")))
            .collect();
        let mut blocked = record(LAND, Blocked, "landing");
        blocked.blocked_by = BlockedOn::issues(vec![41]);
        let mut records = signed.clone();
        records.push(blocked);
        // Another declaration under the spent claim never changes its first word.
        records.push(record(LAND, Escalate, "landing"));
        let parked = history(workflow, records.clone());
        assert_eq!(parked.place.position, Position::LandingParked);
        assert_eq!(advance(&parked, "landing"), Advance::Blocked(BlockedOn::issues(vec![41])));
        assert!(!lands(&parked, "landing"));
        assert!(lands(&parked, "resumed"));
        assert_eq!(parked.readings.last(), Some(&Reading::ClaimSpent));
        assert_eq!(stage_label(parked.place.position).as_deref(), Some("stage:land"));

        records.push(record(LAND, Escalate, "resumed"));
        let escalated = history(workflow, records.clone());
        assert_eq!(escalated.place.position, Position::LandingEscalating);
        assert_eq!(advance(&escalated, "resumed"), Advance::Escalate(LANDING_ESCALATION.into()));
        assert!(!lands(&escalated, "another"));
        assert!(unconsulted(&escalated).is_none());
        records.push(human(LAND, Stated::Declared(Ruling)));
        let resumed = history(workflow, records);
        assert_eq!(resumed.place.position, Position::Landing);
        assert_eq!(resumed.place.rework(), parked.place.rework());
        assert!(lands(&resumed, "another"));

        // `land` before the last stage passes cannot move the issue there.
        assert_eq!(
            history(workflow, vec![record(LAND, Blocked, "early")]).readings,
            [Reading::OtherStage]
        );
    }

    #[test]
    fn an_escalation_is_carried_out_whoever_declared_what_led_to_it() {
        let said = |reason: &str| Advance::Escalate(reason.to_string());
        let asked = [record("plan", Escalate, "c1")];
        let verdict = "the stage `plan` declared `escalate`, which it hands to a human";
        assert_eq!(advanced(&asked, "c1"), said(verdict));
        // A verdict its stage may not declare is no valid verdict.
        let invalid = [record("plan", Reject, "c1")];
        assert_eq!(
            advanced(&invalid, "c1"),
            said("no valid verdict was declared at the stage `plan`")
        );
        // The workflow changed under a stage at work: its claim is escalated too.
        let changed = [record("deploy", Pass, "c0")];
        assert!(
            matches!(advanced(&changed, "c1"), Advance::Escalate(why) if why.contains("changed"))
        );
    }

    #[test]
    fn a_breaker_s_rounds_are_every_rework_since_a_human_last_ruled_in_order() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let mut records = vec![record("plan", Pass, "c1")];
        // `review` rejects three times in a row: max_consecutive_rework is 2.
        for round in 0..3 {
            records.push(record("build", Pass, &format!("b{round}")));
            let mut rejection = record("review", Reject, &format!("r{round}"));
            rejection.text = format!("Finding {round}.\nStill there.");
            records.push(rejection);
        }
        let urls: Vec<String> = (0..records.len()).map(|at| format!("https://gh/c/{at}")).collect();
        let before = history(workflow, records[..5].to_vec());
        assert_eq!(before.rounds, [2, 4]);
        assert_eq!(before.rounds_said(&urls), None, "no breaker tripped");
        let tripped = history(workflow, records.to_vec());
        assert_eq!(tripped.rounds, [2, 4, 6], "the one the breaker refused is a round too");
        assert_eq!(
            tripped.rounds_said(&urls).unwrap(),
            "Every round of rework since this issue entered its workflow or a human last ruled \
             on it, oldest first. The last is the one `max_consecutive_rework` refused.\n\n\
             1. `review` declared `reject` — https://gh/c/2\n\n> Finding 0.\n> Still there.\n\n\
             2. `review` declared `reject` — https://gh/c/4\n\n> Finding 1.\n> Still there.\n\n\
             3. `review` declared `reject` — https://gh/c/6\n\n> Finding 2.\n> Still there."
        );
        // What is declared on an escalated issue is no round, and a human's
        // ruling starts the rounds over with the counts.
        records.push(record("build", Reject, "x1"));
        assert_eq!(history(workflow, records.to_vec()).rounds, [2, 4, 6]);
        records.push(record("build", Ruling, "h1"));
        assert_eq!(history(workflow, records.to_vec()).rounds, [] as [usize; 0]);
        records.extend([record("build", Pass, "b3"), record("review", Reject, "r3")]);
        assert_eq!(history(workflow, records.to_vec()).rounds, [10]);
        // A consult's ruling is a round; a park and an escalation no breaker made are none.
        let asked = [record("plan", Blocked, "c0"), record("plan", Pass, "c1")];
        let asked = [&asked[..], &[record("build", Escalate, "c2"), record("build", Ruling, "c3")]]
            .concat();
        assert_eq!(history(workflow, asked.to_vec()).rounds, [3]);
        let stuck = [record("plan", Escalate, "c1")];
        assert_eq!(history(workflow, stuck.to_vec()).rounds_said(&[]), None);
    }

    #[test]
    fn a_human_declares_the_verdict_of_a_stage_under_no_claim() {
        let reviewed =
            [record("plan", Pass, "c1"), record("build", Pass, "c2"), record("review", Pass, "c3")];
        assert_eq!(after(&reviewed).0, "stage:sign-off");
        let pass = Stated::Declared(Pass);
        let signed = [&reviewed[..], &[human("sign-off", pass)]].concat();
        assert_eq!(after(&signed).0, "stage:land");
        assert_eq!(after(&signed).1.last(), Some(&"verdict"));
        // A human's stage may reject, and that is rework like any rejection.
        let back = [&reviewed[..], &[human("sign-off", Stated::Declared(Reject))]].concat();
        assert_eq!(after(&back).0, "stage:plan");
        let team = one(FEATURE);
        let read = history(team.workflow("feature").unwrap(), back.to_vec());
        assert_eq!((read.place.rework(), &read.rounds[..]), (1, &[3][..]));
        // Each of a human's records counts: none is under a claim to be spent.
        let twice = [human("plan", pass), human("build", pass)];
        assert_eq!(after(&twice), ("stage:review".to_string(), vec!["verdict", "verdict"]));
        // …but only at the stage the issue is at.
        assert_eq!(after(&[human("review", pass)]).1, ["other-stage"]);
    }

    #[test]
    fn a_human_s_ruling_runs_the_stage_again_wherever_the_issue_stopped() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let ruling = Stated::Declared(Ruling);
        // On an escalation its records show…
        let asked = [record("plan", Escalate, "c1"), human("plan", ruling)];
        assert_eq!(after(&asked), ("stage:plan".to_string(), vec!["verdict", "verdict"]));
        assert!(matches!(history(workflow, asked.to_vec()).place.position, Position::Perform(..)));
        // …and on one they do not: a worker that went silent declared nothing.
        let silent = [record("plan", Pass, "c1"), human("build", ruling)];
        assert_eq!(after(&silent), ("stage:build".to_string(), vec!["verdict", "verdict"]));
        // Both rework counts are taken from after it.
        let mut records = vec![record("plan", Pass, "c1")];
        for round in 0..3 {
            records.push(record("build", Pass, &format!("b{round}")));
            records.push(record("review", Reject, &format!("r{round}")));
        }
        assert_eq!(after(&records).0, "consult at build: HandOff(Breaker(ConsecutiveRework))");
        records.push(human("build", ruling));
        let ruled = history(workflow, records.to_vec());
        let review = &workflow.stages()[2];
        assert_eq!((ruled.place.rework(), ruled.place.consecutive_rework(review)), (0, 0));
        assert_eq!(ruled.rounds, [] as [usize; 0]);
        records.extend([record("build", Pass, "b3"), record("review", Reject, "r3")]);
        let on = history(workflow, records.to_vec());
        assert_eq!((on.place.rework(), on.place.consecutive_rework(review)), (1, 1));
        assert_eq!(on.rounds, [9]);
        // A ruling names the stage the issue is at, and an issue past its stages is at none.
        assert_eq!(after(&[human("build", ruling)]).1, ["other-stage"]);
        let ended = [record("plan", AlreadySatisfied, "c1"), human("plan", ruling)];
        assert_eq!(after(&ended), ("ended".to_string(), vec!["verdict", "other-stage"]));
    }

    #[test]
    fn a_human_sends_an_issue_to_any_stage_of_its_workflow() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let sent = [record("plan", Pass, "c1"), human("review", Stated::Sent)];
        assert_eq!(after(&sent), ("stage:review".to_string(), vec!["verdict", "sent"]));
        // From an escalation and from an end alike.
        for from in [Escalate, AlreadySatisfied] {
            let back = [record("plan", from, "c1"), human("build", Stated::Sent)];
            assert_eq!(after(&back).0, "stage:build", "{from:?}");
        }
        // The counts start over with it, as with a ruling.
        let mut rejected = vec![record("plan", Pass, "c1"), record("build", Pass, "c2")];
        rejected.extend([record("review", Reject, "c3"), human("review", Stated::Sent)]);
        let read = history(workflow, rejected.to_vec());
        assert_eq!((read.place.rework(), read.rounds.len()), (0, 0));
        // To a human's stage it waits for them; to a stage the workflow lacks, nothing is guessed.
        let waiting = history(workflow, vec![human("sign-off", Stated::Sent)]).place.position;
        assert!(matches!(waiting, Position::Waiting(_)));
        assert_eq!(
            after(&[human("deploy", Stated::Sent)]),
            ("escalating at plan: WorkflowChanged".to_string(), vec!["unknown-stage"])
        );
    }

    #[test]
    fn a_claim_with_no_stage_to_perform_is_released() {
        // Taken from a dead peer after its stage declared, or by hand: the
        // issue waits on a human, or has ended.
        let waiting =
            [record("plan", Pass, "c1"), record("build", Pass, "c2"), record("review", Pass, "c3")];
        assert_eq!(advanced(&waiting, "c9"), Advance::HandOn);
        assert_eq!(advanced(&[record("plan", AlreadySatisfied, "c1")], "c9"), Advance::HandOn);
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
        let at_build = history(workflow, vec![record("plan", Pass, "c1")]).place.position;
        let carried = labels(&["flow:feature", "stage:plan", "stage:review", "bug"]);
        let moves = label_moves(&carried, at_build);
        assert_eq!(moves.add.as_deref(), Some("stage:build"));
        assert_eq!(moves.remove, ["stage:plan", "stage:review"]);
        let settled = label_moves(&labels(&["flow:feature", "stage:build"]), at_build);
        assert_eq!(settled, LabelMoves { add: None, remove: vec![] });
        let ended = history(workflow, vec![record("plan", AlreadySatisfied, "c1")]).place.position;
        let moves = label_moves(&labels(&["stage:plan"]), ended);
        assert_eq!(moves, LabelMoves { add: None, remove: vec!["stage:plan".to_string()] });
    }
}

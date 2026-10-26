//! The stock consult (docs/adr/0018): afkconvoy's own, asked before a human
//! wherever nothing else would be — at a stage of a bound repo that names no
//! consult, and in an unbound repo. It carries no opinion of how a product is
//! built and no rule per kind of failure: its whole instruction is to stand
//! where the human would.

use crate::record;

/// The stock consult's whole instruction: the rules of the directory its
/// worker starts in.
pub const INSTRUCTION: &str = "\
# The stock consult

You stand where a human would stand when afkconvoy hands them an issue, and nobody checks what you
decide after you: it is acted on as written. So decide what that human would, had they seen what
you saw. Find out what is true yourself: what you were told is a claim. Look for the cause, not
the symptom, and answer the cause so that it does not come back. Keep to what was decided before.
Say what is to be done so that it needs no question, and leave behind nothing your looking wrote.
Your brief says what you were handed and the commands a human answers with.
";

/// The role the stock consult's worker is started as, in a bound repo: no
/// role of any team.
pub const ROLE: &str = "stock-consult";
/// The skill it performs: answering what it was handed.
pub const SKILL: &str = "consult";

/// An unbound consult's park, shown on GitHub until the ready label is restored.
pub const PARKED_LABEL: &str = "convoy:parked";

/// Whether an unbound issue is parked for evidence. Restoring the ready label
/// brings it back; an escalation after that is a new hand-off, even if the
/// human left the old park label on the issue.
pub fn parked(labels: &[String], ready: &str, escalate: &str) -> bool {
    labels.iter().any(|label| label == PARKED_LABEL)
        && !labels.iter().any(|label| label == ready || label == escalate)
}

/// The word of the marker the fleet leaves on an unbound repo's issue as it
/// asks the stock consult of the issue's escalation.
const ASKED: &str = "convoy:consult";
/// The word of an escalation's marker (`transition::escalation_comment`).
const ESCALATION: &str = "convoy:escalation";

/// The comment the fleet leaves on an unbound repo's escalated issue as it
/// asks the stock consult of it under the claim `claim`.
pub fn asked_comment(claim: &str) -> String {
    let marker = record::marker(ASKED, &[("claim", claim)]);
    format!(
        "{marker}\n**afkconvoy** — the stock consult is asked of this escalation before a human \
         is.\n"
    )
}

/// What one comment of an unbound repo's issue says of its hand-offs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Said {
    /// An escalation, under this claim.
    Escalated(String, bool),
    /// The stock consult was asked, under this claim.
    Asked(String),
}

fn said<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Vec<Said> {
    let claim = |fields: record::Fields| fields.get("claim").unwrap_or_default().to_string();
    bodies
        .into_iter()
        .filter_map(|body| {
            if let Some(fields) = record::read_marker(ESCALATION, body) {
                return Some(Said::Escalated(
                    claim(fields),
                    crate::transition::skips_consult(body),
                ));
            }
            record::read_marker(ASKED, body).map(|fields| Said::Asked(claim(fields)))
        })
        .collect()
}

/// Whether the stock consult is to be asked of an unbound repo's escalated
/// issue, by its comments (`bodies`, oldest first): it was escalated, under
/// no consult's claim — what a consult let through, or its own failure, is a
/// human's — and is no fleet fault, no consult was asked of that escalation yet, and the hand-off
/// before it was not one a consult answered: the same again goes to a human.
pub fn due<'a>(bodies: impl IntoIterator<Item = &'a str>) -> bool {
    let said = said(bodies);
    let escalated = |claim: &str| {
        said.iter().any(|said| matches!(said, Said::Escalated(under, _) if under == claim))
    };
    // Read in order: the latest escalation, whether a consult was asked
    // since it, and whether one that escalated nothing itself answered the
    // escalation before it.
    let (mut last, mut asked_since, mut answered, mut answered_before) =
        (None, false, false, false);
    for said in &said {
        match said {
            Said::Escalated(claim, fault) => {
                (last, asked_since, answered_before) = (Some((claim, fault)), false, answered);
                answered = false;
            }
            Said::Asked(claim) => {
                asked_since = true;
                answered = answered || !escalated(claim);
            }
        }
    }
    let Some((last, fault)) = last else { return false };
    // What a consult let through, or its own failure, is escalated under its claim.
    let consults = said.contains(&Said::Asked(last.clone()));
    !fault && !consults && !asked_since && !answered_before
}

/// Whether `claim` is the stock consult's on an unbound repo's issue, by its
/// comments (`bodies`, oldest first): the fleet asked it under that claim.
pub fn asked_under<'a>(bodies: impl IntoIterator<Item = &'a str>, claim: &str) -> bool {
    asked_at(bodies, claim).is_some()
}

/// Which of an issue's comments (`bodies`, oldest first) asked the stock
/// consult under `claim`; `None` where none did. What it answers is said
/// after it.
pub fn asked_at<'a>(bodies: impl IntoIterator<Item = &'a str>, claim: &str) -> Option<usize> {
    bodies.into_iter().position(|body| {
        record::read_marker(ASKED, body).is_some_and(|fields| fields.get("claim") == Some(claim))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition::FleetFault;
    use crate::transition::escalation_comment;

    #[test]
    fn the_instruction_names_no_kind_of_failure_and_suggests_no_answer() {
        let text = INSTRUCTION.to_lowercase();
        let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).collect();
        let failures = [
            "silent", "breaker", "rework", "commit", "gate", "red", "blocked", "verdict", "stage",
            "conflict", "disk", "timeout", "workflow", "crash", "failure", "failed",
        ];
        let answers = ["rule", "send", "close", "label", "through", "escalate", "open", "retry"];
        for word in failures.iter().chain(&answers) {
            assert!(!words.contains(word), "the instruction says {word:?}:\n{INSTRUCTION}");
        }
        assert!(INSTRUCTION.lines().count() <= 8, "one short text");
        assert!(text.contains("where a human would stand"), "{INSTRUCTION}");
    }

    #[test]
    fn an_escalation_goes_to_the_stock_consult_once_and_the_same_again_to_a_human() {
        let escalated = |claim: &str| escalation_comment(claim, None, "its worker went silent");
        let (e1, asked, e2) = (escalated("c1"), asked_comment("k1"), escalated("c2"));
        assert!(!due(["no marker"]), "never escalated");
        assert!(due([e1.as_str()]));
        assert!(asked_under([e1.as_str(), asked.as_str()], "k1"));
        assert!(!asked_under([e1.as_str(), asked.as_str()], "c1"));
        // Asked: not again.
        assert!(!due([e1.as_str(), asked.as_str()]));
        // It answered — put the ready label back — and the next hand-off is a human's.
        assert!(!due([e1.as_str(), asked.as_str(), e2.as_str()]));
        // What it let through, or its own failure, is escalated under its claim: a human's.
        let through = escalated("k1");
        assert!(!due([e1.as_str(), asked.as_str(), through.as_str()]));
        // A human answered that; the next hand-off is the consult's again.
        assert!(due([e1.as_str(), asked.as_str(), through.as_str(), e2.as_str()]));
    }

    #[test]
    fn a_fleet_fault_skips_the_consult_without_consuming_a_later_work_hand_off() {
        let work = escalation_comment("work", None, "The schema is ambiguous.");
        for fault in
            [FleetFault::WorkerStart, FleetFault::WorktreeTrust, FleetFault::TrainAbandoned]
        {
            let faulted = fault.comment("fault", None, "Evidence from the host.");
            assert_eq!(FleetFault::read(&faulted), Some(fault));
            assert!(!due([faulted.as_str()]));
            assert!(!due([work.as_str(), faulted.as_str()]));
            assert!(due([faulted.as_str(), work.as_str()]));
            let quoted = escalation_comment("work", None, &format!("Earlier evidence:\n{faulted}"));
            assert_eq!(FleetFault::read(&quoted), None, "only the escalation's own marker");
            assert!(due([quoted.as_str()]));
        }
    }
}

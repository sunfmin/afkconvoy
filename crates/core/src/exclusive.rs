//! The host's exclusive queue as the fleet holds it (docs/adr/0019): the
//! queue's rules are the gate crate's (`afkconvoy_gate::exclusive`); a
//! fleet's ticket notes besides the issue of the worker that ran it and the
//! display copy of a landing turn's candidate, which no rule reads.

use serde::{Deserialize, Serialize};

pub use afkconvoy_gate::exclusive::*;

/// One `exclusive` command in the queue, as a fleet's ticket says it.
pub type Ticket = afkconvoy_gate::exclusive::Ticket<Note>;
/// A fleet's ticket at one observation time.
pub type Observed = afkconvoy_gate::exclusive::Observed<Note>;
/// The fleet's queue at one observation time.
pub type Snapshot = afkconvoy_gate::exclusive::Snapshot<Note>;

/// What a fleet notes of a ticket beside what the queue reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// The issue of the worker that ran it, where its environment says one.
    pub issue: Option<u64>,
    /// A landing turn's candidate, as the turn last reported it: a display
    /// copy, which authorizes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<crate::candidate::Validation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_reads_as_it_was_written_with_its_issue_among_its_fields() {
        let older = r#"{"number":1,"pid":2,"command":"cargo test","dir":"/a","issue":null,"asked":100,"ran":null}"#;
        let ticket: Ticket = serde_json::from_str(older).unwrap();
        assert_eq!((ticket.number, ticket.note.issue), (1, None));
        let issued = r#"{"number":1,"pid":2,"command":"cargo test","dir":"/a","issue":7,"asked":100,"ran":null}"#;
        let ticket: Ticket = serde_json::from_str(issued).unwrap();
        assert_eq!(ticket.note.issue, Some(7));
        let written = serde_json::to_value(&ticket).unwrap();
        assert_eq!(written["issue"], 7);
        assert!(written.get("validation").is_none() && written.get("note").is_none());
    }
}

//! The candidate chain as the fleet keeps it (docs/adr/0060): the chain's
//! rules are the landing crate's (`afkconvoy_landing::chain`); a fleet's
//! candidate notes besides the issue its PR completes, which no rule reads,
//! and a turn words what the chain decided as its ending.

use serde::{Deserialize, Serialize};

pub use afkconvoy_landing::chain::{
    Barrier, Decision, Evidence, Observed, Overloaded, Phase, Process, Publication, Reused, Slow,
    Status, Validation,
};

use crate::ending::Ending;

/// One merge candidate, as a fleet's chain records it.
pub type Candidate = afkconvoy_landing::chain::Candidate<Note>;
/// The candidate chain of one target branch, as a fleet records it.
pub type Chain = afkconvoy_landing::chain::Chain<Note>;

/// What a fleet notes of a candidate beside what the chain reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// The issue of the fleet's tracker its PR completes: none of a watch's
    /// or a cut's PR, nor of another tracker's issue.
    pub issue: Option<u64>,
}

/// The ending a turn answers for what the chain decided of its candidate.
pub fn ending(decision: Decision) -> Ending {
    match decision {
        Decision::Landed => Ending::Merged,
        Decision::Red => Ending::GateRed,
        Decision::Unsettled => Ending::GateUnsettled,
        Decision::RoundRed => Ending::RoundRed,
        Decision::TargetMoved => Ending::TargetMoved,
        Decision::Cancelled => Ending::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn's ending is what the chain decided of its candidate, mapped
    /// in one place (sunfmin/control#620).
    #[test]
    fn a_turn_s_ending_is_the_chain_s_decision_of_its_candidate() {
        let endings: Vec<Ending> = [
            Decision::Landed,
            Decision::Red,
            Decision::Unsettled,
            Decision::RoundRed,
            Decision::TargetMoved,
            Decision::Cancelled,
        ]
        .map(ending)
        .to_vec();
        assert_eq!(
            endings,
            [
                Ending::Merged,
                Ending::GateRed,
                Ending::GateUnsettled,
                Ending::RoundRed,
                Ending::TargetMoved,
                Ending::Cancelled
            ]
        );
    }

    /// A candidate an earlier afkconvoy wrote reads back, its issue among
    /// its own fields, and is written so again.
    #[test]
    fn a_candidate_reads_as_it_was_written_with_its_issue_among_its_fields() {
        let kept = serde_json::json!({
            "id": "c0", "pr": 4, "issue": 7, "branch": "b", "original_head": "h",
            "predecessor": "base", "commit": "m", "tree": "t", "command": "fast",
            "seconds": 60, "phase": "green",
            "owner": {"host": "h", "pid": 1, "started": "s"}, "gate": null,
            "descendants": [], "worktree": "w", "barrier": false,
            "cancelled_by": null, "cause": null, "stopped": true, "result": null,
        });
        let c: Candidate = serde_json::from_value(kept).unwrap();
        assert_eq!((c.change, c.note.issue), (4, Some(7)));
        let written = serde_json::to_value(&c).unwrap();
        assert_eq!((written["pr"].clone(), written["issue"].clone()), (4.into(), 7.into()));
        assert!(written.get("note").is_none() && written.get("change").is_none());
    }
}

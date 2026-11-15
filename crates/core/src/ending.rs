//! How one turn ends: the word its answer's `outcome` carries.
//!
//! Each word is made here and means one situation. The land step a worker is
//! told and the help of `land` both say each word as `told` has it, so what a
//! turn can answer and what its reader is taught are the same list.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    Merged,
    Conflict,
    GateRed,
    GateUnsettled,
    BaseRed,
    BaseUncertified,
    TargetMoved,
    Cancelled,
    RoundRed,
    CandidateActive,
    SpeculativeConflict,
    GateLaneChanged,
    WaitingForCapacity,
    Blocked,
    Locked,
}

impl Ending {
    /// Every ending, in the order a reader is told them.
    pub const ALL: [Ending; 15] = [
        Ending::Merged,
        Ending::Conflict,
        Ending::GateRed,
        Ending::GateUnsettled,
        Ending::BaseRed,
        Ending::BaseUncertified,
        Ending::TargetMoved,
        Ending::Cancelled,
        Ending::RoundRed,
        Ending::CandidateActive,
        Ending::SpeculativeConflict,
        Ending::GateLaneChanged,
        Ending::WaitingForCapacity,
        Ending::Blocked,
        Ending::Locked,
    ];

    /// The word a turn's answer carries as `outcome`.
    pub fn word(self) -> &'static str {
        match self {
            Ending::Merged => "merged",
            Ending::Conflict => "conflict",
            Ending::GateRed => "gate_red",
            Ending::GateUnsettled => "gate_unsettled",
            Ending::BaseRed => "base_red",
            Ending::BaseUncertified => "base_uncertified",
            Ending::TargetMoved => "target_moved",
            Ending::Cancelled => "cancelled",
            Ending::RoundRed => "round_red",
            Ending::CandidateActive => "candidate_active",
            Ending::SpeculativeConflict => "speculative_conflict",
            Ending::GateLaneChanged => "gate_lane_changed",
            Ending::WaitingForCapacity => "waiting_for_capacity",
            Ending::Blocked => "blocked",
            Ending::Locked => "locked",
        }
    }

    /// The one situation the word means, and what its reader does then.
    pub fn told(self) -> &'static str {
        match self {
            Ending::Merged => {
                "the candidate landed on the target branch, or the PR was already on it: wake the fleet and stop"
            }
            Ending::Conflict => {
                "merging onto the target tip conflicts; no gate ran and nothing landed. `files` lists the conflicts and `worktree` holds the merge in progress: resolve every listed file, `git add` it, commit the merge and run this again"
            }
            Ending::GateRed => {
                "the candidate gated red, or green without a written record (`detail` says what prevented recording); nothing landed: fix your change, commit and run this again. Where the slow gate ran red on a round at your candidate, `round` lists its PRs and issues and your branch already holds the round: do not split it; read those issues' requirements and the failing tests, fix the round, commit and run this again"
            }
            Ending::GateUnsettled => {
                "the candidate's gate, fast or slow, ran red only on TIMEOUTs it cannot classify on an overloaded host (`detail` names them and the load); that says nothing of your change, nothing landed and nothing is recorded red: ask for a turn again"
            }
            Ending::BaseRed => {
                "the target tip has no recorded green run and gated red alone; nothing of this PR ran: ask for a turn again once the fleet's repair of that tip has landed"
            }
            Ending::BaseUncertified => {
                "the target tip has no recorded green run yet: it moved before a candidate was assigned, or gated alone red only on TIMEOUTs it cannot classify on an overloaded host, which records it neither green nor red (`detail` says which); nothing of this PR ran: ask for a turn again, which gates that tip alone first"
            }
            Ending::TargetMoved => {
                "the target branch moved outside the candidate chain before this candidate landed, and its fast-forward was rejected or the move invalidated it; nothing landed: ask for a turn again on the new base tip"
            }
            Ending::Cancelled => {
                "a predecessor or other evidence this candidate depended on was invalidated; `cancelled_by` names the root candidate and `cause` why. Your source branch is preserved and this is not your gate failing: ask for another turn when your issue's blockers permit"
            }
            Ending::RoundRed => {
                "the slow gate ran red on the round this candidate was fast-green in, at the round's tip, PR `round_pr`; nothing landed and the round is not split. Your change is in that PR, whose worker fixes the round: stop; your issue is settled when that PR lands"
            }
            Ending::CandidateActive => {
                "this PR already has an active candidate of another turn, which alone validates and lands it; nothing ran: ask for a turn again once that turn has ended; no gate failure or rework occurred"
            }
            Ending::SpeculativeConflict => {
                "merging onto the candidate ahead of this PR conflicts; no gate ran and nothing landed. `files` lists the conflicts and `worktree` holds that candidate's merge into your branch in progress: resolve every listed file, `git add` it, commit the merge and run this again; your PR then carries that candidate's change"
            }
            Ending::GateLaneChanged => {
                "the candidate ahead of this PR changes the gate to one that runs in a worker lane, which this turn did not take; nothing ran: ask for a turn again after that predecessor lands; no gate failure or rework occurred"
            }
            Ending::WaitingForCapacity => {
                "the bounded candidate window was full; nothing ran: ask for a turn again; no gate failure or rework occurred"
            }
            Ending::Blocked => {
                "`blocked_by` lists the issue dependencies still open; nothing landed: the fleet does not ask again while they all stay open and the issue's blocked-by list is unchanged; it asks again once one closes or the list changes"
            }
            Ending::Locked => {
                "a fleet lock stands, named by `locks`; nothing landed: ask for a turn again once no lock stands"
            }
        }
    }

    /// Whether the ending only asks that the turn be asked for again: nothing
    /// in the tree is to change, and the fleet asks for it itself. A
    /// speculative conflict leaves a merge in the worker's tree to resolve.
    /// A fleet lock is a wait too: a pass asks after no landing the lock keeps
    /// it off, so it asks again only once the lock is gone.
    pub fn waits(self) -> bool {
        !matches!(
            self,
            Ending::Merged | Ending::Conflict | Ending::SpeculativeConflict | Ending::GateRed
        )
    }

    /// One line per ending, each as `line` writes its word and what it says.
    pub fn listed(line: impl Fn(&str, &str) -> String) -> String {
        Ending::ALL.iter().map(|e| line(e.word(), e.told())).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The word is the ending's serialized name, and no two endings share it.
    #[test]
    fn each_ending_has_its_own_word() {
        for (i, e) in Ending::ALL.iter().enumerate() {
            assert_eq!(serde_json::to_value(e).unwrap(), e.word());
            assert!(Ending::ALL[..i].iter().all(|o| o.word() != e.word()), "{}", e.word());
        }
    }
}

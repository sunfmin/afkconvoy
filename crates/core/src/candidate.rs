//! Immutable merge candidates, their validation, and ordered publication.
//!
//! The remote holds one ordered record. Assignment and every transition use
//! compare-and-swap; publication changes that record and the base atomically.
//! Source heads never move. Invalidation is permanent and precedes signalling
//! processes. A cancelled gate keeps its lane until its descendants are gone.
//! A green successor waits on its exact predecessor, without another gate.
use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Building,
    Validating,
    Green,
    Red,
    Cancelling,
    Cancelled,
    Landed,
}

/// A kernel process identity, including its start time to exclude PID reuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub host: String,
    pub pid: u32,
    pub started: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub pr: u64,
    pub issue: Option<u64>,
    pub branch: String,
    pub original_head: String,
    pub predecessor: String,
    pub commit: String,
    pub tree: String,
    pub command: String,
    pub seconds: u64,
    pub phase: Phase,
    pub owner: Process,
    pub gate: Option<Process>,
    pub worktree: String,
    /// Cuts, watches, certification and fleet-lock repairs are barriers.
    pub barrier: bool,
    /// A cancelled candidate names the root that invalidated it and why.
    pub cancelled_by: Option<String>,
    pub cause: Option<String>,
    /// True only after the gate group and descendants have been observed gone.
    pub stopped: bool,
    /// Actual gate evidence survives invalidation and owner death.
    pub result: Option<serde_json::Value>,
}

impl Candidate {
    pub fn active(&self) -> bool {
        matches!(self.phase, Phase::Building | Phase::Validating | Phase::Green) || !self.stopped
    }

    pub fn valid(&self) -> bool {
        matches!(self.phase, Phase::Building | Phase::Validating | Phase::Green)
            && self.cancelled_by.is_none()
    }

    pub fn waiting(&self) -> &'static str {
        match self.phase {
            Phase::Building => "waiting_for_capacity",
            Phase::Validating => "validating",
            Phase::Green => "waiting_for_predecessor",
            Phase::Cancelling => "confirming_process_exit",
            Phase::Cancelled => "cancelled",
            Phase::Red => "gate_red",
            Phase::Landed => "landed",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub candidates: Vec<Candidate>,
}

impl Chain {
    /// A bounded speculative window includes green waiters and unconfirmed
    /// cancellation. A barrier drains the window and admits no successors.
    pub fn can_assign(&self, limit: u64, barrier: bool) -> bool {
        let active: Vec<_> = self.candidates.iter().filter(|c| c.active()).collect();
        (active.len() as u64) < limit
            && (!barrier || active.is_empty())
            && !active.iter().any(|c| c.barrier || !c.valid())
    }

    pub fn predecessor(&self, base: &str) -> String {
        self.candidates
            .iter()
            .rev()
            .find(|c| c.valid())
            .map_or_else(|| base.to_string(), |c| c.commit.clone())
    }

    /// Permanently invalidates the root and its transitive suffix in ONE
    /// transition. Late green completion cannot restore an invalid candidate.
    /// A red root keeps its own failure, distinct from dependency cancellation.
    pub fn invalidate(&mut self, root: &str, cause: &str) {
        let mut invalid = vec![root.to_string()];
        for c in &mut self.candidates {
            if c.phase == Phase::Landed {
                continue;
            }
            if c.id == root || invalid.contains(&c.predecessor) {
                invalid.push(c.commit.clone());
                c.cancelled_by = Some(root.to_string());
                c.cause = Some(cause.to_string());
                if c.phase != Phase::Red {
                    c.phase = if c.stopped { Phase::Cancelled } else { Phase::Cancelling };
                }
            }
        }
    }

    pub fn finish(&mut self, id: &str, green: bool, result: serde_json::Value) {
        let mut failed = false;
        for c in &mut self.candidates {
            if c.id == id && c.valid() {
                c.result = Some(result.clone());
                c.phase = if green { Phase::Green } else { Phase::Red };
                failed = !green;
            }
        }
        if failed {
            self.invalidate(id, "predecessor_failed");
        }
    }

    /// An ordinary advance to a known candidate preserves its successors.
    /// An external base move invalidates all candidates rooted elsewhere.
    /// This also recognizes a push whose process died before settlement.
    pub fn observe_base(&mut self, base: &str) {
        let landed = self.candidates.iter().position(|c| c.commit == base && c.valid());
        if let Some(last) = landed {
            for c in self.candidates.iter_mut().take(last + 1) {
                if c.valid() {
                    c.phase = Phase::Landed;
                }
            }
        }
        let first = self.candidates.iter().find(|c| c.valid());
        if let Some(c) = first
            && c.predecessor != base
        {
            let id = c.id.clone();
            self.invalidate(&id, "base_moved");
        }
    }
}

/// The effect boundary must supply each observation anew. Flux checks that
/// publication cannot be authorized with any missing observation.
#[derive(Debug)]
#[refined_by(valid: bool, green: bool, predecessor: bool, unlocked: bool, head: bool)]
#[invariant(valid && green && predecessor && unlocked && head)]
pub struct Publication {
    #[field(bool[valid])]
    _valid: bool,
    #[field(bool[green])]
    _green: bool,
    #[field(bool[predecessor])]
    _predecessor: bool,
    #[field(bool[unlocked])]
    _unlocked: bool,
    #[field(bool[head])]
    _head: bool,
}

impl Publication {
    pub fn new(
        valid: bool,
        green: bool,
        predecessor: bool,
        unlocked: bool,
        head: bool,
    ) -> Option<Self> {
        if valid && green && predecessor && unlocked && head {
            Some(Self {
                _valid: valid,
                _green: green,
                _predecessor: predecessor,
                _unlocked: unlocked,
                _head: head,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};

    fn candidate(n: usize) -> Candidate {
        Candidate {
            id: format!("c{n}"),
            pr: n as u64,
            issue: Some(n as u64),
            branch: format!("b{n}"),
            original_head: format!("h{n}"),
            predecessor: if n == 0 { "base".into() } else { format!("m{}", n - 1) },
            commit: format!("m{n}"),
            tree: format!("t{n}"),
            command: "true".into(),
            seconds: 60,
            phase: Phase::Validating,
            owner: Process { host: "h".into(), pid: 1, started: "s".into() },
            gate: None,
            worktree: format!("w{n}"),
            barrier: false,
            cancelled_by: None,
            cause: None,
            stopped: false,
            result: None,
        }
    }

    #[test]
    fn exhaust_three_candidates_through_completion_invalidation_exit_and_publication() {
        // Exactly three assigned candidates, two results per gate, cancellation
        // and exit confirmations, external moves and publication. Enumerates
        // every reachable state, not arbitrary graph sizes or OS behaviour.
        let initial = (Chain { candidates: (0..3).map(candidate).collect() }, "base".to_string());
        let mut pending = VecDeque::from([initial]);
        let mut seen = HashSet::new();
        while let Some((chain, base)) = pending.pop_front() {
            let key = serde_json::to_string(&(&chain, &base)).unwrap();
            if !seen.insert(key) {
                continue;
            }
            assert!(seen.len() < 100_000, "the exploration bound changed");
            for c in &chain.candidates {
                if c.cancelled_by.is_some() {
                    assert!(!c.valid());
                }
                if c.phase == Phase::Landed {
                    assert!(c.result.is_some());
                }
            }
            for i in 0..3 {
                let c = &chain.candidates[i];
                if c.phase == Phase::Validating {
                    for green in [true, false] {
                        let mut next = chain.clone();
                        next.finish(&c.id, green, serde_json::json!({"green": green}));
                        pending.push_back((next, base.clone()));
                    }
                }
                if !c.stopped {
                    let mut next = chain.clone();
                    next.candidates[i].stopped = true;
                    if c.phase == Phase::Cancelling {
                        next.candidates[i].phase = Phase::Cancelled;
                    }
                    pending.push_back((next, base.clone()));
                }
                let publish = Publication::new(
                    c.valid(),
                    c.phase == Phase::Green,
                    c.predecessor == base,
                    true,
                    true,
                );
                if publish.is_some() {
                    assert!(c.cancelled_by.is_none());
                    let mut next = chain.clone();
                    next.candidates[i].phase = Phase::Landed;
                    pending.push_back((next, c.commit.clone()));
                }
                if c.valid() {
                    let mut next = chain.clone();
                    next.invalidate(&c.id, "head_changed");
                    pending.push_back((next, base.clone()));
                }
            }
            if base != "outside" {
                let mut next = chain.clone();
                next.observe_base("outside");
                pending.push_back((next, "outside".into()));
            }
        }
        assert!(seen.len() > 500, "exploration stopped exercising interleavings: {}", seen.len());
    }
}

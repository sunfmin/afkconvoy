//! A turn's rules, which every runner of one follows: afkconvoy's, with its
//! lanes and process groups, and the plain one (`land`). What a candidate's
//! commit gates, where the chain is recorded, what a turn waiting on the
//! chain does next, and what one atomic publication updates.

use afkconvoy_gate::declared::Gates;
use afkconvoy_gate::layout::Layout;

use crate::chain::{Candidate, Chain, Phase, Publication, Slow};

/// The ref the candidate chain of `target` is recorded on, under `layout`:
/// one record per target branch.
pub fn record_ref(layout: Layout, target: &str) -> String {
    layout.under(&format!("candidates/{target}"))
}

/// What a candidate's commit declares of its gate: the whole gate, with its
/// limit, and where it splits it, its fast gate and its slow gate
/// (docs/adr/0047).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    pub whole: String,
    pub seconds: u64,
    pub gates: Option<Gates>,
}

/// The gate a candidate runs itself, and its limit; and the slow gate a
/// round at it runs, where its commit splits its gate (docs/adr/0048).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Own {
    pub command: String,
    pub seconds: u64,
    pub slow: Option<Slow>,
}

impl Declared {
    /// The gate a candidate of this commit runs itself: its fast gate where
    /// it declares one, and a round at it the slow one; its whole gate
    /// otherwise.
    pub fn own(&self) -> Own {
        match &self.gates {
            Some(gates) => Own {
                command: gates.fast.command.clone(),
                seconds: gates.fast.seconds,
                slow: Some(Slow {
                    command: gates.slow.command.clone(),
                    seconds: gates.slow.seconds,
                    whole: self.whole.clone(),
                }),
            },
            None => Own { command: self.whole.clone(), seconds: self.seconds, slow: None },
        }
    }
}

/// What a turn whose candidate is on the chain does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// It landed, by its own turn or with a round at a candidate behind it.
    Landed,
    /// It is no longer valid: the chain decided it (`Candidate::decision`).
    Ended,
    /// It is the round's tip: its turn runs the round's slow gate now.
    Round,
    /// It is valid and waits — for its round, or its predecessor — unless it
    /// is ripe to land on the target as it stands (`Chain::ripe`).
    Waiting,
}

impl<N: Clone> Chain<N> {
    /// Puts `candidate` on the chain, in the place its turn reserved,
    /// pending or as a barrier, which it holds no more.
    pub fn assign(&mut self, candidate: Candidate<N>) {
        let id = candidate.id.clone();
        self.candidates.push(candidate);
        self.barriers.retain(|b| b.id != id);
        self.pending.retain(|b| b.id != id);
    }

    /// What the turn of the candidate `id` does next; `None` once the chain
    /// holds no such candidate.
    pub fn next(&self, id: &str) -> Option<Next> {
        let c = self.candidates.iter().find(|c| c.id == id)?;
        Some(if c.phase == Phase::Landed {
            Next::Landed
        } else if !c.valid() {
            Next::Ended
        } else if self.round_tip().is_some_and(|tip| tip.id == id) {
            Next::Round
        } else {
            Next::Waiting
        })
    }

    /// The landing at `id` on `base` as the chain authorizes it, from what
    /// its runner observed anew just before: whether the whole gate's green
    /// run of its tree is on record, and whether every member's source head
    /// is the original head it merged. `None` where anything is missing.
    pub fn publication(
        &self,
        id: &str,
        base: &str,
        recorded: bool,
        heads: bool,
    ) -> Option<Publication> {
        let members = self.members(id);
        Publication::new(
            members.iter().all(|c| c.valid()),
            self.ripe(id, base) && recorded,
            members.first().is_some_and(|m| m.predecessor == base),
            true,
            heads,
            members.iter().all(|m| m.stopped),
        )
    }
}

/// The whole gate a landing at `c` stands on: its own gate, or its round's
/// two parts together.
pub fn whole<N>(c: &Candidate<N>) -> &str {
    c.slow.as_ref().map_or(&c.command, |slow| &slow.whole)
}

/// Whether `members`, the candidates one landing takes, fast-forward the
/// target from `base` one after another: each stands on the one before it,
/// the first on `base`, as each commit's first parent (`parent`) says.
pub fn fast_forwards<N, E>(
    members: &[&Candidate<N>],
    base: &str,
    parent: impl Fn(&str) -> Result<String, E>,
) -> Result<bool, E> {
    let mut expected = base.to_string();
    for m in members {
        if m.predecessor != expected || parent(&m.commit)? != expected {
            return Ok(false);
        }
        expected = m.commit.clone();
    }
    Ok(!members.is_empty())
}

/// One ref an atomic publication sets: from `old` to `new`, where it is
/// still `old`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub name: String,
    pub old: String,
    pub new: String,
}

/// What one publication of `members` updates, all or none: every member's
/// source branch kept at its original head, the chain's record from
/// `record.1` to `record.2`, and the target from the first member's
/// predecessor to the last member's commit.
pub fn updates<N>(
    members: &[&Candidate<N>],
    record: (&str, &str, &str),
    target: &str,
) -> Vec<Update> {
    let mut updates: Vec<Update> = members
        .iter()
        .map(|m| Update {
            name: format!("refs/heads/{}", m.branch),
            old: m.original_head.clone(),
            new: m.original_head.clone(),
        })
        .collect();
    let (name, old, new) = record;
    updates.push(Update { name: name.into(), old: old.into(), new: new.into() });
    if let (Some(first), Some(last)) = (members.first(), members.last()) {
        updates.push(Update {
            name: format!("refs/heads/{target}"),
            old: first.predecessor.clone(),
            new: last.commit.clone(),
        });
    }
    updates
}

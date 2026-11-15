//! A round's pairing tree (docs/adr/0083): the PRs that asked to land are
//! taken into a round as it starts; in the order they asked, each pairs with
//! the first later one it merges with cleanly, and each pair's merge is
//! fast-gated alone. Pairs pair the same way, level by level, fast gate
//! only; a node left without a partner at a level goes up unpaired. The
//! tree's root is merged onto the target tip, one candidate of the chain per
//! PR in the order they asked (`Chain::assign`), and the last of them runs
//! the whole gate once (`Chain::whole_tip`); green, every PR of the round
//! lands in one fast-forward.
//!
//! A pair red on its fast gate stops no other pair: its later-asked PR
//! leaves the round with that run, and the other side goes up unpaired. A
//! PR that merges cleanly with no partner, or whose pair said nothing of its
//! tree, leaves the round for the next.
//!
//! The rules are the chain's, on its one record, so every runner of a round
//! follows them: afkconvoy's turns, one per PR, each gating the pairs it is
//! the later-asked PR of; and the plain runner (`land`), which gates them
//! all.

use serde::{Deserialize, Serialize};

use crate::chain::{Candidate, Chain, Decision, Phase, Process};

/// What a turn asks to land: its change, the source its head is at
/// (`forge::Change::source`), and that head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asks {
    pub change: u64,
    pub branch: String,
    pub head: String,
}

/// One PR of a round, as its turn asked it to land.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    /// The place its turn took pending, which the round took it in by.
    pub id: String,
    pub owner: Process,
    #[serde(flatten)]
    pub asks: Asks,
    /// Why it left the round, where it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<Left>,
    /// Its candidate of the chain, merged onto the target tip as part of the
    /// round's root, once assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

/// Why a PR left its round before its root was assigned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Left {
    /// It is the later-asked PR of a pair whose fast gate ran red, the pair
    /// named: its worker is given that run.
    Red(String),
    /// It merges cleanly with no other node of its level: it waits for the
    /// next round.
    Unpaired,
    /// A pair it was of said nothing of its tree, or was cancelled, before
    /// it passed: it waits for the next round.
    Next,
    /// Its head merged onto the round's candidates, or the target tip,
    /// conflicts: its turn says so as it does of any conflict.
    Conflict,
}

/// A node of a pairing tree: a PR's head, or a pair's merge, and the PRs it
/// holds, in the order they asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub changes: Vec<u64>,
    pub commit: String,
}

/// What a pair of the tree joins: the two nodes, in asking order, the first
/// its candidate's `predecessor`, its first parent; `partner` the second's
/// commit, its second parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paired {
    /// The round it is of (`Round::id`).
    pub round: String,
    /// The level it pairs, from 0: the PRs' own heads.
    pub level: u32,
    pub first: Vec<u64>,
    pub second: Vec<u64>,
    pub partner: String,
}

impl Paired {
    /// The PRs it joins.
    pub fn joins(&self) -> impl Iterator<Item = &u64> {
        self.first.iter().chain(&self.second)
    }
}

/// A round being landed through its pairing tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Round {
    pub id: String,
    /// Its PRs, in the order they asked to land; a PR whose turn ended is
    /// no longer one.
    pub asked: Vec<Ask>,
    /// The level being paired now.
    pub level: u32,
    /// That level's nodes, in the order their first PR asked.
    pub nodes: Vec<Node>,
    /// Whether the level's nodes have been paired: its pairs are on the
    /// chain.
    pub paired: bool,
    /// The tree's root, once there is one: what lands as the round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<Node>,
}

/// What the turn of one PR of the round does next (`Chain::round_next`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundNext {
    /// Nothing of it now: another turn's gate, or pairing, runs.
    Wait,
    /// Pair the round's level (`Chain::to_pair`): any turn of it may.
    Pair,
    /// Gate the pair named: it is its later-asked PR.
    Gate(String),
    /// Assign its candidate of the round's root (`Chain::assign`): every PR
    /// of the root asked before it has its own.
    Assign,
    /// Its candidate of the chain, named: the chain decides it from here.
    Assigned(String),
    /// It left the round.
    Left(Left),
}

/// A pair the runner made of one level (`Chain::pair_level`): the indexes
/// of its two nodes in the level, its merge, and what its commit declares of
/// its fast gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub first: usize,
    pub second: usize,
    pub commit: String,
    pub tree: String,
    pub command: String,
    pub seconds: u64,
}

/// The pairs of a level of `n` nodes, in asking order: each node not yet
/// paired pairs with the first later one, not yet paired, that `clean` says
/// it merges with cleanly; one left without a partner goes up unpaired.
pub fn pairs<E>(
    n: usize,
    mut clean: impl FnMut(usize, usize) -> Result<bool, E>,
) -> Result<Vec<(usize, usize)>, E> {
    let mut taken: Vec<usize> = vec![];
    let mut pairs = vec![];
    for first in 0..n {
        if taken.contains(&first) {
            continue;
        }
        for second in first..n {
            if second == first || taken.contains(&second) {
                continue;
            }
            if clean(first, second)? {
                taken.push(first);
                taken.push(second);
                pairs.push((first, second));
                break;
            }
        }
    }
    Ok(pairs)
}

impl Round {
    /// The ask of `change`, where its turn is still in the round.
    pub fn ask(&self, change: u64) -> Option<&Ask> {
        self.asked.iter().find(|a| a.asks.change == change)
    }

    /// Where `change` asked, among the round's PRs.
    fn place(&self, change: u64) -> usize {
        self.asked.iter().position(|a| a.asks.change == change).unwrap_or(usize::MAX)
    }

    /// The PRs of the root still in the round, in the order they asked.
    fn rooted(&self) -> Vec<&Ask> {
        let Some(root) = &self.root else { return vec![] };
        let mut asks: Vec<&Ask> = root
            .changes
            .iter()
            .filter_map(|&change| self.ask(change))
            .filter(|a| a.left.is_none())
            .collect();
        asks.sort_by_key(|a| self.place(a.asks.change));
        asks
    }

    /// Whether the round's root is all assigned: it has a root, and every
    /// PR of it still in the round has its candidate of the chain.
    pub fn assigned<N>(&self, _chain: &Chain<N>) -> bool {
        self.root.is_some() && self.rooted().iter().all(|a| a.candidate.is_some())
    }
}

/// How a pair's gate ended, as the chain says it.
enum Gated {
    Gating,
    Green,
    Red,
    /// It said nothing of its tree, or was cancelled.
    Nothing,
}

fn gated<N>(c: &Candidate<N>) -> Gated {
    let decided = c.decision();
    if decided.is_none() {
        return if c.phase == Phase::Green { Gated::Green } else { Gated::Gating };
    }
    if decided == Some(Decision::Red) { Gated::Red } else { Gated::Nothing }
}

impl<N: Clone> Chain<N> {
    /// Opens a round, where none is open, of every pending turn that says
    /// what it asks to land, in the order they took their places, named
    /// `id` → whether it did. A barrier, on the chain or waiting, opens none:
    /// it drains the chain first; nor does a candidate of the chain whose
    /// gate runs.
    pub fn open_round(&mut self, id: &str) -> bool {
        let gating = self.chained().into_iter().any(|c| {
            c.valid() && (c.barrier || matches!(c.phase, Phase::Validating | Phase::Slow))
        });
        if self.round.is_some() || !self.barriers.is_empty() || gating {
            return false;
        }
        let mut asked: Vec<Ask> = vec![];
        let mut kept = vec![];
        for place in self.pending.drain(..) {
            match place.asks.clone() {
                Some(asks) if !asked.iter().any(|a| a.asks.change == asks.change) => {
                    asked.push(Ask {
                        id: place.id,
                        owner: place.owner,
                        asks,
                        left: None,
                        candidate: None,
                    });
                }
                _ => kept.push(place),
            }
        }
        self.pending = kept;
        if asked.is_empty() {
            return false;
        }
        let nodes = asked
            .iter()
            .map(|a| Node { changes: vec![a.asks.change], commit: a.asks.head.clone() })
            .collect();
        self.round =
            Some(Round { id: id.to_string(), asked, level: 0, nodes, paired: false, root: None });
        self.advance_round();
        true
    }

    /// The level the round pairs next, where it has one to pair: the
    /// round's id, the level, and its nodes, two or more. A runner merges
    /// them (`pairs`) and gives the pairs it made to `pair_level`.
    pub fn to_pair(&self) -> Option<(String, u32, Vec<Node>)> {
        let round = self.round.as_ref()?;
        let pairing = round.root.is_none() && !round.paired && round.nodes.len() >= 2;
        pairing.then(|| (round.id.clone(), round.level, round.nodes.clone()))
    }

    /// The pairs `built` of the round `round`'s level `level`, as its
    /// runner made them: each a candidate beside the chain, of its
    /// later-asked PR, whose turn gates it, its note `note` and its worktree
    /// at `worktree`, by its id. None made, and two nodes or more: they
    /// merge with no partner, and the first is the root; the rest leave the
    /// round for the next. A level already paired, or another round's, is
    /// left as it is.
    pub fn pair_level(
        &mut self,
        (round, level): (&str, u32),
        built: Vec<Built>,
        note: impl Fn(u64) -> N,
        worktree: impl Fn(&str) -> String,
        now: u64,
    ) {
        let Some(r) = self.round.as_mut() else { return };
        if r.id != round || r.level != level || r.paired || r.root.is_some() {
            return;
        }
        r.paired = true;
        if built.is_empty() {
            let mut nodes = r.nodes.clone().into_iter();
            r.root = nodes.next();
            let rest: Vec<u64> = nodes.flat_map(|n| n.changes).collect();
            for ask in r.asked.iter_mut().filter(|a| rest.contains(&a.asks.change)) {
                ask.left = Some(Left::Unpaired);
            }
            return;
        }
        let r = r.clone();
        for b in built {
            let (Some(first), Some(second)) = (r.nodes.get(b.first), r.nodes.get(b.second)) else {
                continue;
            };
            let mut joins: Vec<u64> =
                first.changes.iter().chain(&second.changes).copied().collect();
            joins.sort_by_key(|&change| r.place(change));
            let Some(owner) = joins.last().and_then(|&change| r.ask(change)) else { continue };
            let id = self.unused_id(format!(
                "{}-{level}-{}",
                r.id,
                b.commit.get(..12).unwrap_or(&b.commit)
            ));
            self.candidates.push(Candidate {
                id: id.clone(),
                change: owner.asks.change,
                note: note(owner.asks.change),
                branch: owner.asks.branch.clone(),
                original_head: owner.asks.head.clone(),
                predecessor: first.commit.clone(),
                commit: b.commit,
                tree: b.tree,
                command: b.command,
                seconds: b.seconds,
                phase: Phase::Building,
                owner: owner.owner.clone(),
                gate: None,
                descendants: vec![],
                worktree: worktree(&id),
                barrier: false,
                cancelled_by: None,
                cause: None,
                stopped: false,
                abandoned: false,
                result: None,
                slow: None,
                slowed: false,
                batch: false,
                pair: Some(Paired {
                    round: r.id.clone(),
                    level,
                    first: first.changes.clone(),
                    second: second.changes.clone(),
                    partner: second.commit.clone(),
                }),
                assigned: Some(now),
            });
        }
        self.advance_round();
    }

    /// The pairs of the round's level now.
    fn level_pairs(&self) -> Vec<&Candidate<N>> {
        let Some(round) = &self.round else { return vec![] };
        self.candidates
            .iter()
            .filter(|c| {
                c.pair.as_ref().is_some_and(|p| p.round == round.id && p.level == round.level)
            })
            .collect()
    }

    /// Carries the round on from what its pairs' gates decided: once every
    /// pair of its level is decided, the next level's nodes are what went up
    /// — each green pair's merge, each node no pair took, the earlier side
    /// of each pair red or said nothing — and what left the round is told
    /// why. A level of one node is the root.
    pub fn advance_round(&mut self) {
        loop {
            let Some(round) = self.round.as_ref() else { return };
            if round.root.is_some() {
                return;
            }
            if !round.paired {
                let Some(round) = self.round.as_mut() else { return };
                let present: Vec<u64> = round.asked.iter().map(|a| a.asks.change).collect();
                round.nodes.retain(|n| n.changes.iter().any(|c| present.contains(c)));
                if round.nodes.len() <= 1 {
                    round.root = round.nodes.first().cloned();
                }
                return;
            }
            let pairs: Vec<Candidate<N>> = self.level_pairs().into_iter().cloned().collect();
            let gating = pairs.iter().any(|p| matches!(gated(p), Gated::Gating));
            let Some(round) = self.round.as_mut() else { return };
            let mut up: Vec<Node> = vec![];
            let mut left: Vec<(u64, Left)> = vec![];
            for node in &round.nodes {
                let first =
                    pairs.iter().find(|p| p.pair.as_ref().is_some_and(|q| q.first == node.changes));
                let second = pairs
                    .iter()
                    .find(|p| p.pair.as_ref().is_some_and(|q| q.second == node.changes));
                match (first, second) {
                    (Some(p), _) | (None, Some(p)) => {
                        let Some(q) = p.pair.as_ref() else { continue };
                        let owner = p.change;
                        let mine = node.changes.contains(&owner);
                        match gated(p) {
                            Gated::Green if first.is_some() => {
                                let changes = q.joins().copied().collect::<Vec<_>>();
                                let mut changes = changes;
                                changes.sort_by_key(|&c| round.place(c));
                                up.push(Node { changes, commit: p.commit.clone() });
                            }
                            Gated::Green => {}
                            Gated::Red if mine => {
                                for &c in &node.changes {
                                    let why = if c == owner {
                                        Left::Red(p.id.clone())
                                    } else {
                                        Left::Next
                                    };
                                    left.push((c, why));
                                }
                            }
                            Gated::Nothing if mine => {
                                left.extend(node.changes.iter().map(|&c| (c, Left::Next)));
                            }
                            _ => up.push(node.clone()),
                        }
                    }
                    (None, None) => up.push(node.clone()),
                }
            }
            // What left is told at once: a red pair stops no other.
            for (change, why) in left {
                for ask in round.asked.iter_mut().filter(|a| a.asks.change == change) {
                    ask.left.get_or_insert(why.clone());
                }
            }
            if gating {
                return;
            }
            up.sort_by_key(|n| n.changes.first().map_or(usize::MAX, |&c| round.place(c)));
            round.nodes = up;
            round.level = round.level.saturating_add(1);
            round.paired = false;
        }
    }

    /// What the turn of the round's PR whose place is `id` does next; `None`
    /// where no round holds it.
    pub fn round_next(&self, id: &str) -> Option<RoundNext> {
        let round = self.round.as_ref()?;
        let ask = round.asked.iter().find(|a| a.id == id)?;
        if let Some(left) = &ask.left {
            return Some(RoundNext::Left(left.clone()));
        }
        if let Some(candidate) = &ask.candidate {
            return Some(RoundNext::Assigned(candidate.clone()));
        }
        if round.root.is_none() {
            if self.to_pair().is_some() {
                return Some(RoundNext::Pair);
            }
            let mine = self
                .level_pairs()
                .into_iter()
                .find(|p| p.change == ask.asks.change && p.phase == Phase::Building && p.valid());
            return Some(mine.map_or(RoundNext::Wait, |p| RoundNext::Gate(p.id.clone())));
        }
        let mut ready = true;
        for a in round.rooted() {
            if a.id == id {
                break;
            }
            ready = ready && a.candidate.is_some();
        }
        Some(if ready { RoundNext::Assign } else { RoundNext::Wait })
    }

    /// The round's PR whose place is `id` leaves it: `why`.
    pub fn leave(&mut self, id: &str, why: Left) {
        for ask in self.round.iter_mut().flat_map(|r| &mut r.asked).filter(|a| a.id == id) {
            ask.left.get_or_insert(why.clone());
        }
    }

    /// `candidate` is the candidate of the round's root of the PR whose
    /// place is `id`: one its runner assigned it, or the chain's valid one
    /// of its change from its head, taken up (`Chain::reuse`).
    pub fn root_assigned(&mut self, id: &str, candidate: &str) {
        for ask in self.round.iter_mut().flat_map(|r| &mut r.asked).filter(|a| a.id == id) {
            ask.candidate = Some(candidate.to_string());
        }
    }

    /// Takes every PR `gone` says of out of the round: its turn ended. Each
    /// pair it would gate and has not passed is cancelled `owner_died`, and
    /// the round carries on without it. A round no PR is in any more closes:
    /// its pairs still valid end with it, `round_ended`.
    pub fn leave_round(&mut self, gone: impl Fn(&Ask) -> bool) {
        let Some(round) = self.round.as_mut() else { return };
        let mut leaving: Vec<u64> = vec![];
        for a in &round.asked {
            if gone(a) {
                leaving.push(a.asks.change);
            }
        }
        if leaving.is_empty() && !round.asked.is_empty() {
            return;
        }
        round.asked.retain(|a| !leaving.contains(&a.asks.change));
        let id = round.id.clone();
        let closed = round.asked.is_empty();
        let mut ended: Vec<(String, &str)> = vec![];
        for c in &self.candidates {
            let Some(p) = &c.pair else { continue };
            if p.round != id || !c.valid() {
                continue;
            }
            if closed {
                ended.push((c.id.clone(), "round_ended"));
            } else if leaving.contains(&c.change) && c.phase != Phase::Green {
                ended.push((c.id.clone(), "owner_died"));
            }
        }
        for (pair, cause) in ended {
            self.invalidate(&pair, cause);
        }
        if closed {
            self.round = None;
        } else {
            self.advance_round();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Barrier, Evidence, Plain, Status};

    fn owner(n: u64) -> Process {
        Process { host: "h".into(), pid: n as u32, started: "s".into() }
    }

    /// A chain whose pending turns ask to land the changes 1 to `n`, in order.
    fn asking(n: u64) -> Chain {
        let mut chain = Chain::default();
        for change in 1..=n {
            chain.pending.push(Barrier {
                id: format!("t{change}"),
                owner: owner(change),
                since: Some(change),
                asks: Some(Asks {
                    change,
                    branch: format!("b{change}"),
                    head: format!("h{change}"),
                }),
            });
        }
        chain
    }

    fn ran(green: bool) -> Evidence {
        let mut evidence = Evidence::red(if green { 0 } else { 1 }, false, 1, None);
        if green {
            evidence.status = Status::Green;
            evidence.recorded = true;
        }
        evidence
    }

    /// Pairs the round's level by `clean`, each pair's merge named for the
    /// commits it joins.
    fn pair(chain: &mut Chain, clean: impl Fn(&Node, &Node) -> bool) {
        let (round, level, nodes) = chain.to_pair().expect("a level to pair");
        let made = pairs(nodes.len(), |a, b| Ok::<_, ()>(clean(&nodes[a], &nodes[b]))).unwrap();
        let built = made
            .into_iter()
            .map(|(first, second)| Built {
                first,
                second,
                commit: format!("({}+{})", nodes[first].commit, nodes[second].commit),
                tree: String::new(),
                command: "fast".into(),
                seconds: 1,
            })
            .collect();
        chain.pair_level((&round, level), built, |_| Plain::default(), |id| format!("w/{id}"), 0);
    }

    /// Gates the pair `id` of the round green or red.
    fn gate(chain: &mut Chain, id: &str, green: bool) {
        chain.finish(id, ran(green));
        chain.candidates.iter_mut().filter(|c| c.id == id).for_each(|c| c.stopped = true);
        chain.advance_round();
    }

    fn gating(chain: &Chain) -> Vec<(u64, String)> {
        let round = chain.round.as_ref().unwrap();
        round
            .asked
            .iter()
            .filter_map(|a| match chain.round_next(&a.id) {
                Some(RoundNext::Gate(pair)) => Some((a.asks.change, pair)),
                _ => None,
            })
            .collect()
    }

    fn root(chain: &Chain) -> Option<Node> {
        chain.round.as_ref().and_then(|r| r.root.clone())
    }

    #[test]
    fn each_node_pairs_with_the_first_later_one_it_merges_with_cleanly() {
        let clean = |a: usize, b: usize| Ok::<_, ()>(!(a == 0 && b == 1));
        assert_eq!(pairs(4, clean).unwrap(), [(0, 2), (1, 3)]);
        assert_eq!(pairs(5, |_, _| Ok::<_, ()>(true)).unwrap(), [(0, 1), (2, 3)]);
        assert_eq!(pairs(3, |_, _| Ok::<_, ()>(false)).unwrap(), []);
        assert_eq!(pairs(1, |_, _| Ok::<_, ()>(true)).unwrap(), []);
    }

    /// Four ready, mutually clean PRs: two pairs, gated by their later-asked
    /// PRs' turns at once, then their pair, then the root.
    #[test]
    fn four_clean_prs_pair_twice_and_their_pairs_once() {
        let mut chain = asking(4);
        assert!(chain.open_round("r"));
        assert!(chain.pending.is_empty());
        assert!(!chain.open_round("r2"), "one round at a time");
        assert_eq!(chain.round_next("t1"), Some(RoundNext::Pair));
        pair(&mut chain, |_, _| true);
        let level0 = gating(&chain);
        assert_eq!(level0.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [2, 4]);
        assert_eq!(chain.round_next("t1"), Some(RoundNext::Wait));
        gate(&mut chain, &level0[0].1, true);
        assert_eq!(chain.to_pair(), None, "the level waits for its other pair");
        gate(&mut chain, &level0[1].1, true);
        pair(&mut chain, |_, _| true);
        let level1 = gating(&chain);
        assert_eq!(level1.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [4]);
        gate(&mut chain, &level1[0].1, true);
        let root = root(&chain).expect("the root");
        assert_eq!(root.changes, [1, 2, 3, 4]);
        assert_eq!(root.commit, "((h1+h2)+(h3+h4))");
        // The root is assigned in asking order.
        assert_eq!(chain.round_next("t1"), Some(RoundNext::Assign));
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Wait));
        chain.root_assigned("t1", "c1");
        assert_eq!(chain.round_next("t1"), Some(RoundNext::Assigned("c1".into())));
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Assign));
    }

    /// A red pair stops no other: its later-asked PR leaves with the run,
    /// its earlier one goes up unpaired, and the rest of the round goes on.
    #[test]
    fn a_red_pair_sends_its_later_pr_back_and_its_earlier_one_goes_up() {
        let mut chain = asking(4);
        chain.open_round("r");
        pair(&mut chain, |_, _| true);
        let level0 = gating(&chain);
        gate(&mut chain, &level0[0].1, false);
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Left(Left::Red(level0[0].1.clone()))));
        assert_eq!(chain.round_next("t4"), Some(RoundNext::Gate(level0[1].1.clone())));
        gate(&mut chain, &level0[1].1, true);
        let nodes = chain.to_pair().unwrap().2;
        assert_eq!(
            nodes.iter().map(|n| n.changes.clone()).collect::<Vec<_>>(),
            [vec![1], vec![3, 4]]
        );
        pair(&mut chain, |_, _| true);
        let level1 = gating(&chain);
        gate(&mut chain, &level1[0].1, true);
        assert_eq!(root(&chain).unwrap().changes, [1, 3, 4]);
    }

    /// An odd PR goes up unpaired; one that merges with no node leaves the
    /// round for the next; a PR asking during a round waits for the next.
    #[test]
    fn an_odd_pr_goes_up_unpaired_and_one_clean_with_none_leaves_for_the_next_round() {
        let mut chain = asking(3);
        chain.open_round("r");
        pair(&mut chain, |_, _| true);
        let level0 = gating(&chain);
        assert_eq!(level0.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [2]);
        assert_eq!(chain.round_next("t3"), Some(RoundNext::Wait), "unpaired, it waits");
        chain.pending.push(Barrier {
            id: "t5".into(),
            owner: owner(5),
            since: Some(5),
            asks: Some(Asks { change: 5, branch: "b5".into(), head: "h5".into() }),
        });
        gate(&mut chain, &level0[0].1, true);
        // The pair and PR 3 conflict: 3 leaves for the next round.
        pair(&mut chain, |_, _| false);
        assert_eq!(root(&chain).unwrap().changes, [1, 2]);
        assert_eq!(chain.round_next("t3"), Some(RoundNext::Left(Left::Unpaired)));
        assert_eq!(chain.round_next("t5"), None, "it asked during the round");
        assert_eq!(chain.pending.len(), 1);
        // Its turns leave; the round closes, and the next one takes 5.
        chain.leave_round(|_| true);
        assert_eq!(chain.round, None);
        assert!(chain.candidates.iter().all(|c| !c.valid()), "its pairs end with it");
        assert!(chain.open_round("r2"));
        assert_eq!(root(&chain).unwrap().changes, [5], "a round of one is its own root");
    }

    /// A turn that ends takes its PR out: a pair it would gate is cancelled,
    /// and the other side goes up.
    #[test]
    fn a_pr_whose_turn_ended_leaves_and_its_pair_with_it() {
        let mut chain = asking(2);
        chain.open_round("r");
        pair(&mut chain, |_, _| true);
        let pair_id = gating(&chain)[0].1.clone();
        chain.leave_round(|a| a.asks.change == 2);
        let p = chain.candidates.iter().find(|c| c.id == pair_id).unwrap();
        assert_eq!(p.cause.as_deref(), Some("owner_died"));
        assert_eq!(root(&chain).unwrap().changes, [1]);
    }
}

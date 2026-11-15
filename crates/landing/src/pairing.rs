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
//! While the open round's whole gate runs, the PRs that ask to land do not
//! wait idle, nor interrupt it: they are a **stacked round** (`Chain::stacked`,
//! sunfmin/control#1140). Its PRs pair among themselves as any round's do,
//! and its root, merged onto the open round's root-onto-tip commit (`On`),
//! is fast-gated there: its **stacked merge**. A PR that asks later joins it
//! while it can still pair it. The open round landed, the target tip is
//! that commit: the stacked round is the open round, its root's last
//! candidate has the stacked merge's tree, whose fast gate on record answers
//! for it, and only its slow gate runs. The open round ended otherwise, the
//! stacked round is discarded: its PRs are pending again, charged nothing,
//! and pair in the next round. The running gate is never killed or
//! restarted for it.
//!
//! The rules are the chain's, on its one record, so every runner of a round
//! follows them: afkconvoy's turns, one per PR, each gating the pairs it is
//! the later-asked PR of; and the plain runner (`land`), which gates them
//! all.

use serde::{Deserialize, Serialize};

use crate::chain::{Barrier, Candidate, Chain, Decision, Phase, Process};

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

impl Node {
    /// The node of one PR's head.
    fn of(ask: &Ask) -> Node {
        Node { changes: vec![ask.asks.change], commit: ask.asks.head.clone() }
    }
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
    /// It is a stacked round's stacked merge, not a pair of its tree: the
    /// root's PRs merged one after another, in the order they asked, onto
    /// the commit the round stands on (`On::commit`), its `predecessor`;
    /// `second` the root's PRs, `partner` the root's commit, `first` none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stacked: bool,
}

impl Paired {
    /// The PRs it joins.
    pub fn joins(&self) -> impl Iterator<Item = &u64> {
        self.first.iter().chain(&self.second)
    }
}

/// What a stacked round stands on: the open round's last candidate, whose
/// whole gate runs, and its commit, the open round's root merged onto the
/// target tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct On {
    pub candidate: String,
    pub commit: String,
    /// Its root's stacked merge was made, or found to conflict.
    #[serde(default)]
    pub merged: bool,
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
    /// What a stacked round stands on; `None` of the open round, which
    /// stands on the target tip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<On>,
    /// The PRs that left it, which it does not take in again as they ask.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub out: Vec<u64>,
}

/// What the turn of one PR of the round does next (`Chain::round_next`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundNext {
    /// Nothing of it now: another turn's gate, or pairing, runs; or, of a
    /// stacked round, the open round's whole gate.
    Wait,
    /// Pair the round's level (`Chain::to_pair`): any turn of it may.
    Pair,
    /// Gate the pair named, or the stacked merge: it is its later-asked PR.
    Gate(String),
    /// Merge the stacked round's root onto what it stands on
    /// (`Chain::to_stack`, `Chain::stack_merge`): it is the root's
    /// last-asked PR.
    Stack,
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

/// A stacked merge the runner made (`Chain::stack_merge`): its last commit,
/// its tree, and what that commit declares of its fast gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub commit: String,
    pub tree: String,
    pub command: String,
    pub seconds: u64,
}

/// What a stacked round's root is merged onto, as `Chain::to_stack` names
/// it: the round's id, the commit it stands on, and its root's PRs' heads in
/// the order they asked, each merged onto the merge before.
pub type ToStack = (String, String, Vec<Asks>);

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
    /// A round named `id` of `asked`, at its first level, standing on `on`.
    fn of(id: &str, asked: Vec<Ask>, on: Option<On>) -> Round {
        let nodes = asked.iter().map(Node::of).collect();
        Round {
            id: id.to_string(),
            asked,
            level: 0,
            nodes,
            paired: false,
            root: None,
            on,
            out: vec![],
        }
    }

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

    /// Whether it has a level to pair now.
    fn pairing(&self) -> bool {
        self.root.is_none() && !self.paired && self.nodes.len() >= 2
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

/// Which of a chain's two rounds: the open one, or the one stacked on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Open,
    Stacked,
}

const SLOTS: [Slot; 2] = [Slot::Open, Slot::Stacked];

/// A candidate beside the chain, of `owner`'s PR, whose turn gates it: a
/// pair, or a stacked merge, as `pair` says, of `built`'s commit on
/// `predecessor`.
struct Beside<'a, N> {
    id: String,
    owner: &'a Ask,
    predecessor: String,
    built: (String, String, String, u64),
    note: N,
    worktree: String,
    now: u64,
    pair: Paired,
}

impl<N> Beside<'_, N> {
    fn candidate(self) -> Candidate<N> {
        let (commit, tree, command, seconds) = self.built;
        Candidate {
            id: self.id,
            change: self.owner.asks.change,
            note: self.note,
            branch: self.owner.asks.branch.clone(),
            original_head: self.owner.asks.head.clone(),
            predecessor: self.predecessor,
            commit,
            tree,
            command,
            seconds,
            phase: Phase::Building,
            owner: self.owner.owner.clone(),
            gate: None,
            descendants: vec![],
            worktree: self.worktree,
            barrier: false,
            cancelled_by: None,
            cause: None,
            stopped: false,
            abandoned: false,
            result: None,
            slow: None,
            slowed: false,
            batch: false,
            pair: Some(self.pair),
            assigned: Some(self.now),
        }
    }
}

impl<N: Clone> Chain<N> {
    fn slot(&self, slot: Slot) -> Option<&Round> {
        match slot {
            Slot::Open => self.round.as_ref(),
            Slot::Stacked => self.stacked.as_ref(),
        }
    }

    fn slot_mut(&mut self, slot: Slot) -> &mut Option<Round> {
        match slot {
            Slot::Open => &mut self.round,
            Slot::Stacked => &mut self.stacked,
        }
    }

    /// The open round, then the round stacked on it.
    pub fn rounds(&self) -> impl Iterator<Item = &Round> {
        self.round.iter().chain(&self.stacked)
    }

    /// The round whose PR's place is `id`.
    fn holding(&self, id: &str) -> Option<Slot> {
        SLOTS
            .into_iter()
            .find(|&s| self.slot(s).is_some_and(|r| r.asked.iter().any(|a| a.id == id)))
    }

    /// The round named `round`.
    fn named(&self, round: &str) -> Option<Slot> {
        SLOTS.into_iter().find(|&s| self.slot(s).is_some_and(|r| r.id == round))
    }

    /// Takes out of pending every turn that says what it asks to land, in
    /// the order they took their places, but one of a change `taken` names
    /// or another taken already → their asks.
    fn take_pending(&mut self, taken: &[u64]) -> Vec<Ask> {
        let mut asked: Vec<Ask> = vec![];
        let mut kept = vec![];
        for place in self.pending.drain(..) {
            match place.asks.clone() {
                Some(asks)
                    if !taken.contains(&asks.change)
                        && !asked.iter().any(|a| a.asks.change == asks.change) =>
                {
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
        asked
    }

    /// Opens a round, where none is open, of every pending turn that says
    /// what it asks to land, in the order they took their places, named
    /// `id` → whether it did. A barrier, on the chain or waiting, opens none:
    /// it drains the chain first; nor does a candidate of the chain whose
    /// gate runs, nor a stacked round still waiting on what it stands on.
    /// Where one is open and its whole gate runs, the pending turns stack on
    /// it instead (`stack`).
    pub fn open_round(&mut self, id: &str) -> bool {
        self.settle_stacked();
        if self.round.is_some() {
            return self.stack(id);
        }
        let gating = self.chained().into_iter().any(|c| {
            c.valid() && (c.barrier || matches!(c.phase, Phase::Validating | Phase::Slow))
        });
        if self.stacked.is_some() || !self.barriers.is_empty() || gating {
            return false;
        }
        let asked = self.take_pending(&[]);
        if asked.is_empty() {
            return false;
        }
        self.round = Some(Round::of(id, asked, None));
        self.advance_round();
        true
    }

    /// The candidate whose run of the open round's whole gate runs now: the
    /// chain's last valid candidate, of the round's root, all assigned, once
    /// its whole gate started (`start_whole`).
    pub fn whole_running(&self) -> Option<&Candidate<N>> {
        let round = self.round.as_ref()?;
        let tip = self.chained().into_iter().rev().find(|c| c.valid())?;
        (tip.batch && tip.phase == Phase::Validating && round.assigned(self)).then_some(tip)
    }

    /// Stacks the pending turns on the open round while its whole gate runs
    /// (sunfmin/control#1140) → whether any were taken: a stacked round of
    /// them, named `id`, where none is; otherwise they join the one there
    /// is, while it can pair them — as nodes of the level it pairs, that
    /// level not paired yet; or, once it has its root, a level above it, of
    /// that root and theirs, whose own stacked merge is made once it has its
    /// root again. A stacked merge of the root before that has not started
    /// ends `stack_joined`; one gating runs on, as no gate is killed for a
    /// newcomer. A PR that left the stacked round does not join it again. A
    /// barrier stacks nothing, nor does a round whose whole gate does not
    /// run.
    fn stack(&mut self, id: &str) -> bool {
        if !self.barriers.is_empty() {
            return false;
        }
        let Some(tip) = self.whole_running() else { return false };
        let on = On { candidate: tip.id.clone(), commit: tip.commit.clone(), merged: false };
        let Some(stacked) = self.stacked.clone() else {
            let asked = self.take_pending(&[]);
            if asked.is_empty() {
                return false;
            }
            self.stacked = Some(Round::of(id, asked, Some(on)));
            self.advance_round();
            return true;
        };
        if stacked.on.as_ref().is_none_or(|o| o.candidate != on.candidate) {
            return false;
        }
        if stacked.root.is_none() && stacked.paired {
            return false;
        }
        let mut merges: Vec<String> = vec![];
        for c in &self.candidates {
            let of_it = c.pair.as_ref().is_some_and(|p| p.stacked && p.round == stacked.id);
            if of_it && c.valid() && c.phase == Phase::Building {
                merges.push(c.id.clone());
            }
        }
        let mut taken = stacked.out.clone();
        taken.extend(stacked.asked.iter().map(|a| a.asks.change));
        let asked = self.take_pending(&taken);
        if asked.is_empty() {
            return false;
        }
        // A stacked merge not started is of a root it is no more.
        for merge in merges {
            self.invalidate(&merge, "stack_joined");
        }
        let Some(s) = self.stacked.as_mut() else { return false };
        if let Some(root) = s.root.take() {
            s.nodes = vec![root];
            s.level = s.level.saturating_add(1);
            if let Some(on) = s.on.as_mut() {
                on.merged = false;
            }
        }
        s.nodes.extend(asked.iter().map(Node::of));
        s.asked.extend(asked);
        s.paired = false;
        self.advance_round();
        true
    }

    /// Settles the stacked round once the open round has closed: where the
    /// candidate it stands on landed, the target tip is the commit it stood
    /// on, and it is the open round now — a stacked merge not yet gated ends
    /// `stack_promoted`, as its root's candidates are merged onto that tip
    /// with the tree it had; where that candidate ended otherwise, it is
    /// discarded: its pairs and its stacked merge end
    /// `stacked_round_discarded`, and its PRs are pending again, ahead of
    /// every other, charged nothing, for the next round. While that
    /// candidate is valid it waits.
    pub fn settle_stacked(&mut self) {
        if self.round.is_some() {
            return;
        }
        let Some(stacked) = self.stacked.as_ref() else { return };
        let stood = stacked
            .on
            .as_ref()
            .and_then(|on| self.candidates.iter().find(|c| c.id == on.candidate));
        if stood.is_some_and(|c| c.valid()) {
            return;
        }
        let landed = stood.is_some_and(|c| c.phase == Phase::Landed);
        let Some(mut round) = self.stacked.take() else { return };
        let mut ended: Vec<String> = vec![];
        for c in &self.candidates {
            let Some(p) = c.pair.as_ref().filter(|p| p.round == round.id) else { continue };
            if c.valid() && (!landed || (p.stacked && c.phase == Phase::Building)) {
                ended.push(c.id.clone());
            }
        }
        let cause = if landed { "stack_promoted" } else { "stacked_round_discarded" };
        for id in ended {
            self.invalidate(&id, cause);
        }
        if landed {
            round.on = None;
            self.round = Some(round);
            self.advance_round();
            return;
        }
        let again = round.asked.into_iter().map(|ask| Barrier {
            id: ask.id,
            owner: ask.owner,
            since: None,
            asks: Some(ask.asks),
        });
        self.pending.splice(0..0, again);
    }

    /// The level a round pairs next, where one has a level to pair — the
    /// open round's first, then the stacked one's: the round's id, the
    /// level, and its nodes, two or more. A runner merges them (`pairs`) and
    /// gives the pairs it made to `pair_level`.
    pub fn to_pair(&self) -> Option<(String, u32, Vec<Node>)> {
        let round = self.rounds().find(|r| r.pairing())?;
        Some((round.id.clone(), round.level, round.nodes.clone()))
    }

    /// The pairs `built` of the round `round`'s level `level`, as its
    /// runner made them: each a candidate beside the chain, of its
    /// later-asked PR, whose turn gates it, its note `note` and its worktree
    /// at `worktree`, by its id. None made, and two nodes or more: they
    /// merge with no partner, and the first is the root; the rest leave the
    /// round for the next. A level already paired, or of no round there is,
    /// is left as it is.
    pub fn pair_level(
        &mut self,
        (round, level): (&str, u32),
        built: Vec<Built>,
        note: impl Fn(u64) -> N,
        worktree: impl Fn(&str) -> String,
        now: u64,
    ) {
        let Some(slot) = self.named(round) else { return };
        let Some(r) = self.slot_mut(slot).as_mut() else { return };
        if r.level != level || r.paired || r.root.is_some() {
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
            let candidate = Beside {
                id: id.clone(),
                owner,
                predecessor: first.commit.clone(),
                built: (b.commit, b.tree, b.command, b.seconds),
                note: note(owner.asks.change),
                worktree: worktree(&id),
                now,
                pair: Paired {
                    round: r.id.clone(),
                    level,
                    first: first.changes.clone(),
                    second: second.changes.clone(),
                    partner: second.commit.clone(),
                    stacked: false,
                },
            }
            .candidate();
            self.candidates.push(candidate);
        }
        self.advance_round();
    }

    /// What the stacked round's root is merged onto, where its stacked merge
    /// is to be made (`ToStack`): a runner merges each head onto the merge
    /// before, from the commit it stands on, and gives the last merge to
    /// `stack_merge`.
    pub fn to_stack(&self) -> Option<ToStack> {
        let round = self.stacked.as_ref()?;
        let on = round.on.as_ref().filter(|on| !on.merged)?;
        let heads: Vec<Asks> = round.rooted().into_iter().map(|a| a.asks.clone()).collect();
        (!heads.is_empty()).then(|| (round.id.clone(), on.commit.clone(), heads))
    }

    /// The stacked merge `merged` of the stacked round `round`'s root, as
    /// its runner made it: a candidate beside the chain, of the root's
    /// last-asked PR, whose turn gates it, on the commit the round stands on.
    /// `None` where the root's heads merged onto it conflict: no stacked
    /// merge gates, and its root's candidates meet that conflict as they are
    /// merged onto the tip.
    pub fn stack_merge(
        &mut self,
        round: &str,
        merged: Option<Merged>,
        note: impl Fn(u64) -> N,
        worktree: impl Fn(&str) -> String,
        now: u64,
    ) {
        let Some(r) = self.stacked.as_mut().filter(|r| r.id == round) else { return };
        let Some(on) = r.on.as_mut().filter(|on| !on.merged) else { return };
        on.merged = true;
        let r = r.clone();
        let (Some(m), Some(root), Some(on)) = (merged, r.root.as_ref(), r.on.as_ref()) else {
            return;
        };
        let Some(owner) = r.rooted().last().copied() else { return };
        let id = self.unused_id(format!("{}-on-{}", r.id, m.commit.get(..12).unwrap_or(&m.commit)));
        let candidate = Beside {
            id: id.clone(),
            owner,
            predecessor: on.commit.clone(),
            built: (m.commit, m.tree, m.command, m.seconds),
            note: note(owner.asks.change),
            worktree: worktree(&id),
            now,
            pair: Paired {
                round: r.id.clone(),
                level: r.level,
                first: vec![],
                second: root.changes.clone(),
                partner: root.commit.clone(),
                stacked: true,
            },
        }
        .candidate();
        self.candidates.push(candidate);
    }

    /// Stops each candidate beside the chain — a pair, or a stacked merge —
    /// of `owner`'s that ended before its gate started: no process of it
    /// runs to be confirmed gone, and one cancelling would hold the window
    /// (`can_assign`). Its owner says so while it gates nothing, as a
    /// stacked round settled, or a newcomer joined it, ends what it had not
    /// started → whether any was.
    pub fn stop_unstarted(&mut self, owner: &Process) -> bool {
        let mut any = false;
        for c in self.candidates.iter_mut().filter(|c| {
            c.is_pair()
                && c.owner == *owner
                && c.phase == Phase::Cancelling
                && c.gate.is_none()
                && !c.stopped
        }) {
            c.stopped = true;
            c.phase = Phase::Cancelled;
            any = true;
        }
        any
    }

    /// The pairs of the level `slot`'s round pairs now.
    fn level_pairs(&self, slot: Slot) -> Vec<&Candidate<N>> {
        let Some(round) = self.slot(slot) else { return vec![] };
        self.candidates
            .iter()
            .filter(|c| {
                c.pair
                    .as_ref()
                    .is_some_and(|p| p.round == round.id && p.level == round.level && !p.stacked)
            })
            .collect()
    }

    /// Carries each round on from what its pairs' gates decided
    /// (`advance`).
    pub fn advance_round(&mut self) {
        for slot in SLOTS {
            self.advance(slot);
        }
    }

    /// Carries `slot`'s round on from what its pairs' gates decided: once
    /// every pair of its level is decided, the next level's nodes are what
    /// went up — each green pair's merge, each node no pair took, the
    /// earlier side of each pair red or said nothing — and what left the
    /// round is told why. A level of one node is the root.
    fn advance(&mut self, slot: Slot) {
        loop {
            let Some(round) = self.slot(slot) else { return };
            if round.root.is_some() {
                return;
            }
            if !round.paired {
                let Some(round) = self.slot_mut(slot).as_mut() else { return };
                let present: Vec<u64> = round.asked.iter().map(|a| a.asks.change).collect();
                round.nodes.retain(|n| n.changes.iter().any(|c| present.contains(c)));
                if round.nodes.len() <= 1 {
                    round.root = round.nodes.first().cloned();
                }
                return;
            }
            let pairs: Vec<Candidate<N>> = self.level_pairs(slot).into_iter().cloned().collect();
            let gating = pairs.iter().any(|p| matches!(gated(p), Gated::Gating));
            let Some(round) = self.slot_mut(slot).as_mut() else { return };
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
                                let mut changes = q.joins().copied().collect::<Vec<_>>();
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
    /// where no round, open or stacked, holds it. A stacked round's PRs,
    /// once its root is found, have its stacked merge made and gated by the
    /// root's last-asked PR, and then wait for the open round.
    pub fn round_next(&self, id: &str) -> Option<RoundNext> {
        let slot = self.holding(id)?;
        let round = self.slot(slot)?;
        let ask = round.asked.iter().find(|a| a.id == id)?;
        if let Some(left) = &ask.left {
            return Some(RoundNext::Left(left.clone()));
        }
        if let Some(candidate) = &ask.candidate {
            return Some(RoundNext::Assigned(candidate.clone()));
        }
        if round.root.is_none() {
            if round.pairing() {
                return Some(RoundNext::Pair);
            }
            let mine = self
                .level_pairs(slot)
                .into_iter()
                .find(|p| p.change == ask.asks.change && p.phase == Phase::Building && p.valid());
            return Some(mine.map_or(RoundNext::Wait, |p| RoundNext::Gate(p.id.clone())));
        }
        if let Some(on) = &round.on {
            if !on.merged {
                let last = round.rooted().last().is_some_and(|a| a.id == id);
                return Some(if last { RoundNext::Stack } else { RoundNext::Wait });
            }
            let mine = self.candidates.iter().find(|c| {
                c.pair.as_ref().is_some_and(|p| p.stacked && p.round == round.id)
                    && c.change == ask.asks.change
                    && c.phase == Phase::Building
                    && c.valid()
            });
            return Some(mine.map_or(RoundNext::Wait, |c| RoundNext::Gate(c.id.clone())));
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
        let rounds = self.round.iter_mut().chain(self.stacked.iter_mut());
        for ask in rounds.flat_map(|r| &mut r.asked).filter(|a| a.id == id) {
            ask.left.get_or_insert(why.clone());
        }
    }

    /// `candidate` is the candidate of the round's root of the PR whose
    /// place is `id`: one its runner assigned it, or the chain's valid one
    /// of its change from its head, taken up (`Chain::reuse`).
    pub fn root_assigned(&mut self, id: &str, candidate: &str) {
        let rounds = self.round.iter_mut().chain(self.stacked.iter_mut());
        for ask in rounds.flat_map(|r| &mut r.asked).filter(|a| a.id == id) {
            ask.candidate = Some(candidate.to_string());
        }
    }

    /// Takes every PR `gone` says of out of its round, open or stacked: its
    /// turn ended. Each pair it would gate and has not passed is cancelled
    /// `owner_died`, and the round carries on without it. A round no PR is
    /// in any more closes: its pairs still valid end with it, `round_ended`.
    /// The open round closed, the stacked one is settled
    /// (`settle_stacked`).
    pub fn leave_round(&mut self, gone: impl Fn(&Ask) -> bool) {
        for slot in SLOTS {
            self.leave_slot(slot, &gone);
        }
        self.settle_stacked();
    }

    fn leave_slot(&mut self, slot: Slot, gone: &impl Fn(&Ask) -> bool) {
        let Some(round) = self.slot_mut(slot).as_mut() else { return };
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
        round.out.extend(leaving.iter().copied());
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
            *self.slot_mut(slot) = None;
        } else {
            self.advance(slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Evidence, Plain, Status};

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
        chain
            .rounds()
            .flat_map(|r| &r.asked)
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

    /// Asks change `n` to land, pending.
    fn asks(chain: &mut Chain, n: u64) {
        chain.pending.push(Barrier {
            id: format!("t{n}"),
            owner: owner(n),
            since: Some(n),
            asks: Some(Asks { change: n, branch: format!("b{n}"), head: format!("h{n}") }),
        });
    }

    /// A chain whose open round, of change 1 alone, runs its whole gate at
    /// its candidate `c1`, commit `tip1`.
    fn whole_running() -> Chain {
        let mut chain = asking(1);
        assert!(chain.open_round("r"));
        let round = chain.round.clone().unwrap();
        let mut c = Beside {
            id: "c1".into(),
            owner: &round.asked[0],
            predecessor: "main".into(),
            built: ("tip1".into(), "tree1".into(), "fast".into(), 1),
            note: Plain::default(),
            worktree: "w/c1".into(),
            now: 0,
            pair: Paired {
                round: "r".into(),
                level: 0,
                first: vec![],
                second: vec![],
                partner: String::new(),
                stacked: false,
            },
        }
        .candidate();
        c.pair = None;
        c.batch = true;
        c.phase = Phase::Batched;
        c.stopped = true;
        chain.candidates.push(c);
        chain.root_assigned("t1", "c1");
        assert!(chain.whole_running().is_none(), "its whole gate has not started");
        assert!(chain.start_whole("c1"));
        assert_eq!(chain.whole_running().map(|c| c.id.as_str()), Some("c1"));
        chain
    }

    /// Makes and gates the stacked round's stacked merge `green`, as its
    /// root's last-asked PR's turn does → its id.
    fn stack_and_gate(chain: &mut Chain, green: bool) -> String {
        let last = chain.to_stack().expect("a root to stack").2.last().unwrap().change;
        make_stack(chain);
        let Some(RoundNext::Gate(id)) = chain.round_next(&format!("t{last}")) else {
            panic!("{chain:?}")
        };
        gate(chain, &id, green);
        id
    }

    /// The open round's whole gate running, the PRs ready now stack on it:
    /// they pair among themselves, their root is merged onto the round's
    /// root-onto-tip commit and fast-gated there, and nothing of the round
    /// running is touched; green and landed, the stacked round is the open
    /// round, on that commit as the tip.
    #[test]
    fn prs_ready_while_a_round_s_whole_gate_runs_stack_on_its_root_and_open_once_it_lands() {
        let mut chain = whole_running();
        asks(&mut chain, 2);
        asks(&mut chain, 3);
        assert!(chain.open_round("s"));
        assert!(chain.pending.is_empty());
        let stacked = chain.stacked.clone().expect("a stacked round");
        assert_eq!(stacked.on.as_ref().map(|on| on.commit.as_str()), Some("tip1"));
        assert_eq!(chain.round.as_ref().map(|r| r.id.as_str()), Some("r"), "the round runs on");
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Pair));
        pair(&mut chain, |_, _| true);
        let [(3, pair_id)] = &gating(&chain)[..] else { panic!("{chain:?}") };
        assert_eq!(chain.round_next("t3"), Some(RoundNext::Gate(pair_id.clone())));
        gate(&mut chain, &pair_id.clone(), true);
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Wait));
        assert_eq!(chain.round_next("t3"), Some(RoundNext::Stack), "its last-asked PR stacks");
        let merge = stack_and_gate(&mut chain, true);
        let m = chain.candidates.iter().find(|c| c.id == merge).unwrap();
        assert_eq!((m.predecessor.as_str(), m.commit.as_str()), ("tip1", "((tip1+h2)+h3)"));
        assert!(m.is_pair() && m.phase == Phase::Green);
        assert_eq!(chain.round_next("t3"), Some(RoundNext::Wait), "it waits for the round");
        let c1 = chain.candidates.iter().find(|c| c.id == "c1").unwrap();
        assert_eq!(c1.phase, Phase::Validating, "the running gate is left as it is");
        // The round lands and its turn leaves: the stacked round opens.
        chain.finish("c1", ran(true));
        chain.land("c1");
        chain.leave_round(|a| a.id == "t1");
        assert_eq!(chain.stacked, None);
        let open = chain.round.clone().expect("the stacked round, open");
        assert_eq!((open.id.as_str(), open.on.as_ref()), ("s", None));
        assert_eq!(open.root.map(|r| r.changes), Some(vec![2, 3]));
        assert!(chain.candidates.iter().find(|c| c.id == merge).unwrap().valid(), "on record");
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Assign));
        assert_eq!(chain.round_next("t3"), Some(RoundNext::Wait));
    }

    /// The round running ends red: the stacked round is discarded, its PRs
    /// pending again, ahead of the rest, and charged nothing.
    #[test]
    fn a_stacked_round_on_a_round_that_ends_red_is_discarded_and_its_prs_pair_next() {
        let mut chain = whole_running();
        asks(&mut chain, 2);
        assert!(chain.open_round("s"));
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Stack), "a round of one is its root");
        let merge = stack_and_gate(&mut chain, false);
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Wait), "a red stacked merge waits");
        asks(&mut chain, 4);
        chain.finish("c1", ran(false));
        chain.leave_round(|a| a.id == "t1");
        assert_eq!((chain.round.as_ref(), chain.stacked.as_ref()), (None, None));
        let pending: Vec<&str> = chain.pending.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(pending, ["t2", "t4"]);
        assert_eq!(chain.round_next("t2"), None, "nothing was said to it");
        let m = chain.candidates.iter().find(|c| c.id == merge).unwrap();
        assert!(!m.valid());
        assert!(chain.open_round("n"));
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Pair), "it pairs in the next round");
    }

    /// A PR ready later in the gate joins the stacked round, a level above
    /// its root: a stacked merge of that root not started ends, one gating
    /// runs on. None stacks while the round pairs, nor does a PR that left
    /// the stacked round.
    #[test]
    fn a_pr_ready_later_joins_the_stacked_round_above_its_root() {
        let mut chain = asking(2);
        chain.open_round("r");
        asks(&mut chain, 3);
        assert!(!chain.open_round("s"), "the round pairs: no whole gate runs");
        assert!(chain.stacked.is_none());

        let mut chain = whole_running();
        asks(&mut chain, 2);
        assert!(chain.open_round("s"));
        let merge = make_stack(&mut chain);
        asks(&mut chain, 3);
        assert!(chain.open_round("s3"));
        let stacked = chain.stacked.clone().unwrap();
        assert_eq!(stacked.id, "s", "it joins the one there is");
        assert_eq!(
            stacked.nodes.iter().map(|n| n.changes.clone()).collect::<Vec<_>>(),
            [vec![2], vec![3]]
        );
        let m = chain.candidates.iter().find(|c| c.id == merge).unwrap();
        assert_eq!(m.cause.as_deref(), Some("stack_joined"));
        // 3 merges with no node: it leaves, and asking again stacks nothing.
        pair(&mut chain, |_, _| false);
        assert_eq!(chain.round_next("t3"), Some(RoundNext::Left(Left::Unpaired)));
        chain.leave_round(|a| a.id == "t3");
        asks(&mut chain, 3);
        assert!(!chain.open_round("s3"));
        assert_eq!(chain.pending.len(), 1);
        assert_eq!(chain.round_next("t2"), Some(RoundNext::Stack));
        // Its stacked merge gating, a PR that asks joins, and the gate runs on.
        chain.pending.clear();
        let gating = make_stack(&mut chain);
        chain.candidates.iter_mut().filter(|c| c.id == gating).for_each(|c| {
            c.phase = Phase::Validating;
        });
        asks(&mut chain, 4);
        assert!(chain.open_round("s4"));
        let m = chain.candidates.iter().find(|c| c.id == gating).unwrap();
        assert!(m.valid() && m.phase == Phase::Validating, "{m:?}");
        assert_eq!(chain.round_next("t4"), Some(RoundNext::Pair));
    }

    /// Makes the stacked round's stacked merge, not gated yet → its id.
    fn make_stack(chain: &mut Chain) -> String {
        let (round, on, heads) = chain.to_stack().expect("a root to stack");
        let commit = heads.iter().fold(on, |merged, a| format!("({merged}+{})", a.head));
        let merged = Merged { commit, tree: "stacked".into(), command: "fast".into(), seconds: 1 };
        chain.stack_merge(&round, Some(merged), |_| Plain::default(), |id| format!("w/{id}"), 0);
        let made =
            chain.candidates.iter().rev().find(|c| c.pair.as_ref().is_some_and(|p| p.stacked));
        made.unwrap().id.clone()
    }
}

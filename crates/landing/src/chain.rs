//! Immutable merge candidates, their validation, and ordered publication.
//!
//! The remote holds one ordered record. Assignment and every transition use
//! compare-and-swap; publication changes that record and the base atomically.
//! Source heads never move. Invalidation is permanent and precedes signalling
//! processes. A cancelled gate keeps its lane until its descendants are gone.
//! A green successor waits on its exact predecessor, without another gate.
//! Where the commit declares a fast gate and a slow gate, a candidate runs
//! the fast one, and a round runs the slow one once at the chain's tip,
//! landing every candidate up to it together (docs/adr/0048). In a batch
//! round no candidate gates alone: the chain's tip runs both gates, and a
//! red one is bisected to its first red candidate (docs/adr/0068).
use std::collections::BTreeMap;

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

use afkconvoy_gate::gate::{GreenRun, Judged, Ran, Termination};

use crate::lease::{stamp_due, stamp_stale};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Building,
    Validating,
    Green,
    /// The round's tip, running the slow gate on its commit.
    Slow,
    /// A candidate of a batch round not gated alone: the gate at the chain's
    /// tip, or at a candidate the round bisects to, gates it with the rest
    /// (docs/adr/0068).
    Batched,
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

/// A program's candidate notes nothing beside what the chain reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plain {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate<N = Plain> {
    pub id: String,
    /// The change it is of, by its forge's number (`forge::Change::number`).
    #[serde(rename = "pr")]
    pub change: u64,
    /// What the program that lands it notes of the candidate, which no rule
    /// of the chain reads: its fields are the candidate's own when it is
    /// written and read (afkconvoy's: the issue the change completes).
    #[serde(flatten)]
    pub note: N,
    /// The branch the change's head is on, which landing never moves.
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
    /// Frozen descendants are recorded before any are killed, so interrupted
    /// cancellation still owns escaped and reparented processes.
    #[serde(default)]
    pub descendants: Vec<Process>,
    pub worktree: String,
    /// Cuts, watches, certification and fleet-lock repairs are barriers.
    pub barrier: bool,
    /// A cancelled candidate names the root that invalidated it and why.
    pub cancelled_by: Option<String>,
    pub cause: Option<String>,
    /// True only after the gate group and descendants have been observed gone.
    pub stopped: bool,
    /// Its owner's host was read dead (`Chain::abandon`): it holds the chain
    /// no more, though nobody confirmed its processes gone — they are on a
    /// host nobody else can reach. Should that host come back, its own turn
    /// stops them, and only then is it `stopped`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub abandoned: bool,
    /// Actual gate evidence survives invalidation and owner death.
    pub result: Option<Evidence>,
    /// The slow gate its commit declares, which a round at this candidate
    /// runs; `None` where it declares no gates and `command` is the whole gate.
    #[serde(default)]
    pub slow: Option<Slow>,
    /// The round at this candidate gated green.
    #[serde(default)]
    pub slowed: bool,
    /// It was assigned for a batch round (docs/adr/0068): it is not gated
    /// alone, and a gate at it gates every candidate up to it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub batch: bool,
    /// The gate of its batch round ran red at it: the round's first red
    /// candidate is this one or one before it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub holds_red: bool,
}

/// How a candidate's gate run ended, as its evidence says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Green,
    #[default]
    Red,
    /// The candidate was cancelled while its gate ran.
    Cancelled,
}

/// A run red only on TIMEOUTs the gate said it cannot classify on an
/// overloaded host (`gate::Unsettled`), as the chain keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overloaded {
    /// The gate's line that says the host is overloaded, with its load.
    pub said: String,
    /// The TIMEOUT tests.
    pub tests: Vec<String>,
}

/// A candidate's gate evidence: what its run, fast or a round's slow one,
/// said of its tree and what became of its record. The chain decides from
/// it (`Chain::finish`). Its keys are in the order a JSON object's are
/// written, so a record reads as it did when the chain kept it untyped. A
/// key a record lacks reads as red's: nothing kept is no green.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Evidence {
    pub command: Option<String>,
    pub excerpt: Vec<String>,
    pub exit_code: i32,
    /// The commit the run was of.
    pub head: Option<String>,
    /// Why a green run is not on record, where it is not.
    pub not_recorded: Option<String>,
    /// A green run of the committed tree is on record on the remote.
    pub recorded: bool,
    /// The candidate's gate did not run: the green run on record of its
    /// tree, of the same command and kind of machine, answered (a reused
    /// run), and this names it. `None` where the gate ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reused: Option<Reused>,
    pub seconds: u64,
    pub status: Status,
    pub termination: Option<Termination>,
    pub timed_out: bool,
    /// What kept the run from being of the committed tree.
    pub uncommitted: Vec<String>,
    /// The run says nothing of its tree (`Overloaded`).
    pub unsettled: Option<Overloaded>,
}

impl Evidence {
    /// What the chain hears at a red run's first observed exit or limit,
    /// before its log is drained or its record settled: how it ended. Its
    /// owner adds the rest (`Chain::finish`).
    pub fn red(
        exit_code: i32,
        timed_out: bool,
        seconds: u64,
        termination: Option<Termination>,
    ) -> Evidence {
        Evidence { exit_code, seconds, termination, timed_out, ..Evidence::default() }
    }

    /// The run passes its candidate: green, and on record.
    pub fn green(&self) -> bool {
        self.status == Status::Green && self.recorded
    }
}

/// The green run on record a reused run stands on: when it finished, and,
/// where its record says, how long it took on which host and the kind of
/// machine it ran on. With the evidence's command and its commit's tree, it
/// names the record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reused {
    pub at: u64,
    pub host: Option<String>,
    pub machine: Option<String>,
    pub seconds: Option<u64>,
}

impl Reused {
    /// The run on record `run` names.
    pub fn of(run: &GreenRun) -> Reused {
        Reused {
            at: run.at(),
            host: run.took().map(|took| took.host().to_string()),
            machine: run.machine().map(str::to_string),
            seconds: run.took().map(|took| took.seconds()),
        }
    }
}

/// One gate run of a candidate, as its runner saw it end: what its
/// evidence is made of (`Evidence::of`).
#[derive(Debug)]
pub struct Observed<'a> {
    /// The command it ran.
    pub command: &'a str,
    pub ran: Ran,
    /// What the run says of its tree.
    pub judged: &'a Judged,
    /// The commit the run was of.
    pub head: String,
    /// The end of its output.
    pub excerpt: Vec<String>,
    /// What kept the run from being of the committed tree.
    pub strays: Vec<String>,
    /// A green run of it was written on record…
    pub written: bool,
    /// …and why it is not on the remote, where it is not.
    pub not_recorded: Option<String>,
    pub termination: Option<Termination>,
    /// The candidate was cancelled while its gate ran.
    pub cancelled: bool,
    /// The green run on record that answered instead, where nothing ran.
    pub reused: Option<Reused>,
}

impl Evidence {
    /// The evidence of a candidate's run `run`: green where it ran green
    /// and was not cancelled, on record where its record was written and
    /// reached the remote, or a run on record answered for it (`reused`);
    /// unsettled where it says nothing of its tree.
    pub fn of(run: Observed<'_>) -> Evidence {
        let unsettled = match run.judged {
            Judged::Unsettled(unsettled) => {
                Some(Overloaded { said: unsettled.said.clone(), tests: unsettled.tests().to_vec() })
            }
            Judged::Green | Judged::Red(_) => None,
        };
        let status = match () {
            _ if run.cancelled => Status::Cancelled,
            _ if run.ran.green() => Status::Green,
            _ => Status::Red,
        };
        Evidence {
            command: Some(run.command.to_string()),
            exit_code: run.ran.exit_code,
            excerpt: run.excerpt,
            head: Some(run.head),
            recorded: (run.written || run.reused.is_some()) && run.not_recorded.is_none(),
            not_recorded: run.not_recorded,
            reused: run.reused,
            seconds: run.ran.seconds,
            status,
            termination: run.termination,
            timed_out: run.ran.timed_out,
            uncommitted: run.strays,
            unsettled,
        }
    }
}

/// The slow gate a candidate's commit declares, and its whole gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slow {
    pub command: String,
    pub seconds: u64,
    /// `gate_command`, which its fast and slow runs on one tree pass together.
    pub whole: String,
}

/// What the chain decided of a candidate that holds it no more: the one
/// source of its turn's ending, which its program words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// It landed, by its own turn or with a round at a candidate behind it.
    Landed,
    /// Its own gate, or the slow gate of the round at it, ran red.
    Red,
    /// Its own gate said nothing of it (`gate_unsettled`).
    Unsettled,
    /// The slow gate ran red on a round at another candidate, which holds
    /// this one's change (`round_red`).
    RoundRed,
    /// The target branch moved past it outside the chain (`base_moved`).
    TargetMoved,
    /// Another candidate's fault, or its own turn's, cancelled it.
    Cancelled,
}

impl<N> Candidate<N> {
    /// What the chain decided of it: `None` while it is valid, waiting or
    /// gating. It is red where its own gate or its round failed it; it left
    /// the chain unsettled where its own run said nothing of it; otherwise
    /// what cancelled it says which ending.
    pub fn decision(&self) -> Option<Decision> {
        let cause = self.cause.as_deref();
        let own = self.cancelled_by.as_deref() == Some(self.id.as_str());
        Some(match self.phase {
            Phase::Landed => Decision::Landed,
            Phase::Red => Decision::Red,
            _ if self.valid() => return None,
            _ if own && cause == Some("gate_unsettled") => Decision::Unsettled,
            _ if cause == Some("round_red") => Decision::RoundRed,
            _ if cause == Some("base_moved") => Decision::TargetMoved,
            _ => Decision::Cancelled,
        })
    }

    /// Display copy only: every interface uses this lifecycle interpretation.
    pub fn observation(&self) -> Validation {
        Validation {
            id: self.id.clone(),
            predecessor: self.predecessor.clone(),
            commit: self.commit.clone(),
            phase: self.phase,
            waiting: self.waiting().to_string(),
            cancelled_by: self.cancelled_by.clone(),
            cause: self.cause.clone(),
        }
    }
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            Phase::Building | Phase::Validating | Phase::Green | Phase::Slow | Phase::Batched
        ) || (!self.stopped && !self.abandoned)
    }

    pub fn valid(&self) -> bool {
        matches!(
            self.phase,
            Phase::Building | Phase::Validating | Phase::Green | Phase::Slow | Phase::Batched
        ) && self.cancelled_by.is_none()
    }

    /// A later turn may own this candidate's preserved evidence on the same
    /// host: it is fast-green, and its gate processes are confirmed gone.
    pub fn adoptable(&self) -> bool {
        self.valid() && self.phase == Phase::Green && self.stopped
    }

    /// Its tree has passed the whole gate: its own gate was the whole gate,
    /// or the round at it gated green.
    pub fn whole(&self) -> bool {
        self.phase == Phase::Green && (self.slow.is_none() || self.slowed)
    }

    pub fn waiting(&self) -> &'static str {
        match self.phase {
            Phase::Building => "constructing_worktree",
            Phase::Validating => "validating",
            Phase::Green if !self.whole() => "waiting_for_round",
            Phase::Green => "waiting_for_predecessor",
            Phase::Slow => "slow_gating",
            Phase::Batched => "waiting_for_batch_gate",
            Phase::Cancelling => "confirming_process_exit",
            Phase::Cancelled => "cancelled",
            Phase::Red => "gate_red",
            Phase::Landed => "landed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validation {
    pub id: String,
    pub predecessor: String,
    pub commit: String,
    pub phase: Phase,
    pub waiting: String,
    pub cancelled_by: Option<String>,
    pub cause: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain<N = Plain> {
    pub candidates: Vec<Candidate<N>>,
    /// A serial turn reserves its place without occupying a worker lane.
    #[serde(default)]
    pub barriers: Vec<Barrier>,
    /// The turns that asked to land and hold no candidate yet: a round waits
    /// for them, as it is as big as the PRs that are ready.
    #[serde(default)]
    pub pending: Vec<Barrier>,
    /// When each host that owns a member — a candidate not yet stopped, a
    /// barrier or a pending turn — last said a turn of it is alive, in
    /// seconds: its turns stamp it as they wait (`beat`). Another host reads
    /// it to tell a live owner from a dead one (`abandon`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub heartbeats: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Barrier {
    pub id: String,
    pub owner: Process,
}

impl<N> Default for Chain<N> {
    fn default() -> Chain<N> {
        Chain { candidates: vec![], barriers: vec![], pending: vec![], heartbeats: BTreeMap::new() }
    }
}

impl<N: Clone> Chain<N> {
    /// The window has no bound of its own: the worker lanes bound how many
    /// gates run at once, and a round takes every PR that is ready
    /// (sunfmin/control#244). Unconfirmed cancellation still holds it, and a
    /// barrier drains it and admits no successors.
    pub fn can_assign(&self, barrier: bool) -> bool {
        let active: Vec<_> = self.candidates.iter().filter(|c| c.active()).collect();
        (!barrier || active.is_empty())
            && (barrier || self.barriers.is_empty())
            && !active.iter().any(|c| c.barrier || !c.valid())
    }

    /// The hosts that own a member: a candidate not yet stopped nor
    /// abandoned, a barrier or a pending turn.
    pub fn owners(&self) -> Vec<&str> {
        let mut hosts: Vec<&str> = vec![];
        for c in &self.candidates {
            if !c.stopped && !c.abandoned {
                hosts.push(&c.owner.host);
            }
        }
        for b in &self.barriers {
            hosts.push(&b.owner.host);
        }
        for b in &self.pending {
            hosts.push(&b.owner.host);
        }
        hosts.sort_unstable();
        hosts.dedup();
        hosts
    }

    /// A turn on `host` is alive at `now`: stamps it, where `host` owns a
    /// member and its stamp is missing or due (`lease::stamp_due`). A stamp
    /// of a host that owns nothing is dropped → whether anything changed.
    pub fn beat(&mut self, host: &str, now: u64) -> bool {
        let before = self.heartbeats.clone();
        let owners: Vec<String> = self.owners().into_iter().map(str::to_string).collect();
        self.heartbeats.retain(|stamped, _| owners.contains(stamped));
        if owners.iter().any(|owner| owner == host)
            && self.heartbeats.get(host).is_none_or(|&stamp| stamp_due(stamp, now))
        {
            self.heartbeats.insert(host.to_string(), now);
        }
        before != self.heartbeats
    }

    /// `host` was heard alive at `ts` some other way — by a fleet heartbeat
    /// of it — and is stamped so, where that is newer than its stamp.
    pub fn heard(&mut self, host: &str, ts: u64) {
        let stamp = self.heartbeats.entry(host.to_string()).or_insert(ts);
        *stamp = (*stamp).max(ts);
    }

    /// Whether a turn on `host` said it was alive within the lease at `now`.
    pub fn beats(&self, host: &str, now: u64) -> bool {
        self.heartbeats.get(host).is_some_and(|&stamp| !stamp_stale(stamp, now))
    }

    /// Takes over everything `host` owns, once it is read dead: its barriers
    /// and pending turns are dropped, each of its valid candidates is
    /// invalidated `owner_died` with its suffix, and each of its candidates
    /// not confirmed stopped is abandoned. The suffix's own owners cancel
    /// their candidates as for any failed predecessor.
    pub fn abandon(&mut self, host: &str) {
        self.barriers.retain(|b| b.owner.host != host);
        self.pending.retain(|b| b.owner.host != host);
        let mut roots: Vec<String> = vec![];
        for c in &self.candidates {
            if c.owner.host == host && c.valid() {
                roots.push(c.id.clone());
            }
        }
        for root in roots {
            self.invalidate(&root, "owner_died");
        }
        for c in &mut self.candidates {
            if c.owner.host == host && !c.stopped && c.phase != Phase::Landed {
                c.abandoned = true;
                if c.phase == Phase::Cancelling {
                    c.phase = Phase::Cancelled;
                }
            }
        }
        self.heartbeats.remove(host);
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
                if c.cancelled_by.is_some() {
                    continue;
                }
                c.cancelled_by = Some(root.to_string());
                c.cause = Some(cause.to_string());
                if c.phase != Phase::Red {
                    c.phase = if c.stopped { Phase::Cancelled } else { Phase::Cancelling };
                }
            }
        }
    }

    /// A candidate's gate evidence, which decides it: a run that says
    /// nothing of its tree leaves it unsettled (`unsettle`); one green and
    /// on record is green; any other is red. For the round's tip in `Slow`,
    /// the round's: green passes the whole gate at it; red fails the round.
    /// For a candidate of a batch round, its gate says what it says of every
    /// candidate up to it (`finish_batched`).
    pub fn finish(&mut self, id: &str, result: Evidence) {
        let batch = self.candidates.iter().any(|c| c.id == id && c.batch && c.valid());
        if result.unsettled.is_some() {
            self.unsettle(id, result);
        } else if batch {
            self.finish_batched(id, result);
        } else {
            let green = result.green();
            self.green_or_red(id, green, result);
        }
    }

    fn green_or_red(&mut self, id: &str, green: bool, result: Evidence) {
        let mut failed = false;
        let mut round = false;
        for c in &mut self.candidates {
            if c.id == id && c.valid() {
                c.result = Some(result.clone());
                round = c.phase == Phase::Slow;
                c.phase = if green { Phase::Green } else { Phase::Red };
                c.slowed = round && green;
                failed = !green;
            } else if c.id == id
                && c.phase == Phase::Red
                && c.cancelled_by.as_deref() == Some(id)
                && !green
            {
                // The process observer invalidated immediately. Its owner
                // later adds the complete log/worktree/recording evidence.
                c.result = Some(result.clone());
            }
        }
        if failed && round {
            self.round_red(id);
        } else if failed {
            self.invalidate(id, "predecessor_failed");
        }
    }

    /// A candidate's gate, fast or a round's slow one, ran red only on
    /// TIMEOUTs it cannot classify on an overloaded host: the run says
    /// nothing of it, so it is no red. It keeps `result` and leaves the chain
    /// `gate_unsettled`, for its turn asks again; what was merged onto its
    /// commit goes with it, `predecessor_unsettled`, as no successor can land
    /// without it. Its round is not failed: the members before it stay
    /// fast-green, and the last of them is the round's tip now.
    fn unsettle(&mut self, id: &str, result: Evidence) {
        if !self.candidates.iter().any(|c| c.id == id && c.valid()) {
            return;
        }
        self.invalidate(id, "gate_unsettled");
        for c in &mut self.candidates {
            if c.id == id {
                c.result = Some(result.clone());
            } else if c.cancelled_by.as_deref() == Some(id) {
                c.cause = Some("predecessor_unsettled".to_string());
            }
        }
    }

    /// The candidates a landing at `id` takes onto the base: every valid one
    /// up to and including it, in chain order. Empty when `id` is not valid.
    pub fn members(&self, id: &str) -> Vec<&Candidate<N>> {
        let mut members = vec![];
        for c in self.candidates.iter().filter(|c| c.valid()) {
            members.push(c);
            if c.id == id {
                return members;
            }
        }
        vec![]
    }

    /// The candidate whose turn runs a round's slow gate now: the last valid
    /// candidate, once every valid candidate is fast-green and none is
    /// slow-gating, and no pending turn may still join it, where its commit
    /// declares a slow gate not yet passed. A candidate assigned during the
    /// round joins the next one.
    pub fn round_tip(&self) -> Option<&Candidate<N>> {
        let valid: Vec<&Candidate<N>> = self.candidates.iter().filter(|c| c.valid()).collect();
        let tip = valid.last()?;
        let ready = !self.joinable() && valid.iter().all(|c| c.phase == Phase::Green);
        (ready && tip.slow.is_some() && !tip.slowed).then_some(*tip)
    }

    /// Whether a pending turn may still join the round: one is pending, and
    /// no barrier, on the chain or waiting, admits no successors. A round
    /// that waited for turns a barrier holds back would hold the barrier
    /// back in turn: the watch's serial landing of 2026-10-29, fast-green,
    /// waited for six pending turns that waited for it.
    pub fn joinable(&self) -> bool {
        !self.pending.is_empty()
            && self.barriers.is_empty()
            && !self.candidates.iter().any(|c| c.valid() && c.barrier)
    }

    /// The candidate whose running slow gate `id` waits on: the tip of the
    /// round now slow-gating, where `id` is valid and at or before it. A
    /// candidate behind it, or one with no round running, waits for the next.
    pub fn round_of(&self, id: &str) -> Option<&Candidate<N>> {
        let mut member = false;
        for c in self.candidates.iter().filter(|c| c.valid()) {
            member = member || c.id == id;
            if c.phase == Phase::Slow {
                return member.then_some(c);
            }
        }
        None
    }

    /// Starts the round at `id` when it is the round's tip → whether it did.
    /// Its gate process is a new one, to be confirmed gone again.
    pub fn start_round(&mut self, id: &str) -> bool {
        if self.round_tip().is_none_or(|tip| tip.id != id) {
            return false;
        }
        for c in &mut self.candidates {
            if c.id == id {
                c.phase = Phase::Slow;
                c.stopped = false;
                c.gate = None;
                c.descendants.clear();
            }
        }
        true
    }

    /// A red round is not bisected: its tip keeps the failure, every other
    /// candidate of the round is cancelled by it, and its successors by
    /// their failed predecessor.
    pub fn round_red(&mut self, tip: &str) {
        // The tip is already red here: the round is what stands before it.
        let mut members: Vec<String> = vec![];
        for c in &self.candidates {
            if c.id == tip {
                break;
            }
            if c.valid() {
                members.push(c.id.clone());
            }
        }
        for c in &mut self.candidates {
            if members.contains(&c.id) {
                c.cancelled_by = Some(tip.to_string());
                c.cause = Some("round_red".to_string());
                c.phase = if c.stopped { Phase::Cancelled } else { Phase::Cancelling };
            }
        }
        self.invalidate(tip, "round_red");
        for c in &mut self.candidates {
            if c.cancelled_by.as_deref() == Some(tip) && c.id != tip && !members.contains(&c.id) {
                c.cause = Some("predecessor_failed".to_string());
            }
        }
    }

    /// What a batch round does next (docs/adr/0068), over the valid
    /// candidates of the chain, every one assigned for it; `None` while a
    /// gate at one of them runs, where the chain holds none, or where it
    /// holds one that is not the batch round's. With no red known, its tip
    /// is gated, and once green it lands every candidate up to it. With a
    /// red known, the chain is bisected between the last candidate known
    /// green before it and the first known red: the middle one is gated,
    /// until the two are next to each other, and the red one is the first
    /// red candidate.
    pub fn batch_step(&self) -> Option<BatchStep> {
        let valid: Vec<&Candidate<N>> = self.candidates.iter().filter(|c| c.valid()).collect();
        let tip = valid.last()?;
        if !valid.iter().all(|c| c.batch && matches!(c.phase, Phase::Batched | Phase::Green)) {
            return None;
        }
        // The candidates the bisection is between: after the last known
        // green, up to the first known red.
        let mut open: Vec<&Candidate<N>> = vec![];
        let mut red = false;
        for c in &valid {
            if c.holds_red {
                open.push(c);
                red = true;
                break;
            }
            if c.whole() {
                open.clear();
            } else {
                open.push(c);
            }
        }
        if !red {
            let id = tip.id.clone();
            return Some(if tip.whole() { BatchStep::Land(id) } else { BatchStep::Gate(id) });
        }
        let (first_red, before) = open.split_last()?;
        if before.is_empty() {
            return Some(BatchStep::Drop(first_red.id.clone()));
        }
        before.get(before.len() / 2).map(|middle| BatchStep::Gate(middle.id.clone()))
    }

    /// Starts the gate of the batch round at `id`, where its round gates it
    /// next (`batch_step`) → whether it did. Its gate process is a new one.
    pub fn start_batched(&mut self, id: &str) -> bool {
        if self.batch_step() != Some(BatchStep::Gate(id.to_string())) {
            return false;
        }
        for c in self.candidates.iter_mut().filter(|c| c.id == id) {
            c.phase = Phase::Validating;
            c.stopped = false;
            c.gate = None;
            c.descendants.clear();
        }
        true
    }

    /// The gate of the batch round at `id` ended with `result`, its fast
    /// gate's and, that green, its slow gate's: green, its tree passed the
    /// whole gate, and it may land with every candidate before it; red, it
    /// holds the round's first red candidate, itself or one before it, and
    /// is not decided by it.
    fn finish_batched(&mut self, id: &str, result: Evidence) {
        let green = result.green();
        for c in self.candidates.iter_mut().filter(|c| c.id == id && c.valid()) {
            c.result = Some(result.clone());
            c.slowed = green && c.slow.is_some();
            c.holds_red = !green;
            c.phase = if green { Phase::Green } else { Phase::Batched };
        }
    }

    /// Drops `red`, a batch round's first red candidate (`batch_step`): it is
    /// red, with the run that found it, and every candidate behind it is
    /// cancelled `rebuilt`, for its change to be assigned again without it.
    /// The candidates before it stay, and land.
    pub fn drop_red(&mut self, red: &str) {
        if self.batch_step() != Some(BatchStep::Drop(red.to_string())) {
            return;
        }
        for c in self.candidates.iter_mut().filter(|c| c.id == red) {
            c.phase = Phase::Red;
        }
        self.invalidate(red, "rebuilt");
        for c in self.candidates.iter_mut().filter(|c| c.id == red) {
            c.cause = Some("predecessor_failed".to_string());
        }
    }

    /// `id` may land now on `base`, taking every member with it: its tree
    /// passed the whole gate, every member is green, every one ahead of it
    /// is waiting for a round, and the first stands on `base`.
    pub fn ripe(&self, id: &str, base: &str) -> bool {
        let members = self.members(id);
        let Some(first) = members.first() else { return false };
        let Some(last) = members.last() else { return false };
        // A whole-gated candidate ahead lands by its own turn, not this one;
        // in a batch round the tip's gate passed every one before it.
        let (ahead, _) = members.split_at(members.len() - 1);
        let batch = last.batch;
        last.whole()
            && members
                .iter()
                .all(|c| c.phase == Phase::Green || (batch && c.phase == Phase::Batched))
            && (batch || ahead.iter().all(|c| !c.whole()))
            && first.predecessor == base
    }

    /// Marks `id` and every member before it landed.
    pub fn land(&mut self, id: &str) {
        let members: Vec<String> = self.members(id).iter().map(|c| c.id.clone()).collect();
        for c in &mut self.candidates {
            if members.contains(&c.id) {
                c.phase = Phase::Landed;
            }
        }
    }

    /// Drops every ended candidate `released` says nothing holds any more,
    /// unless a candidate kept depends on it: as its predecessor, or as the
    /// root that cancelled it. An active one is always kept.
    pub fn prune(&mut self, released: impl Fn(&Candidate<N>) -> bool) {
        let mut kept: Vec<Candidate<N>> = vec![];
        let mut needed: Vec<String> = vec![];
        for c in self.candidates.drain(..).rev() {
            if c.active() || needed.contains(&c.commit) || needed.contains(&c.id) || !released(&c) {
                needed.push(c.predecessor.clone());
                needed.extend(c.cancelled_by.clone());
                kept.push(c);
            }
        }
        kept.reverse();
        self.candidates = kept;
    }

    /// An ordinary advance to a known candidate preserves its successors.
    /// An external base move invalidates all candidates rooted elsewhere.
    /// Atomic publication already records the landed prefix before its
    /// program settles what landed. An external move never promotes an unfinished candidate.
    pub fn observe_base(&mut self, base: &str) {
        let first = self.candidates.iter().find(|c| c.valid());
        if let Some(c) = first
            && c.predecessor != base
        {
            let id = c.id.clone();
            self.invalidate(&id, "base_moved");
        }
    }
}

/// What a batch round does next (`Chain::batch_step`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchStep {
    /// Gate the candidate named, its fast gate and then its slow one.
    Gate(String),
    /// The candidate named is the round's first red one: drop it.
    Drop(String),
    /// Land the candidate named, with every candidate before it.
    Land(String),
}

impl BatchStep {
    /// The candidate it is of.
    pub fn id(&self) -> &str {
        match self {
            BatchStep::Gate(id) | BatchStep::Drop(id) | BatchStep::Land(id) => id,
        }
    }
}

/// The effect boundary must supply each observation anew. Flux checks that
/// publication cannot be authorized with any missing observation.
#[derive(Debug)]
#[refined_by(valid: bool, green: bool, predecessor: bool, unlocked: bool, head: bool, stopped: bool)]
#[invariant(valid && green && predecessor && unlocked && head && stopped)]
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
    #[field(bool[stopped])]
    _stopped: bool,
}

impl Publication {
    pub fn new(
        valid: bool,
        green: bool,
        predecessor: bool,
        unlocked: bool,
        head: bool,
        stopped: bool,
    ) -> Option<Self> {
        if valid && green && predecessor && unlocked && head && stopped {
            Some(Self {
                _valid: valid,
                _green: green,
                _predecessor: predecessor,
                _unlocked: unlocked,
                _head: head,
                _stopped: stopped,
            })
        } else {
            None
        }
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    #[should_fail]
    fn unconfirmed_exit_cannot_authorize_publication() -> Publication {
        Publication {
            _valid: true,
            _green: true,
            _predecessor: true,
            _unlocked: true,
            _head: true,
            _stopped: false,
        }
    }

    #[should_fail]
    fn invalidated_green_cannot_authorize_publication() -> Publication {
        Publication {
            _valid: false,
            _green: true,
            _predecessor: true,
            _unlocked: true,
            _head: true,
            _stopped: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's evidence: green and on record, or red.
    fn ran(green: bool) -> Evidence {
        let mut evidence = Evidence::red(if green { 0 } else { 1 }, false, 1, None);
        if green {
            evidence.status = Status::Green;
            evidence.recorded = true;
        }
        evidence
    }

    /// A run red only on a TIMEOUT the gate cannot classify under overload.
    fn unsettled() -> Evidence {
        let said = "gate: host overloaded (load 13, CPUs 12); cannot classify TIMEOUT tests.";
        let overloaded = Overloaded { tests: vec!["nx a_test".into()], said: said.into() };
        Evidence { unsettled: Some(overloaded), ..Evidence::red(100, false, 1, None) }
    }

    /// What the chain leaves a candidate in decides it
    /// (sunfmin/control#620): each state to its decision.
    #[test]
    fn the_chain_decides_each_candidate_it_leaves() {
        let decisions = |chain: &Chain| -> Vec<Option<Decision>> {
            chain.candidates.iter().map(Candidate::decision).collect()
        };
        // Gating, or green and waiting: nothing decided.
        let mut chain = Chain { candidates: (0..2).map(candidate).collect(), ..Chain::default() };
        assert_eq!(decisions(&chain), [None, None]);
        chain.finish("c0", ran(true));
        assert_eq!(decisions(&chain), [None, None]);
        chain.land("c0");
        assert_eq!(decisions(&chain)[0], Some(Decision::Landed));
        // A red root, and the suffix it cancels.
        let mut chain = Chain { candidates: (0..2).map(candidate).collect(), ..Chain::default() };
        chain.finish("c0", ran(false));
        assert_eq!(decisions(&chain), [Some(Decision::Red), Some(Decision::Cancelled)]);
        // An unsettled root; what was merged onto it is cancelled.
        let mut chain = Chain { candidates: (0..2).map(candidate).collect(), ..Chain::default() };
        chain.finish("c0", unsettled());
        assert_eq!(decisions(&chain), [Some(Decision::Unsettled), Some(Decision::Cancelled)]);
        // A red round: its tip red, the members it holds round-red.
        let mut chain = Chain { candidates: (0..2).map(split).collect(), ..Chain::default() };
        for c in ["c0", "c1"] {
            chain.finish(c, ran(true));
            chain.candidates.iter_mut().for_each(|c| c.stopped = c.phase == Phase::Green);
        }
        assert!(chain.start_round("c1"));
        chain.finish("c1", ran(false));
        assert_eq!(decisions(&chain), [Some(Decision::RoundRed), Some(Decision::Red)]);
        // A target moved outside the chain.
        let mut chain = Chain { candidates: vec![candidate(0)], ..Chain::default() };
        chain.observe_base("elsewhere");
        assert_eq!(decisions(&chain), [Some(Decision::TargetMoved)]);
    }

    /// The chain decides a candidate from its typed evidence
    /// (sunfmin/control#620): green only where the run was green and is on
    /// record; a green not on record, a cancelled run and a red are red; a
    /// run that says nothing of its tree is unsettled, never red.
    #[test]
    fn the_chain_decides_a_candidate_from_its_gate_evidence() {
        let decided = |evidence: Evidence| {
            let mut chain = Chain { candidates: vec![candidate(0)], ..Chain::default() };
            chain.finish("c0", evidence.clone());
            assert_eq!(chain.candidates[0].result, Some(evidence));
            (chain.candidates[0].phase, chain.candidates[0].cause.clone())
        };
        let red = (Phase::Red, Some("predecessor_failed".to_string()));
        assert_eq!(decided(ran(true)), (Phase::Green, None));
        assert_eq!(decided(ran(false)), red);
        let unrecorded = Evidence { recorded: false, ..ran(true) };
        assert_eq!(decided(unrecorded), red, "a green not on record");
        let cancelled = Evidence { status: Status::Cancelled, ..ran(false) };
        assert_eq!(decided(cancelled), red);
        let (phase, cause) = decided(unsettled());
        assert_ne!(phase, Phase::Red);
        assert_eq!(cause.as_deref(), Some("gate_unsettled"));
    }

    /// Evidence an earlier afkconvoy kept untyped reads back: the run's keys,
    /// its nulls, the keys of the turn's answer it carried beside them, and
    /// a red's first, shorter evidence. Its own keys are written in the
    /// order an untyped record wrote them.
    #[test]
    fn evidence_kept_untyped_reads_back_typed() {
        let kept = r#"{"candidate":"c1","command":"./gate.sh fast","exit_code":100,
            "excerpt":["x"],"head":"abc","merged":"base","not_recorded":null,
            "predecessor":"base","recorded":false,"seconds":12,"status":"red",
            "termination":null,"timed_out":false,"uncommitted":[],
            "unsettled":{"said":"gate: host overloaded","tests":["nx a_test"]}}"#;
        let evidence: Evidence = serde_json::from_str(kept).unwrap();
        assert_eq!(evidence.unsettled.as_ref().map(|u| u.tests.len()), Some(1));
        assert_eq!(evidence.head.as_deref(), Some("abc"));
        let written = serde_json::to_string(&evidence).unwrap();
        let untyped = serde_json::to_string(&serde_json::to_value(&evidence).unwrap()).unwrap();
        assert_eq!(written, untyped, "keys in the order an untyped record writes them");
        let first = r#"{"status":"red","exit_code":1,"timed_out":true,"seconds":3,
            "termination":null}"#;
        let first: Evidence = serde_json::from_str(first).unwrap();
        assert_eq!(first, Evidence::red(1, true, 3, None));
    }
    use std::collections::{HashSet, VecDeque};

    fn candidate(n: usize) -> Candidate {
        Candidate {
            id: format!("c{n}"),
            change: n as u64,
            note: Plain::default(),
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
            descendants: vec![],
            worktree: format!("w{n}"),
            barrier: false,
            cancelled_by: None,
            cause: None,
            stopped: false,
            abandoned: false,
            result: None,
            slow: None,
            slowed: false,
            batch: false,
            holds_red: false,
        }
    }

    fn owned(c: Candidate, host: &str) -> Candidate {
        Candidate { owner: Process { host: host.into(), ..c.owner.clone() }, ..c }
    }

    fn reserved(id: &str, host: &str) -> Barrier {
        Barrier { id: id.into(), owner: Process { host: host.into(), pid: 9, started: "s".into() } }
    }

    /// The Offices-MBP chain of 2026-11-01: a dead host's round tip slow-gating,
    /// a live host's green successor behind it, stopped, and the dead host's
    /// pending turn and barrier.
    #[test]
    fn a_dead_hosts_members_are_taken_over_and_the_chain_admits_again() {
        let tip = Candidate { phase: Phase::Slow, ..owned(split(0), "far") };
        let behind = Candidate { phase: Phase::Green, stopped: true, ..owned(split(1), "here") };
        let mut chain = Chain {
            candidates: vec![tip, behind],
            pending: vec![reserved("p-far", "far"), reserved("p-here", "here")],
            barriers: vec![reserved("b-far", "far")],
            heartbeats: [("far".to_string(), 1000), ("here".to_string(), 1000)].into(),
        };
        assert!(!chain.can_assign(false), "the slow round holds the chain");
        chain.abandon("far");
        let (tip, behind) = (&chain.candidates[0], &chain.candidates[1]);
        assert_eq!(
            (tip.phase, tip.cause.as_deref(), tip.abandoned, tip.stopped, tip.active()),
            (Phase::Cancelled, Some("owner_died"), true, false, false),
            "never confirmed stopped, yet no longer holding the chain"
        );
        assert_eq!(
            (behind.phase, behind.cancelled_by.as_deref(), behind.abandoned),
            (Phase::Cancelled, Some("c0"), false),
            "its successor cancels as behind any failed predecessor"
        );
        let ids = |b: &[Barrier]| b.iter().map(|b| b.id.clone()).collect::<Vec<_>>();
        assert_eq!((ids(&chain.pending), ids(&chain.barriers)), (vec!["p-here".into()], vec![]));
        assert_eq!(chain.heartbeats.keys().collect::<Vec<_>>(), ["here"]);
        assert!(chain.can_assign(false), "a turn is assigned a candidate");
    }

    #[test]
    fn a_dead_hosts_successor_on_a_live_host_holds_the_chain_until_its_owner_stops_it() {
        let tip = Candidate { phase: Phase::Slow, ..owned(split(0), "far") };
        let behind = Candidate { phase: Phase::Validating, ..owned(split(1), "here") };
        let mut chain = Chain { candidates: vec![tip, behind], ..Chain::default() };
        chain.abandon("far");
        assert_eq!(chain.candidates[1].phase, Phase::Cancelling);
        assert!(!chain.can_assign(false), "its gate on a live host is not confirmed gone");
    }

    #[test]
    fn a_turn_stamps_its_host_while_it_owns_a_member_and_only_when_due() {
        let mut chain = Chain {
            candidates: vec![owned(candidate(0), "here")],
            heartbeats: [("gone".to_string(), 10)].into(),
            ..Chain::default()
        };
        assert!(chain.beat("here", 1000), "a first stamp");
        assert_eq!(
            chain.heartbeats,
            [("here".to_string(), 1000)].into(),
            "an owner of nothing is dropped"
        );
        assert!(!chain.beat("here", 1000 + crate::lease::BEAT_EVERY), "not due yet");
        assert!(chain.beat("here", 1001 + crate::lease::BEAT_EVERY));
        assert!(!chain.beat("elsewhere", 5000), "a host that owns nothing is not stamped");
        assert!(chain.beats("here", 1001 + crate::lease::BEAT_EVERY + crate::lease::LEASE));
        assert!(!chain.beats("here", 1002 + crate::lease::BEAT_EVERY + crate::lease::LEASE));
        assert!(!chain.beats("elsewhere", 5000));
    }

    /// A candidate whose commit declares a fast gate and a slow gate: its own
    /// gate is the fast one.
    fn split(n: usize) -> Candidate {
        let slow = Slow { command: "slow".into(), seconds: 60, whole: "whole".into() };
        Candidate { command: "fast".into(), slow: Some(slow), ..candidate(n) }
    }

    #[test]
    fn an_ended_candidate_nothing_holds_is_pruned_unless_a_kept_one_depends_on_it() {
        let ended = |n, phase| Candidate { phase, stopped: true, ..candidate(n) };
        let mut chain = Chain {
            candidates: vec![
                ended(0, Phase::Landed),
                Candidate { predecessor: "base".into(), ..ended(1, Phase::Red) },
                Candidate { cancelled_by: Some("c1".into()), ..ended(2, Phase::Cancelled) },
                ended(3, Phase::Landed),
                candidate(4),
            ],
            ..Chain::default()
        };
        // c2 is still held; c1 cancelled it, and c4 stands on c3.
        chain.prune(|c| c.id != "c2");
        let ids: Vec<_> = chain.candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["c1", "c2", "c3", "c4"]);
        chain.candidates.retain(|c| c.id != "c4");
        chain.prune(|_| true);
        assert!(chain.candidates.is_empty(), "{:?}", chain.candidates);
    }

    #[test]
    fn a_round_runs_once_at_the_tip_when_every_candidate_is_fast_green() {
        let mut chain = Chain { candidates: (0..3).map(split).collect(), ..Chain::default() };
        chain.finish("c0", ran(true));
        chain.finish("c2", ran(true));
        assert!(chain.round_tip().is_none(), "c1 is still fast-gating");
        assert!(!chain.ripe("c0", "base"), "a fast-green candidate does not land alone");
        chain.finish("c1", ran(true));
        assert_eq!(chain.round_tip().map(|c| c.id.as_str()), Some("c2"));
        assert!(!chain.start_round("c1"), "only the tip's turn runs the round");
        assert!(chain.start_round("c2"));
        assert_eq!(chain.candidates[2].waiting(), "slow_gating");
        assert_eq!(chain.candidates[0].waiting(), "waiting_for_round");
        let round = |chain: &Chain, id| chain.round_of(id).map(|c| c.id.clone());
        assert_eq!(round(&chain, "c0").as_deref(), Some("c2"));
        assert_eq!(round(&chain, "c2").as_deref(), Some("c2"));
        // A candidate assigned during the round waits for the next one.
        let mut later = split(3);
        later.phase = Phase::Green;
        chain.candidates.push(later);
        assert!(chain.round_tip().is_none());
        assert_eq!(round(&chain, "c3"), None, "it waits for the next round");
        chain.finish("c2", ran(true));
        assert!(chain.candidates[2].slowed);
        assert!(chain.ripe("c2", "base"));
        assert!(!chain.ripe("c2", "m0"), "the round's first candidate stands on the base");
        chain.land("c2");
        let landed: Vec<_> = chain.candidates.iter().map(|c| c.phase).collect();
        assert_eq!(landed, [Phase::Landed, Phase::Landed, Phase::Landed, Phase::Green]);
        assert_eq!(chain.round_tip().map(|c| c.id.as_str()), Some("c3"));
    }

    /// A candidate of a batch round, not gated alone.
    fn batched(n: usize) -> Candidate {
        Candidate { phase: Phase::Batched, stopped: true, batch: true, ..split(n) }
    }

    /// Runs a batch round over `n` candidates, whose gate is red at a
    /// candidate whose tree holds one of `reds` → the candidates it gated,
    /// in order, and the one it dropped.
    fn bisected(n: usize, reds: &[usize]) -> (Vec<String>, Option<String>) {
        let mut chain = Chain { candidates: (0..n).map(batched).collect(), ..Chain::default() };
        let mut gated = vec![];
        loop {
            match chain.batch_step() {
                Some(BatchStep::Gate(id)) => {
                    assert!(chain.start_batched(&id));
                    assert!(!chain.start_batched(&id), "one gate at a time");
                    let at: usize = id[1..].parse().unwrap();
                    chain.finish(&id, ran(!reds.iter().any(|&red| red <= at)));
                    stop(&mut chain, &id);
                    gated.push(id);
                }
                Some(BatchStep::Drop(id)) => {
                    chain.drop_red(&id);
                    let red = chain.candidates.iter().find(|c| c.id == id).unwrap();
                    assert_eq!(red.decision(), Some(Decision::Red));
                    assert!(red.result.is_some(), "it is sent back with the red run");
                    let behind = chain.candidates.iter().skip_while(|c| c.id != id).skip(1);
                    assert!(behind.clone().all(|c| c.cause.as_deref() == Some("rebuilt")));
                    assert!(behind.clone().all(|c| c.decision() == Some(Decision::Cancelled)));
                    let kept = chain.batch_step();
                    let before: Vec<_> =
                        chain.candidates.iter().take_while(|c| c.id != id).collect();
                    match before.last() {
                        Some(last) => {
                            assert_eq!(kept, Some(BatchStep::Land(last.id.clone())));
                            assert!(chain.ripe(&last.id, "base"), "the candidates before it land");
                        }
                        None => assert_eq!(kept, None),
                    }
                    return (gated, Some(id));
                }
                Some(BatchStep::Land(id)) => {
                    assert!(chain.ripe(&id, "base"));
                    return (gated, None);
                }
                None => panic!("the round stopped: {chain:?}"),
            }
        }
    }

    fn stop(chain: &mut Chain, id: &str) {
        chain.candidates.iter_mut().filter(|c| c.id == id).for_each(|c| c.stopped = true);
    }

    #[test]
    fn a_green_batch_round_gates_its_tip_alone_and_lands_it() {
        assert_eq!(bisected(5, &[]), (vec!["c4".to_string()], None));
    }

    /// The first red candidate is found in at most ceil(log2 N)+1 gates,
    /// wherever it is, and the gate is never run twice on one candidate.
    #[test]
    fn a_red_batch_round_is_bisected_to_its_first_red_candidate() {
        for n in 1..=9usize {
            let most = n.next_power_of_two().trailing_zeros() as usize + 1;
            for red in 0..n {
                let (gated, dropped) = bisected(n, &[red]);
                assert_eq!(dropped, Some(format!("c{red}")), "{n} candidates, red at {red}");
                assert!(gated.len() <= most, "{n} candidates, red at {red}: {gated:?}");
                let mut distinct = gated.clone();
                distinct.sort();
                distinct.dedup();
                assert_eq!(distinct.len(), gated.len());
            }
            // Two reds: the first is found; the second is the rebuilt round's.
            if n >= 2 {
                assert_eq!(bisected(n, &[n / 2, n - 1]).1, Some(format!("c{}", n / 2)));
            }
        }
        let (gated, _) = bisected(4, &[1]);
        assert_eq!(gated, ["c3", "c1", "c0"]);
    }

    #[test]
    fn a_batch_round_waits_while_its_gate_runs_and_leaves_a_per_candidate_chain_alone() {
        let mut chain = Chain { candidates: (0..3).map(batched).collect(), ..Chain::default() };
        assert!(chain.start_batched("c2"));
        assert_eq!(chain.batch_step(), None, "its gate runs");
        assert!(chain.round_tip().is_none(), "a batch round runs no per-candidate round");
        let chain = Chain { candidates: (0..3).map(split).collect(), ..Chain::default() };
        assert_eq!(chain.batch_step(), None);
        let mut chain = chain;
        chain.drop_red("c0");
        assert!(chain.candidates.iter().all(|c| c.valid()), "drops only a batch round's");
    }

    #[test]
    fn a_round_waits_for_every_turn_that_asked_to_land() {
        let mut chain = Chain { candidates: (0..2).map(split).collect(), ..Chain::default() };
        chain.finish("c0", ran(true));
        chain.finish("c1", ran(true));
        let owner = chain.candidates[0].owner.clone();
        chain.pending.push(Barrier { id: "waiting".into(), owner });
        assert!(chain.round_tip().is_none(), "a PR still waiting for a lane is in the round");
        assert!(chain.can_assign(false), "the window is not bounded by the lanes");
        chain.pending.clear();
        assert_eq!(chain.round_tip().map(|c| c.id.as_str()), Some("c1"));
    }

    #[test]
    fn a_red_round_is_not_bisected_its_tip_keeps_the_failure() {
        let mut chain = Chain { candidates: (0..4).map(split).collect(), ..Chain::default() };
        for c in ["c0", "c1", "c2"] {
            chain.finish(c, ran(true));
            chain.candidates.iter_mut().for_each(|c| c.stopped = c.phase == Phase::Green);
        }
        chain.candidates.pop();
        assert!(chain.start_round("c2"));
        let mut later = split(3);
        later.phase = Phase::Validating;
        chain.candidates.push(later);
        chain.finish("c2", ran(false));
        let phases: Vec<_> = chain.candidates.iter().map(|c| c.phase).collect();
        assert_eq!(phases, [Phase::Cancelled, Phase::Cancelled, Phase::Red, Phase::Cancelling]);
        let causes: Vec<_> = chain.candidates.iter().map(|c| c.cause.as_deref()).collect();
        assert_eq!(
            causes,
            [Some("round_red"), Some("round_red"), Some("round_red"), Some("predecessor_failed")]
        );
        assert!(chain.candidates.iter().all(|c| c.cancelled_by.as_deref() == Some("c2")));
        assert!(chain.round_tip().is_none(), "no smaller round runs after a red one");
    }

    /// A gate red only on TIMEOUTs it cannot classify under overload is no
    /// red (sunfmin/control#617): its round is not failed, and its own
    /// suffix goes with it unsettled, not failed.
    #[test]
    fn an_unsettled_gate_fails_no_round_and_its_suffix_goes_unsettled() {
        let mut chain = Chain { candidates: (0..4).map(split).collect(), ..Chain::default() };
        for c in ["c0", "c1", "c2"] {
            chain.finish(c, ran(true));
            chain.candidates.iter_mut().for_each(|c| c.stopped = c.phase == Phase::Green);
        }
        chain.candidates.pop();
        assert!(chain.start_round("c2"));
        let mut later = split(3);
        later.phase = Phase::Validating;
        chain.candidates.push(later);
        chain.finish("c2", unsettled());
        let phases: Vec<_> = chain.candidates.iter().map(|c| c.phase).collect();
        assert_eq!(phases, [Phase::Green, Phase::Green, Phase::Cancelling, Phase::Cancelling]);
        let causes: Vec<_> = chain.candidates.iter().map(|c| c.cause.as_deref()).collect();
        assert_eq!(causes, [None, None, Some("gate_unsettled"), Some("predecessor_unsettled")]);
        assert_eq!(chain.candidates[2].result, Some(unsettled()));
        assert_eq!(chain.round_tip().map(|c| c.id.as_str()), Some("c1"), "the round runs again");
        // A fast gate unsettled cancels its successor unsettled, not failed.
        let mut chain = Chain { candidates: vec![candidate(0), candidate(1)], ..Chain::default() };
        chain.finish("c0", unsettled());
        let causes: Vec<_> = chain.candidates.iter().map(|c| c.cause.as_deref()).collect();
        assert_eq!(causes, [Some("gate_unsettled"), Some("predecessor_unsettled")]);
        assert!(chain.candidates.iter().all(|c| c.phase != Phase::Red));
    }

    #[test]
    fn a_whole_gated_candidate_lands_the_fast_green_ones_before_it() {
        let mut chain = Chain { candidates: vec![split(0), candidate(1)], ..Chain::default() };
        chain.finish("c0", ran(true));
        chain.finish("c1", ran(true));
        assert!(chain.round_tip().is_none(), "its gate was the whole gate");
        assert!(chain.ripe("c1", "base"));
        chain.land("c1");
        assert!(chain.candidates.iter().all(|c| c.phase == Phase::Landed));
        let mut chain = Chain { candidates: vec![candidate(0), candidate(1)], ..Chain::default() };
        chain.finish("c0", ran(true));
        chain.finish("c1", ran(true));
        assert!(!chain.ripe("c1", "base"), "a whole-gated one ahead lands by its own turn");
        assert!(chain.ripe("c0", "base"));
    }

    #[test]
    fn late_completion_never_resurrects_an_invalidated_suffix_or_erases_the_root_failure() {
        let mut chain = Chain { candidates: (0..3).map(candidate).collect(), ..Chain::default() };
        chain.finish("c0", ran(false));
        let invalidated = chain.clone();
        for c in ["c1", "c2"] {
            chain.finish(c, ran(true));
            chain.finish(c, ran(false));
        }
        assert_eq!(chain, invalidated);
        assert_eq!(chain.candidates[0].phase, Phase::Red);
        assert!(!chain.can_assign(false), "unconfirmed cancellation keeps the window occupied");
    }

    #[test]
    fn a_pending_serial_barrier_drains_the_window_without_reserving_capacity() {
        let mut chain = Chain { candidates: vec![candidate(0)], ..Chain::default() };
        chain.barriers.push(Barrier { id: "cut".into(), owner: chain.candidates[0].owner.clone() });
        assert!(!chain.can_assign(false));
        assert!(!chain.can_assign(true));
        chain.candidates[0].phase = Phase::Landed;
        chain.candidates[0].stopped = true;
        assert!(chain.can_assign(true));
        assert!(!chain.can_assign(false));
    }

    #[test]
    fn a_fast_green_barrier_starts_its_round_beside_turns_it_holds_back() {
        let mut chain = Chain { candidates: vec![split(0)], ..Chain::default() };
        let owner = chain.candidates[0].owner.clone();
        chain.pending.push(Barrier { id: "waiting".into(), owner: owner.clone() });
        chain.finish("c0", ran(true));
        assert!(chain.round_tip().is_none(), "a pending turn may still join");
        chain.candidates[0].barrier = true;
        assert_eq!(chain.round_tip().map(|c| c.id.as_str()), Some("c0"));
        chain.candidates[0].barrier = false;
        chain.barriers.push(Barrier { id: "cut".into(), owner });
        assert_eq!(chain.round_tip().map(|c| c.id.as_str()), Some("c0"), "a cut waits");
    }

    #[test]
    fn an_external_push_of_an_unfinished_candidate_does_not_settle_its_prefix() {
        let mut chain = Chain { candidates: (0..3).map(candidate).collect(), ..Chain::default() };
        chain.finish("c1", ran(true));
        chain.observe_base("m1");
        assert!(chain.candidates.iter().all(|c| !c.valid() && c.phase != Phase::Landed));
        assert!(chain.candidates.iter().all(|c| c.cause.as_deref() == Some("base_moved")));
    }

    #[test]
    fn exhaust_three_candidates_through_completion_invalidation_exit_and_publication() {
        assert!(explore(candidate) > 500, "exploration stopped exercising interleavings");
    }

    #[test]
    fn exhaust_three_candidates_through_fast_results_rounds_exit_and_publication() {
        assert!(explore(split) > 500, "exploration stopped exercising interleavings");
    }

    /// Exactly three assigned candidates and one pending turn, two results per
    /// gate, rounds where they declare a slow gate, cancellation and exit
    /// confirmations, external moves and publication. Enumerates every reachable state, not arbitrary
    /// graph sizes or OS behaviour → how many states it reached.
    fn explore(made: fn(usize) -> Candidate) -> usize {
        let candidates: Vec<Candidate> = (0..3).map(made).collect();
        let owner = candidates[0].owner.clone();
        let waiting = vec![Barrier { id: "pending".into(), owner }];
        let initial =
            (Chain { candidates, pending: waiting, ..Chain::default() }, "base".to_string());
        let mut pending = VecDeque::from([initial]);
        let mut seen = HashSet::new();
        while let Some((chain, base)) = pending.pop_front() {
            let key = serde_json::to_string(&(&chain, &base)).unwrap();
            if !seen.insert(key) {
                continue;
            }
            assert!(seen.len() < 200_000, "the exploration bound changed");
            for (i, c) in chain.candidates.iter().enumerate() {
                if c.cancelled_by.is_some() {
                    assert!(!c.valid());
                }
                if c.phase == Phase::Landed {
                    assert!(c.result.is_some() && c.stopped);
                    // Landings are a prefix: nothing before it is left valid.
                    assert!(chain.candidates[..i].iter().all(|b| !b.valid()));
                }
                assert!(c.phase != Phase::Slow || !chain.joinable(), "a round waited");
                assert!(
                    c.phase != Phase::Slow
                        || chain
                            .candidates
                            .iter()
                            .all(|o| { o.id == c.id || o.phase != Phase::Slow }),
                    "one round at a time"
                );
            }
            for i in 0..3 {
                let c = &chain.candidates[i];
                if matches!(c.phase, Phase::Validating | Phase::Slow) {
                    for green in [true, false] {
                        let mut next = chain.clone();
                        next.finish(&c.id, ran(green));
                        pending.push_back((next, base.clone()));
                    }
                }
                let mut started = chain.clone();
                if started.start_round(&c.id) {
                    pending.push_back((started, base.clone()));
                }
                if !c.stopped {
                    let mut next = chain.clone();
                    next.candidates[i].stopped = true;
                    if c.phase == Phase::Cancelling {
                        next.candidates[i].phase = Phase::Cancelled;
                    }
                    pending.push_back((next, base.clone()));
                }
                let members = chain.members(&c.id);
                let publish = Publication::new(
                    members.iter().all(|m| m.valid()),
                    chain.ripe(&c.id, &base),
                    members.first().is_some_and(|m| m.predecessor == base),
                    true,
                    true,
                    members.iter().all(|m| m.stopped),
                );
                if publish.is_some() {
                    assert!(members.iter().all(|m| m.cancelled_by.is_none() && m.result.is_some()));
                    let mut next = chain.clone();
                    next.land(&c.id);
                    pending.push_back((next, c.commit.clone()));
                }
                if c.valid() {
                    let mut next = chain.clone();
                    next.invalidate(&c.id, "head_changed");
                    pending.push_back((next, base.clone()));
                }
            }
            if !chain.pending.is_empty() {
                let mut next = chain.clone();
                next.pending.clear();
                pending.push_back((next, base.clone()));
            }
            if base != "outside" {
                let mut next = chain.clone();
                next.observe_base("outside");
                pending.push_back((next, "outside".into()));
            }
        }
        seen.len()
    }
}

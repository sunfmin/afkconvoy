//! A tick: one reconciliation pass over the working set, and the pacing of the
//! loop that runs one after another.
//!
//! What a pass does, and in what order, is decided here. The effect layer
//! hands in the one function that carries a step out; it holds no rule about
//! what comes next, and no slot count.

use flux_rs::{attrs::*, macros::qualifier};

use crate::board::Stage;
use crate::patrol::Standing;
use crate::plan::{Status, WorkingSet};
use crate::precedent;

/// One thing a pass does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickStep {
    /// Settle a claim of mine whose work has landed — its issue is closed, or
    /// the landing was cut short of closing it: clean up, release it last.
    Settle { issue: u64 },
    /// Keep a train worker on the landing train while a PR of mine is on it.
    /// The train worker holds no claim and takes no slot.
    TendTrain,
    /// Ask after the worker of a claim of mine with no PR, and settle what it
    /// declared when it stopped.
    Judge { issue: u64 },
    /// Ask after the worker of a claim of mine whose PR is open and not on the
    /// train yet.
    Watch { issue: u64 },
    /// Carry out what the stage of a claim of mine declared, or ask after its
    /// worker while it has declared nothing.
    Advance { issue: u64 },
    /// Hand an unclaimed issue to a human: claim it, escalate it, release it.
    Escalate { issue: u64 },
    /// Delete a stale claim on a closed issue: a lock on nothing.
    Unlock { issue: u64 },
    /// Take a stale claim into a free slot, and continue it.
    Reclaim { issue: u64 },
    /// Start a worker on a frontier issue, into a free slot — or on an issue
    /// whose claim this pass ended with a worker due on it
    /// (`Outcome::HandedOn`), into the slot that freed.
    Dispatch { issue: u64 },
    /// End the run of the patrol at this place among the working set's, when
    /// it is over: ask after the worker of a run of mine, or delete the claim
    /// of a run whose fleet is presumed dead.
    EndPatrol { patrol: usize },
    /// Run the due patrol at this place among the working set's, into a free
    /// slot.
    Patrol { patrol: usize },
    /// End the proposal at this place among the working set's, when its run
    /// is over: ask after the worker of a run of mine and close a PR it
    /// opened that is no proposal, or delete the claim of a run whose fleet
    /// is presumed dead.
    EndProposal { proposal: usize },
    /// Start a worker on the due proposal at this place among the working
    /// set's, into a free slot.
    Propose { proposal: usize },
    /// Show on the issue's status board the stage a claim of mine is at.
    Board { issue: u64, stage: Stage, pr: Option<u64> },
    /// Bring the `stage:` label of an issue that follows a workflow to the
    /// position its stage records give it.
    StageLabel { issue: u64 },
    /// Refresh my heartbeat, if it is due.
    Heartbeat,
}

/// How a step ended, as the pass reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It did what it names. For `Settle`, `Judge`, `Watch` and `Advance`
    /// that ends the claim: its slot is free.
    Done,
    /// `Advance` ended the claim once its stage had declared, and left the
    /// issue where its stage records put it: its slot is free, and the
    /// worker due on it there, when one is (`Mine::due`), is this pass's to
    /// start.
    HandedOn,
    /// A silent worker was told to carry on: its claim is still held.
    Nudged,
    /// A worker was started on the claim again: it is still held.
    Continued,
    /// `Watch` replaced a silent joining worker, the once that is allowed: the
    /// claim is still held.
    Restarted,
    /// `TendTrain` gave the train up, its worker silent past a nudge: nothing
    /// landed, and its PRs join the next one.
    Abandoned,
    /// There was nothing for it to do.
    Nothing,
    /// A peer holds the issue — it won the race, or its heartbeat came back:
    /// nothing was started, and no slot was taken.
    Lost,
    /// It failed. Whatever claim it was about is still held.
    Failed,
}

/// The claims a fleet holds against its bound, as a pass takes and frees slots.
#[refined_by(held: int, limit: int)]
struct Slots {
    #[field(usize[held])]
    held: usize,
    #[field(usize[limit])]
    limit: usize,
}

impl Slots {
    #[spec(fn(limit: usize, held: usize) -> Slots[held, limit])]
    fn new(limit: usize, held: usize) -> Slots {
        Slots { held, limit }
    }

    #[spec(fn(&Slots[@s]) -> bool[s.held < s.limit])]
    fn has_free(&self) -> bool {
        self.held < self.limit
    }

    /// A claim is taken only into a free slot.
    #[spec(fn(self: &mut Self[@s]) requires s.held < s.limit ensures self: Self[s.held + 1, s.limit])]
    fn take(&mut self) {
        self.held += 1;
    }

    #[spec(fn(self: &mut Self[@s]) requires s.held > 0 ensures self: Self[s.held - 1, s.limit])]
    fn free(&mut self) {
        self.held -= 1;
    }

    #[spec(fn(&Slots[@s]) -> usize[s.held])]
    fn held(&self) -> usize {
        self.held
    }
}

/// What one pass came to.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(held: int)]
pub struct Pass {
    /// Every step carried out, in order, with how it ended.
    pub did: Vec<(TickStep, Outcome)>,
    /// The claims held as the pass ended.
    #[field(usize[held])]
    pub held: usize,
    /// Frontier issues the pass left undispatched.
    pub frontier_remaining: usize,
}

impl Pass {
    /// Whether the pass changed anything: a pass that only looked, refreshed
    /// its heartbeat or brought a status board or a `stage:` label up to date
    /// did no work.
    pub fn did_work(&self) -> bool {
        self.did.iter().any(|(step, outcome)| {
            !matches!(
                step,
                TickStep::Heartbeat | TickStep::Board { .. } | TickStep::StageLabel { .. }
            ) && matches!(
                outcome,
                Outcome::Done
                    | Outcome::HandedOn
                    | Outcome::Nudged
                    | Outcome::Continued
                    | Outcome::Restarted
                    | Outcome::Abandoned
            )
        })
    }

    /// Whether the pass left the tick after it a worker to start that it
    /// could not start itself: under a bound of `concurrency` claims a slot
    /// is free, and it advanced an issue or settled one. What follows either
    /// — a consult, the join, an issue the landing unblocked — is on no
    /// frontier before the next rebuild. Not an issue the pass started the
    /// next stage's worker on: nothing is left of that. A slot any other
    /// step freed was the pass's own to fill: it ends claims before it takes
    /// any.
    pub fn follow_up_due(&self, concurrency: usize) -> bool {
        if self.held >= concurrency {
            return false;
        }
        for (step, outcome) in &self.did {
            let left = match step {
                TickStep::Settle { .. } => *outcome == Outcome::Done,
                TickStep::Advance { issue } => {
                    *outcome == Outcome::Done
                        || *outcome == Outcome::HandedOn && !dispatched(&self.did, *issue)
                }
                _ => false,
            };
            if left {
                return true;
            }
        }
        false
    }
}

/// Whether `did` holds a dispatch of `issue`, however it ended.
fn dispatched(did: &[(TickStep, Outcome)], issue: u64) -> bool {
    for (step, _) in did {
        if *step == (TickStep::Dispatch { issue }) {
            return true;
        }
    }
    false
}

/// Runs one pass over `set` under a bound of `concurrency` claims — what
/// `WorkingSet::bound` makes of the config's — with `held` slots taken as it
/// starts (`WorkingSet::held`): settle the
/// claims whose work landed, tend the landing train, ask after the workers
/// not on the train yet — starting, into the slot its claim freed, the worker
/// of the stage a verdict led to — end the patrol runs and the proposals that are over, delete the stale
/// locks on closed issues, hand to a
/// human the unclaimed issues that call for one, take stale
/// claims, then the due patrols, then frontier issues into the free slots,
/// lowest number first, then the due proposals of precedents,
/// bring the status boards and the `stage:` labels up to date, then refresh
/// the heartbeat. `carry_out` performs one
/// step and says how it ended; a step that fails leaves its claim held, and
/// the rest of the pass goes on.
///
/// With a fleet lock standing, the pass leaves alone every claim of mine on
/// another issue whose worker is not on the train yet — what it declared is
/// acted on once no lock stands — and takes no stale claim but a lock
/// issue's. The frontier it is handed holds lock issues only, and no patrol
/// and no proposal in it is due.
///
/// The claims held afterwards are at most `concurrency`, or no more than
/// before: a pass never takes a claim past the bound.
#[spec(fn(set: &WorkingSet, concurrency: usize, held: usize, carry_out: F)
    -> Pass{pass: pass.held <= concurrency || pass.held <= held})]
pub fn run_pass<F>(set: &WorkingSet, concurrency: usize, held: usize, mut carry_out: F) -> Pass
where
    F: FnMut(TickStep) -> Outcome,
{
    let mut did = Vec::new();
    let held = end_claims(set, Sweep::Landed, concurrency, held, &mut did, &mut carry_out);
    if set.mine.iter().any(|mine| mine.status == Status::Joined) {
        let step = TickStep::TendTrain;
        did.push((step, carry_out(step)));
    }
    let held = end_claims(set, Sweep::Stopped, concurrency, held, &mut did, &mut carry_out);
    let held = end_patrols(set, held, &mut did, &mut carry_out);
    let held = end_proposals(set, held, &mut did, &mut carry_out);
    for lock in &set.stale_closed {
        let step = TickStep::Unlock { issue: lock.number };
        did.push((step, carry_out(step)));
    }
    let held = hand_over(set, concurrency, held, &mut did, &mut carry_out);
    let held = fill_slots(&takes(set), concurrency, held, &mut did, &mut carry_out);
    for step in boards(set, &did) {
        did.push((step, carry_out(step)));
    }
    for issue in &set.flowing {
        let step = TickStep::StageLabel { issue: issue.number };
        did.push((step, carry_out(step)));
    }
    if held > 0 {
        let step = TickStep::Heartbeat;
        did.push((step, carry_out(step)));
    }
    let left = set.frontier.iter().filter(|issue| !dispatched(&did, issue.number)).count();
    Pass { did, held, frontier_remaining: left }
}

/// Which of my claims one sweep of a pass looks at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sweep {
    /// Those whose work has landed: settled.
    Landed,
    /// Those not on the train yet — no PR, or one still joining: their
    /// workers asked after.
    Stopped,
}

/// The step `sweep` takes for a claim of mine in `status`, when it takes one.
fn ending_step(sweep: Sweep, issue: u64, status: Status) -> Option<TickStep> {
    match (sweep, status) {
        (Sweep::Landed, Status::Closed | Status::Landed) => Some(TickStep::Settle { issue }),
        (Sweep::Stopped, Status::NoPr) => Some(TickStep::Judge { issue }),
        (Sweep::Stopped, Status::Joining) => Some(TickStep::Watch { issue }),
        (Sweep::Stopped, Status::AtStage) => Some(TickStep::Advance { issue }),
        _ => None,
    }
}

/// Carries out the step that may end each claim of mine `sweep` looks at →
/// the claims still held, under a bound of `concurrency`. A claim handed on
/// with a worker due on its issue has that worker started into the slot it
/// freed, when the bound leaves that slot free: ahead of everything else the
/// pass takes a slot for, and of an issue that was never started.
#[spec(fn(set: &WorkingSet, sweep: Sweep, concurrency: usize, held: usize,
        did: &mut Vec<(TickStep, Outcome)>, carry_out: &mut F)
    -> usize{after: after <= held})]
fn end_claims<F>(
    set: &WorkingSet,
    sweep: Sweep,
    concurrency: usize,
    held: usize,
    did: &mut Vec<(TickStep, Outcome)>,
    carry_out: &mut F,
) -> usize
where
    F: FnMut(TickStep) -> Outcome,
{
    let mut slots = Slots::new(concurrency, held);
    for mine in &set.mine {
        if sweep == Sweep::Stopped && set.locked_out(mine.number) {
            continue;
        }
        let Some(step) = ending_step(sweep, mine.number, mine.status) else {
            continue;
        };
        let outcome = carry_out(step);
        did.push((step, outcome));
        let ended = outcome == Outcome::Done || outcome == Outcome::HandedOn;
        if !ended || slots.held() == 0 {
            continue;
        }
        slots.free();
        if outcome == Outcome::HandedOn && mine.due.is_some() && slots.has_free() {
            let step = TickStep::Dispatch { issue: mine.number };
            let outcome = carry_out(step);
            did.push((step, outcome));
            // A start that failed after its claim was won still holds the claim.
            if outcome != Outcome::Lost {
                slots.take();
            }
        }
    }
    slots.held()
}

/// Carries out the step that ends each patrol run that may be over → the
/// slots still taken. A run of mine that ended frees its slot; a dead
/// fleet's held none of mine.
#[spec(fn(set: &WorkingSet, held: usize, did: &mut Vec<(TickStep, Outcome)>, carry_out: &mut F)
    -> usize{after: after <= held})]
fn end_patrols<F>(
    set: &WorkingSet,
    held: usize,
    did: &mut Vec<(TickStep, Outcome)>,
    carry_out: &mut F,
) -> usize
where
    F: FnMut(TickStep) -> Outcome,
{
    let mut slots = Slots::new(0, held);
    for (at, patrol) in set.patrols.iter().enumerate() {
        let mine = match patrol.standing {
            Standing::Running => true,
            Standing::StaleRun => false,
            _ => continue,
        };
        let step = TickStep::EndPatrol { patrol: at };
        let outcome = carry_out(step);
        did.push((step, outcome));
        if mine && outcome == Outcome::Done && slots.held() > 0 {
            slots.free();
        }
    }
    slots.held()
}

/// Carries out the step that ends each proposal whose run may be over → the
/// slots still taken. A run of mine that ended frees its slot; a dead
/// fleet's held none of mine.
#[spec(fn(set: &WorkingSet, held: usize, did: &mut Vec<(TickStep, Outcome)>, carry_out: &mut F)
    -> usize{after: after <= held})]
fn end_proposals<F>(
    set: &WorkingSet,
    held: usize,
    did: &mut Vec<(TickStep, Outcome)>,
    carry_out: &mut F,
) -> usize
where
    F: FnMut(TickStep) -> Outcome,
{
    let mut slots = Slots::new(0, held);
    for (at, proposal) in set.proposals.iter().enumerate() {
        let mine = match proposal.standing {
            precedent::Standing::Running => true,
            precedent::Standing::StaleRun => false,
            _ => continue,
        };
        let step = TickStep::EndProposal { proposal: at };
        let outcome = carry_out(step);
        did.push((step, outcome));
        if mine && outcome == Outcome::Done && slots.held() > 0 {
            slots.free();
        }
    }
    slots.held()
}

/// Whether `did` holds a step on `issue` that ended as `outcome`.
fn ended(did: &[(TickStep, Outcome)], issue: u64, outcome: Outcome) -> bool {
    for (step, how) in did {
        let about = match step {
            TickStep::Settle { issue }
            | TickStep::Judge { issue }
            | TickStep::Watch { issue }
            | TickStep::Advance { issue }
            | TickStep::Dispatch { issue } => Some(*issue),
            _ => None,
        };
        if about == Some(issue) && *how == outcome {
            return true;
        }
    }
    false
}

/// The status boards a pass brings up to date, once it has done everything
/// else: of each claim of mine it left held, at the stage the pass found it
/// at, and of each issue it dispatched. Never of a claim whose step ended it
/// — or failed, having got who knows how far: its last word may be written
/// already, and is not written over — unless the pass then started the
/// issue's next worker, under a claim that has a board of its own to show. A
/// claim taken from a peer is shown by the next pass, which knows where its
/// PR stands.
fn boards(set: &WorkingSet, did: &[(TickStep, Outcome)]) -> Vec<TickStep> {
    let mut steps: Vec<TickStep> = Vec::new();
    for mine in &set.mine {
        let Some(stage) = Stage::held(mine.status) else {
            continue;
        };
        let handed_on = ended(did, mine.number, Outcome::HandedOn);
        let ending = handed_on
            || ended(did, mine.number, Outcome::Done)
            || ended(did, mine.number, Outcome::Failed);
        // Only a dispatch ends as `Done` on an issue handed on.
        if handed_on && ended(did, mine.number, Outcome::Done) {
            steps.push(TickStep::Board { issue: mine.number, stage: Stage::Claimed, pr: mine.pr });
        } else if !ending {
            steps.push(TickStep::Board { issue: mine.number, stage, pr: mine.pr });
        }
    }
    for issue in &set.frontier {
        if ended(did, issue.number, Outcome::Done) {
            steps.push(TickStep::Board { issue: issue.number, stage: Stage::Claimed, pr: None });
        }
    }
    steps
}

/// What a pass may take a slot for, in the order it does: the stale claims —
/// work somebody began — then the patrols that are due, in the order the repo
/// names them, then the frontier; issues lowest number first. A patrol runs
/// once an interval, so it goes ahead of a frontier that is never empty. The
/// proposals of precedents that are due come last: no issue waits for one.
/// Never a stale claim a fleet lock keeps the fleet off.
fn takes(set: &WorkingSet) -> Vec<TickStep> {
    let mut stale: Vec<u64> = vec![];
    for claim in &set.stale {
        if !set.locked_out(claim.number) {
            stale.push(claim.number);
        }
    }
    stale.sort();
    let mut steps: Vec<TickStep> = Vec::new();
    for issue in stale {
        steps.push(TickStep::Reclaim { issue });
    }
    for (at, patrol) in set.patrols.iter().enumerate() {
        if patrol.standing == Standing::Due {
            steps.push(TickStep::Patrol { patrol: at });
        }
    }
    for issue in &set.frontier {
        steps.push(TickStep::Dispatch { issue: issue.number });
    }
    for (at, proposal) in set.proposals.iter().enumerate() {
        if proposal.standing == precedent::Standing::Due {
            steps.push(TickStep::Propose { proposal: at });
        }
    }
    steps
}

/// Hands each unclaimed issue that calls for a human to one → the claims
/// held. The claim an escalation is made under is taken into a free slot and
/// released as its last step, so only one that failed half way is still held.
#[spec(fn(set: &WorkingSet, concurrency: usize, held: usize, did: &mut Vec<(TickStep, Outcome)>,
        carry_out: &mut F)
    -> usize{after: held <= after && (after <= concurrency || after == held)})]
fn hand_over<F>(
    set: &WorkingSet,
    concurrency: usize,
    held: usize,
    did: &mut Vec<(TickStep, Outcome)>,
    carry_out: &mut F,
) -> usize
where
    F: FnMut(TickStep) -> Outcome,
{
    let mut slots = Slots::new(concurrency, held);
    for issue in &set.escalating {
        // What stays true on every turn, which Flux does not find alone.
        qualifier!(now: int, limit: int, before: int; now <= limit || now == before);
        qualifier!(before: int, now: int; before <= now);
        if !slots.has_free() {
            break;
        }
        let step = TickStep::Escalate { issue: issue.number };
        let outcome = carry_out(step);
        did.push((step, outcome));
        if outcome == Outcome::Failed {
            slots.take();
        }
    }
    slots.held()
}

/// Carries out `takes` while a slot is free → the claims held. A fleet over
/// its bound — after a takeover — takes nothing; what is past the free slots
/// is left as it is.
#[spec(fn(takes: &[TickStep], concurrency: usize, held: usize, did: &mut Vec<(TickStep, Outcome)>,
        carry_out: &mut F)
    -> usize{after: held <= after && (after <= concurrency || after == held)})]
fn fill_slots<F>(
    takes: &[TickStep],
    concurrency: usize,
    held: usize,
    did: &mut Vec<(TickStep, Outcome)>,
    carry_out: &mut F,
) -> usize
where
    F: FnMut(TickStep) -> Outcome,
{
    let mut slots = Slots::new(concurrency, held);
    for step in takes {
        // What stays true on every turn, which Flux does not find alone.
        qualifier!(now: int, limit: int, before: int; now <= limit || now == before);
        qualifier!(before: int, now: int; before <= now);
        if !slots.has_free() {
            break;
        }
        let outcome = carry_out(*step);
        did.push((*step, outcome));
        // A start that failed after its claim was won still holds the claim.
        if outcome != Outcome::Lost {
            slots.take();
        }
    }
    slots.held()
}

/// The loop's sleep while there is work, or a claim held, in seconds.
pub const BUSY_SECONDS: u64 = 90;
/// …and once the fleet has gone quiet.
pub const IDLE_SECONDS: u64 = 1500;
/// Empty ticks in a row before the fleet counts as quiet.
pub const IDLE_AFTER: u32 = 3;

/// The loop's next sleep, in seconds: busy while the last tick did work, a
/// claim is held, or fewer than `IDLE_AFTER` ticks in a row were empty.
///
/// While it holds a claim a fleet never sleeps longer than half the lease, so
/// its heartbeat cannot lapse and get a live claim taken.
#[spec(fn(did_work: bool, held: usize, empty_streak: u32)
    -> u64{sleep: sleep > 0 && (held > 0 => 2 * sleep <= crate::lease::LEASE)})]
pub fn pace(did_work: bool, held: usize, empty_streak: u32) -> u64 {
    if did_work || held > 0 || empty_streak < IDLE_AFTER { BUSY_SECONDS } else { IDLE_SECONDS }
}

/// The streak of empty ticks after one more tick: a tick is empty when it did
/// no work, holds nothing, and left nothing on the frontier.
#[spec(fn(streak: u32, did_work: bool, held: usize, frontier_remaining: usize) -> u32)]
pub fn empty_streak(streak: u32, did_work: bool, held: usize, frontier_remaining: usize) -> u32 {
    if did_work || held > 0 || frontier_remaining > 0 {
        0
    } else if streak < IDLE_AFTER {
        streak + 1
    } else {
        streak
    }
}

/// The most ticks the loop makes back to back, with no sleep between them:
/// a fault that makes every tick leave a follow-up cannot make it spin, nor
/// can a GitHub that stays out of reach.
pub const BACK_TO_BACK: u32 = 4;

/// The sleeps skipped in a row once one more tick has ended, `skipped` having
/// been before it and `follow_up_due` saying whether the next tick is due at
/// once: it left that tick a worker to start (`Pass::follow_up_due`), or it
/// failed on a read GitHub could not be reached for. More than zero: the next tick
/// starts now. Zero: the loop sleeps as `pace` says — after a tick that left
/// nothing, and after the last of `BACK_TO_BACK` ticks whatever it left.
#[spec(fn(skipped: u32, follow_up_due: bool) -> u32{after: after < BACK_TO_BACK})]
pub fn sleeps_skipped(skipped: u32, follow_up_due: bool) -> u32 {
    if follow_up_due && skipped < BACK_TO_BACK - 1 { skipped + 1 } else { 0 }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A sleep skipped after every tick that left a follow-up, however many
    /// ran back to back: a fault spins the loop.
    #[should_fail]
    #[spec(fn(skipped: u32{skipped < BACK_TO_BACK}, follow_up_due: bool)
        -> u32{after: after < BACK_TO_BACK})]
    fn a_sleep_skipped_with_no_look_at_the_run(skipped: u32, follow_up_due: bool) -> u32 {
        if follow_up_due { skipped + 1 } else { 0 }
    }

    /// A claim taken with no look at the bound.
    #[should_fail]
    fn a_claim_taken_past_the_bound(concurrency: usize, held: usize) {
        let mut slots = Slots::new(concurrency, held);
        slots.take();
    }

    /// A pass that claims to end under the bound, whatever it started with.
    #[should_fail]
    #[spec(fn(set: &WorkingSet, concurrency: usize, held: usize) -> Pass{pass: pass.held <= concurrency})]
    fn a_pass_that_sheds_claims_it_holds(
        set: &WorkingSet,
        concurrency: usize,
        held: usize,
    ) -> Pass {
        run_pass(set, concurrency, held, |_| Outcome::Failed)
    }

    /// A sleep of a whole lease: every claim held would be stale on waking.
    #[should_fail]
    #[spec(fn() -> u64{sleep: 2 * sleep <= crate::lease::LEASE})]
    fn a_sleep_of_a_whole_lease() -> u64 {
        crate::lease::LEASE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Dispatchable, Mine};

    fn set(frontier: &[u64], mine: &[(u64, Status)]) -> WorkingSet {
        WorkingSet {
            frontier: frontier
                .iter()
                .map(|&number| Dispatchable { number, title: String::new(), stage: None })
                .collect(),
            escalating: vec![],
            excluded: vec![],
            waiting_on_human: vec![],
            mine: mine
                .iter()
                .map(|&(number, status)| Mine { number, title: None, status, pr: None, due: None })
                .collect(),
            peer_live: vec![],
            stale: vec![],
            stale_closed: vec![],
            free_slots: 0,
            now: 0,
            locks: vec![],
            flowing: vec![],
            patrols: vec![],
            proposals: vec![],
        }
    }

    use Outcome::*;
    use TickStep::*;

    fn peers(numbers: &[u64]) -> Vec<crate::plan::PeerClaim> {
        let claim = |&number| crate::plan::PeerClaim { number, instance: None, sha: String::new() };
        numbers.iter().map(claim).collect()
    }

    fn patrols(standing: &[Standing]) -> Vec<crate::patrol::Row> {
        let row = |(at, standing): (usize, &Standing)| crate::patrol::Row {
            name: format!("p{at}"),
            standing: *standing,
            every: 3600,
            last_run: None,
            open: 0,
            max_open: 1,
            failure: None,
            claim: None,
        };
        standing.iter().enumerate().map(row).collect()
    }

    #[test]
    fn a_pass_ends_the_runs_that_may_be_over_and_runs_the_patrols_due() {
        use Standing::*;
        let mut set = set(&[7, 8], &[]);
        set.patrols = patrols(&[NotDue, Running, StaleRun, Due, Locked, Full, Failed, PeerRunning]);
        // One run of mine is held as it starts; it is over, and its slot is free again.
        let pass = run_pass(&set, 2, 1, |_| Done);
        let did: Vec<TickStep> = pass.did.iter().map(|(step, _)| *step).collect();
        let on_patrols: Vec<TickStep> = did
            .iter()
            .filter(|step| matches!(step, EndPatrol { .. } | TickStep::Patrol { .. }))
            .copied()
            .collect();
        assert_eq!(
            on_patrols,
            [EndPatrol { patrol: 1 }, EndPatrol { patrol: 2 }, TickStep::Patrol { patrol: 3 }]
        );
        // The patrol due is run before an issue is dispatched, and takes a slot as one does.
        let dispatched: Vec<&TickStep> =
            did.iter().filter(|step| matches!(step, Dispatch { .. })).collect();
        assert_eq!(dispatched, [&Dispatch { issue: 7 }]);
        assert_eq!(pass.held, 2);
        assert_eq!(pass.frontier_remaining, 1);
    }

    #[test]
    fn a_run_still_going_keeps_its_slot_and_a_patrol_lost_to_a_peer_takes_none() {
        use Standing::*;
        let mut set = set(&[7], &[]);
        set.patrols = patrols(&[Running, Due]);
        let pass = run_pass(&set, 2, 1, |step| match step {
            EndPatrol { .. } => Nothing,
            TickStep::Patrol { .. } => Lost,
            _ => Done,
        });
        assert!(pass.did.contains(&(Dispatch { issue: 7 }, Done)));
        assert_eq!(pass.held, 2);
        // With every slot held, a patrol due waits.
        let pass = run_pass(&set, 1, 1, |step| match step {
            EndPatrol { .. } => Nothing,
            _ => Done,
        });
        assert!(!pass.did.iter().any(|(step, _)| matches!(step, TickStep::Patrol { .. })));
        assert_eq!(pass.held, 1);
    }

    fn proposals(standing: &[precedent::Standing]) -> Vec<precedent::Row> {
        let row = |(at, standing): (usize, &precedent::Standing)| precedent::Row {
            ruling: precedent::Ruling { issue: 3, comment: at as u64 },
            standing: *standing,
            claim: None,
        };
        standing.iter().enumerate().map(row).collect()
    }

    #[test]
    fn a_pass_ends_the_proposals_that_may_be_over_and_starts_the_due_after_the_frontier() {
        use precedent::Standing::*;
        let mut set = set(&[7], &[]);
        set.proposals = proposals(&[Running, StaleRun, PeerRunning, Due, Full, Locked, Due]);
        // One run of mine is held as it starts; it is over, and its slot is free again.
        let pass = run_pass(&set, 2, 1, |_| Done);
        let did: Vec<TickStep> = pass.did.iter().map(|(step, _)| *step).collect();
        let taken: Vec<TickStep> = did
            .iter()
            .filter(|step| matches!(step, EndProposal { .. } | Propose { .. } | Dispatch { .. }))
            .copied()
            .collect();
        // No issue waits for a proposal: the frontier is dispatched first, and
        // the second proposal due finds no slot.
        assert_eq!(
            taken,
            [
                EndProposal { proposal: 0 },
                EndProposal { proposal: 1 },
                Dispatch { issue: 7 },
                Propose { proposal: 3 }
            ]
        );
        assert_eq!(pass.held, 2);
        assert!(pass.did_work());
    }

    #[test]
    fn a_proposal_still_going_keeps_its_slot_and_one_lost_to_a_peer_takes_none() {
        use precedent::Standing::*;
        let mut set = set(&[], &[]);
        set.proposals = proposals(&[Running, Due, Due]);
        let pass = run_pass(&set, 2, 1, |step| match step {
            EndProposal { .. } => Nothing,
            Propose { proposal: 1 } => Lost,
            _ => Done,
        });
        assert!(pass.did.contains(&(Propose { proposal: 2 }, Done)));
        assert_eq!(pass.held, 2);
    }

    #[test]
    fn the_stage_label_of_every_issue_that_follows_a_workflow_is_brought_up_and_is_no_work() {
        // Whoever holds it, or nobody: a position is not a claim's.
        let mut set = set(&[], &[(4, Status::NoPr)]);
        let flowing = |&number| crate::plan::Issue {
            number,
            title: String::new(),
            labels: vec!["flow:feature".to_string()],
            open_blockers: 0,
        };
        set.flowing = [4, 9].iter().map(flowing).collect();
        let pass = run_pass(&set, 3, 1, |step| match step {
            StageLabel { .. } => Done,
            _ => Nothing,
        });
        let did: Vec<TickStep> = pass.did.iter().map(|(step, _)| *step).collect();
        assert_eq!(
            did,
            [
                Judge { issue: 4 },
                Board { issue: 4, stage: Stage::Claimed, pr: None },
                StageLabel { issue: 4 },
                StageLabel { issue: 9 },
                Heartbeat
            ],
            "after the boards"
        );
        assert!(!pass.did_work(), "a label moved is not work: the fleet may still go quiet");
    }

    #[test]
    fn a_claim_at_a_stage_is_advanced_and_its_slot_is_free_once_its_stage_declared() {
        let set = set(&[5, 6], &[(1, Status::AtStage), (2, Status::AtStage)]);
        // #1's stage declared; #2's worker is at work
        let pass = run_pass(&set, 2, 2, |step| match step {
            Advance { issue: 2 } => Nothing,
            _ => Done,
        });
        assert_eq!(
            steps(&pass),
            [Advance { issue: 1 }, Advance { issue: 2 }, Dispatch { issue: 5 }, Heartbeat],
            "every stage worker takes one slot"
        );
        assert_eq!((pass.held, pass.frontier_remaining), (2, 1));
        let boards: Vec<TickStep> =
            pass.did.iter().map(|(step, _)| *step).filter(|s| matches!(s, Board { .. })).collect();
        assert_eq!(
            boards,
            [
                Board { issue: 2, stage: Stage::Claimed, pr: None },
                Board { issue: 5, stage: Stage::Claimed, pr: None }
            ],
            "never of the claim the pass ended"
        );
    }

    /// `set`, a worker being due at `stage` on each of `issues` once its
    /// claim of mine ends.
    fn due_on(mut set: WorkingSet, issues: &[u64], stage: &str) -> WorkingSet {
        for mine in set.mine.iter_mut().filter(|mine| issues.contains(&mine.number)) {
            mine.due = Some(stage.to_string());
        }
        set
    }

    #[test]
    fn a_claim_handed_on_has_the_worker_due_on_its_issue_started_into_the_slot_it_freed() {
        let one = due_on(set(&[], &[(4, Status::AtStage), (5, Status::NoPr)]), &[4], "review");
        let at_stage = [(4, Status::AtStage), (6, Status::AtStage), (9, Status::AtStage)];
        // A worker performs the stage #4 and #9 were left at; #6 waits for a human.
        let set = due_on(set(&[2, 5], &at_stage), &[4, 9], "review");
        let handed_on = |step| if matches!(step, Advance { .. }) { HandedOn } else { Done };
        let pass = run_pass(&set, 3, 3, handed_on);
        assert_eq!(
            steps(&pass),
            [
                Advance { issue: 4 },
                Dispatch { issue: 4 },
                Advance { issue: 6 },
                Advance { issue: 9 },
                Dispatch { issue: 9 },
                Dispatch { issue: 2 },
                Heartbeat
            ],
            "ahead of the frontier, which gets the slot nothing was due in"
        );
        assert_eq!((pass.held, pass.frontier_remaining), (3, 1));
        assert!(pass.did_work());
        let boards: Vec<TickStep> =
            pass.did.iter().map(|(step, _)| *step).filter(|s| matches!(s, Board { .. })).collect();
        assert_eq!(
            boards,
            [
                Board { issue: 4, stage: Stage::Claimed, pr: None },
                Board { issue: 9, stage: Stage::Claimed, pr: None },
                Board { issue: 2, stage: Stage::Claimed, pr: None }
            ],
            "of the claims its starts made, never of the one #6 was handed on under"
        );

        // A fleet over its bound starts nothing: the issue waits on the frontier.
        let full = run_pass(&one, 1, 2, |step| match step {
            Advance { .. } => HandedOn,
            _ => Nothing,
        });
        assert_eq!(steps(&full), [Advance { issue: 4 }, Judge { issue: 5 }, Heartbeat]);
        assert_eq!(full.held, 1);

        // A claim that ended any other way leaves nothing to start, whatever was due…
        let ended = run_pass(&set, 3, 3, |_| Done);
        assert!(!ended.did.contains(&(Dispatch { issue: 4 }, Done)), "{:?}", ended.did);
        // …and a start a peer won takes no slot: the frontier has it.
        let lost = run_pass(&set, 3, 3, |step| match step {
            Advance { .. } => HandedOn,
            Dispatch { issue: 4 } => Lost,
            _ => Done,
        });
        assert_eq!(lost.frontier_remaining, 0, "{:?}", lost.did);
        assert_eq!(lost.held, 3);
    }

    #[test]
    fn an_issue_that_calls_for_a_human_is_handed_over_into_a_free_slot_and_holds_none() {
        let mut calling = set(&[5], &[(1, Status::AtStage)]);
        let row =
            |&number| crate::plan::Excluded { number, title: String::new(), reason: String::new() };
        calling.escalating = [3, 4, 8].iter().map(row).collect();
        let at_work = |step| if matches!(step, Advance { .. }) { Nothing } else { Done };
        let pass = run_pass(&calling, 2, 1, at_work);
        assert_eq!(
            steps(&pass),
            [
                Advance { issue: 1 },
                Escalate { issue: 3 },
                Escalate { issue: 4 },
                Escalate { issue: 8 },
                Dispatch { issue: 5 },
                Heartbeat
            ],
            "ahead of what takes a slot for good"
        );
        assert_eq!(pass.held, 2);
        assert!(pass.did_work());
        // One that failed half way may still hold its claim: it counts.
        let cut = run_pass(&calling, 2, 1, |step| match step {
            Advance { .. } => Nothing,
            Escalate { issue: 3 } => Failed,
            _ => Done,
        });
        assert_eq!(steps(&cut), [Advance { issue: 1 }, Escalate { issue: 3 }, Heartbeat]);
        assert_eq!((cut.held, cut.frontier_remaining), (2, 1));
        // A fleet at its bound takes no claim, not even to give it up at once.
        let full = run_pass(&calling, 1, 1, at_work);
        assert_eq!(steps(&full), [Advance { issue: 1 }, Heartbeat]);
    }

    #[test]
    fn stale_claims_are_taken_ahead_of_the_frontier_into_the_free_slots() {
        let mut set = set(&[2, 4], &[(1, Status::NoPr)]);
        set.stale = peers(&[9, 3, 6]);
        set.stale_closed = peers(&[8]);
        let pass = run_pass(&set, 3, 1, left_at_work);
        let steps = steps(&pass);
        assert_eq!(
            steps,
            [
                Judge { issue: 1 },
                Unlock { issue: 8 },
                Reclaim { issue: 3 },
                Reclaim { issue: 6 },
                Heartbeat
            ],
            "lowest first; #9 past the free slots is left stale, and so is the frontier"
        );
        assert_eq!((pass.held, pass.frontier_remaining), (3, 2));
    }

    #[test]
    fn with_a_lock_standing_a_pass_leaves_every_other_claim_at_work_alone() {
        let mine = [
            (1, Status::NoPr),
            (2, Status::Joining),
            (3, Status::AtStage),
            (4, Status::Closed),
            (5, Status::Joined),
            (7, Status::NoPr),
        ];
        // #6, #7 and #8 are locks: a dead peer began #6, #7 is at work under
        // my claim, #8 is on the frontier.
        let mut locked = set(&[8], &mine);
        locked.locks = vec![6, 7, 8];
        locked.stale = peers(&[6, 9]);
        let bound = locked.bound(4);
        assert_eq!(bound, 7, "the three left alone take none of the four slots");
        let pass = run_pass(&locked, bound, 6, |step| match step {
            Judge { .. } | TendTrain => Nothing,
            _ => Done,
        });
        assert_eq!(
            steps(&pass),
            [
                Settle { issue: 4 },
                TendTrain,
                Judge { issue: 7 },
                Reclaim { issue: 6 },
                Dispatch { issue: 8 },
                Heartbeat
            ],
            "what landed is settled and the train tended; no stale claim but a lock's is taken"
        );
        assert_eq!((pass.held, pass.frontier_remaining), (7, 0));
        // The lock ended: what the workers left alone declared is acted on.
        locked.locks = vec![];
        let after = run_pass(&locked, locked.bound(4), 6, |_| Done);
        let acted: Vec<TickStep> = steps(&after).into_iter().take(5).collect();
        assert_eq!(
            acted,
            [
                Settle { issue: 4 },
                TendTrain,
                Judge { issue: 1 },
                Watch { issue: 2 },
                Advance { issue: 3 }
            ]
        );
    }

    #[test]
    fn a_stale_claim_a_peer_took_first_costs_no_slot() {
        let mut set = set(&[2], &[]);
        set.stale = peers(&[3, 6]);
        let pass = run_pass(&set, 1, 0, |step| match step {
            Reclaim { issue: 3 } => Lost,
            _ => Done,
        });
        let steps = steps(&pass);
        assert_eq!(steps, [Reclaim { issue: 3 }, Reclaim { issue: 6 }, Heartbeat]);
        assert_eq!(pass.held, 1);
    }

    #[test]
    fn a_fleet_over_its_bound_takes_nothing_until_it_is_back_under() {
        let mut set = set(&[2], &[(1, Status::NoPr), (5, Status::NoPr), (7, Status::Closed)]);
        set.stale = peers(&[3]);
        // three held under a bound of one — a takeover: only the settle frees anything
        let pass = run_pass(&set, 1, 3, left_at_work);
        let steps = steps(&pass);
        assert_eq!(steps, [Settle { issue: 7 }, Judge { issue: 1 }, Judge { issue: 5 }, Heartbeat]);
        assert_eq!(pass.held, 2);
    }

    /// Carries every step out, and finds every worker asked after at work.
    /// What a pass did, without the status boards it brought up to date.
    fn acts(pass: &Pass) -> Vec<(TickStep, Outcome)> {
        pass.did.iter().copied().filter(|(step, _)| !matches!(step, Board { .. })).collect()
    }

    fn steps(pass: &Pass) -> Vec<TickStep> {
        acts(pass).into_iter().map(|(step, _)| step).collect()
    }

    fn left_at_work(step: TickStep) -> Outcome {
        if matches!(step, Judge { .. } | Watch { .. } | TendTrain) { Nothing } else { Done }
    }

    #[test]
    fn a_pass_settles_then_dispatches_then_beats() {
        let set = set(&[5, 6, 7], &[(1, Status::NoPr), (2, Status::Closed)]);
        let pass = run_pass(&set, 3, 2, left_at_work);
        let steps = steps(&pass);
        assert_eq!(
            steps,
            [
                Settle { issue: 2 },
                Judge { issue: 1 },
                Dispatch { issue: 5 },
                Dispatch { issue: 6 },
                Heartbeat
            ]
        );
        assert_eq!((pass.held, pass.frontier_remaining), (3, 1));
        assert!(pass.did_work());
    }

    #[test]
    fn a_claim_is_taken_only_into_a_free_slot() {
        let over = set(&[5, 6], &[(1, Status::NoPr), (2, Status::NoPr), (3, Status::NoPr)]);
        let pass = run_pass(&over, 2, 3, left_at_work);
        let dispatched = pass.did.iter().any(|(step, _)| matches!(step, Dispatch { .. }));
        assert!(!dispatched, "{:?}", pass.did);
        assert_eq!((pass.held, pass.frontier_remaining), (3, 2));
        assert!(!pass.did_work());
    }

    #[test]
    fn a_settling_that_failed_keeps_its_slot() {
        let set = set(&[5], &[(1, Status::Closed)]);
        let pass =
            run_pass(&set, 1, 1, |step| if matches!(step, Settle { .. }) { Failed } else { Done });
        assert_eq!(acts(&pass), [(Settle { issue: 1 }, Failed), (Heartbeat, Done)]);
        assert_eq!(pass.held, 1);
    }

    #[test]
    fn a_lost_race_takes_no_slot_and_a_failed_start_does() {
        let set = set(&[5, 6, 7, 8], &[]);
        let pass = run_pass(&set, 2, 0, |step| match step {
            Dispatch { issue: 5 } => Lost,
            Dispatch { issue: 6 } => Failed,
            _ => Done,
        });
        let steps = steps(&pass);
        assert_eq!(
            steps,
            [Dispatch { issue: 5 }, Dispatch { issue: 6 }, Dispatch { issue: 7 }, Heartbeat]
        );
        assert_eq!((pass.held, pass.frontier_remaining), (2, 1));
    }

    #[test]
    fn a_stopped_claim_that_ends_frees_its_slot_and_the_train_takes_none() {
        let set = set(&[5, 6], &[(1, Status::NoPr), (2, Status::Joined), (3, Status::Joining)]);
        // #1's worker declared a verdict that ends its claim; #3's is at work on its join
        let ended = |step| if matches!(step, Watch { .. }) { Nothing } else { Done };
        let pass = run_pass(&set, 3, 3, ended);
        let steps = steps(&pass);
        assert_eq!(
            steps,
            [TendTrain, Judge { issue: 1 }, Watch { issue: 3 }, Dispatch { issue: 5 }, Heartbeat]
        );
        assert_eq!(pass.held, 3);
        let left = run_pass(&set, 3, 3, left_at_work);
        assert_eq!(
            acts(&left),
            [
                (TendTrain, Nothing),
                (Judge { issue: 1 }, Nothing),
                (Watch { issue: 3 }, Nothing),
                (Heartbeat, Done)
            ]
        );
        assert!(!left.did_work());
    }

    #[test]
    fn a_joining_claim_is_watched_and_only_its_escalation_frees_the_slot() {
        let set = set(&[5], &[(3, Status::Joining)]);
        for kept in [Nudged, Restarted, Continued] {
            let pass = run_pass(&set, 1, 1, |step| match step {
                Watch { .. } => kept,
                _ => Done,
            });
            assert_eq!(acts(&pass), [(Watch { issue: 3 }, kept), (Heartbeat, Done)]);
            assert!(pass.did_work() && pass.held == 1);
        }
        let pass = run_pass(&set, 1, 1, |_| Done);
        let steps = steps(&pass);
        assert_eq!(steps, [Watch { issue: 3 }, Dispatch { issue: 5 }, Heartbeat]);
    }

    #[test]
    fn the_boards_of_what_is_still_held_and_of_what_was_dispatched_are_brought_up_to_date() {
        let mine =
            [(1, Status::NoPr), (2, Status::Joined), (3, Status::Joining), (4, Status::Closed)];
        let held = set(&[5, 6], &mine);
        // #1 is escalated, #4 settled, #5 dispatched into the slot that frees, #6 lost to a peer
        let pass = run_pass(&held, 4, 4, |step| match step {
            Watch { .. } | TendTrain => Nothing,
            Dispatch { issue: 6 } => Lost,
            _ => Done,
        });
        let boards: Vec<TickStep> =
            pass.did.iter().map(|(step, _)| *step).filter(|s| matches!(s, Board { .. })).collect();
        use crate::board::Stage;
        assert_eq!(
            boards,
            [
                Board { issue: 2, stage: Stage::Joined, pr: None },
                Board { issue: 3, stage: Stage::Joining, pr: None },
                Board { issue: 5, stage: Stage::Claimed, pr: None },
            ],
            "never of a claim the pass ended: its last board is the ending's to write"
        );
        // …nor of one whose ending was cut: the last word may be written already
        let cut = run_pass(&held, 4, 4, |step| match step {
            Judge { .. } => Failed,
            _ => Nothing,
        });
        assert!(!cut.did.iter().any(|(step, _)| matches!(step, Board { issue: 1, .. })));
        let last = pass.did.len() - 1;
        assert_eq!(pass.did[last].0, Heartbeat);
        assert!(matches!(pass.did[last - 1].0, Board { .. }), "after everything else");
        // a pass that only brought boards up to date did no work
        let quiet = run_pass(&set(&[], &[(2, Status::Joined)]), 1, 1, |step| match step {
            TendTrain => Nothing,
            _ => Done,
        });
        assert!(!quiet.did_work());
    }

    #[test]
    fn a_fleet_holding_nothing_does_not_beat() {
        let pass = run_pass(&set(&[], &[]), 2, 0, |_| Done);
        assert_eq!(acts(&pass), []);
        assert!(!pass.did_work());
    }

    #[test]
    fn a_follow_up_is_due_after_an_advance_or_a_settle_that_left_a_slot_free() {
        let mine = [(1, Status::AtStage), (2, Status::Closed), (3, Status::NoPr)];
        let only = |ending: TickStep| {
            run_pass(
                &set(&[], &mine),
                3,
                3,
                move |step| if step == ending { Done } else { Nothing },
            )
        };
        assert!(only(Advance { issue: 1 }).follow_up_due(3), "the stage its verdict leads to");
        assert!(only(Settle { issue: 2 }).follow_up_due(3), "what its landing unblocked");
        // A claim handed on leaves the tick after it whoever is due on the
        // issue that this pass did not start: a consult, the join…
        let handed_on = |set: &WorkingSet, bound| {
            run_pass(set, bound, 1, |step| match step {
                Advance { .. } => HandedOn,
                _ => Done,
            })
        };
        let at_stage = set(&[], &[(1, Status::AtStage)]);
        assert!(handed_on(&at_stage, 1).follow_up_due(1));
        // …and nothing once it started the next stage's worker itself.
        let started = handed_on(&due_on(at_stage, &[1], "review"), 2);
        assert!(started.did.contains(&(Dispatch { issue: 1 }, Done)));
        assert!(!started.follow_up_due(2), "a slot is free, and nothing is left to start");
        // A claim that ended for good leaves nothing to start…
        assert!(!only(Judge { issue: 3 }).follow_up_due(3));
        // …and neither does a tick that only looked, or whose step failed.
        assert!(!only(Heartbeat).follow_up_due(3));
        let failed = run_pass(&set(&[], &mine), 3, 3, |step| match step {
            Advance { .. } => Failed,
            _ => Nothing,
        });
        assert!(!failed.follow_up_due(3));
        // With no slot free the stage that follows waits, as any frontier issue does:
        // the slot the advance freed went to #5.
        let full = run_pass(&set(&[5], &[(1, Status::AtStage)]), 1, 1, |_| Done);
        assert_eq!(steps(&full), [Advance { issue: 1 }, Dispatch { issue: 5 }, Heartbeat]);
        assert!(!full.follow_up_due(1));
    }

    #[test]
    fn a_slot_a_patrol_or_a_proposal_freed_is_filled_by_the_pass_that_freed_it() {
        let mut waiting = set(&[5], &[]);
        waiting.patrols = patrols(&[Standing::Running]);
        waiting.proposals = proposals(&[precedent::Standing::Running]);
        let pass = run_pass(&waiting, 2, 2, |_| Done);
        assert!(pass.did.contains(&(Dispatch { issue: 5 }, Done)));
        assert_eq!(pass.frontier_remaining, 0);
        assert!(!pass.follow_up_due(2), "nothing is left for the tick after it");
    }

    #[test]
    fn no_more_than_a_fixed_number_of_ticks_run_back_to_back() {
        // Every tick leaves a follow-up — a fault: the loop still sleeps.
        let (mut skipped, mut run, mut longest) = (0, 1, 1);
        for _ in 0..50 {
            skipped = sleeps_skipped(skipped, true);
            run = if skipped > 0 { run + 1 } else { 1 };
            longest = longest.max(run);
        }
        assert_eq!(longest, BACK_TO_BACK);
        // A tick that left nothing sleeps, and ends the run.
        assert_eq!(sleeps_skipped(0, false), 0);
        assert_eq!(sleeps_skipped(1, false), 0);
        assert_eq!(sleeps_skipped(0, true), 1, "the hop: the next tick starts now");
    }

    #[test]
    fn the_loop_goes_idle_after_three_empty_ticks() {
        assert_eq!(pace(true, 0, 9), BUSY_SECONDS);
        assert_eq!(pace(false, 1, 9), BUSY_SECONDS);
        assert_eq!(pace(false, 0, 2), BUSY_SECONDS);
        assert_eq!(pace(false, 0, 3), IDLE_SECONDS);
        let mut streak = 0;
        for expected in [1, 2, 3, 3] {
            streak = empty_streak(streak, false, 0, 0);
            assert_eq!(streak, expected);
        }
        assert_eq!(empty_streak(3, false, 0, 1), 0, "an undispatched frontier is not quiet");
        assert_eq!(empty_streak(3, true, 0, 0), 0);
    }
}

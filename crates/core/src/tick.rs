//! A tick: one reconciliation pass over the working set, and the pacing of the
//! loop that runs one after another.
//!
//! What a pass does, and in what order, is decided here. The effect layer
//! hands in the one function that carries a step out; it holds no rule about
//! what comes next, and no slot count.

use flux_rs::{attrs::*, macros::qualifier};

use crate::plan::{Status, WorkingSet};

/// One thing a pass does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickStep {
    /// Settle a claim of mine whose issue is closed: clean up, release it last.
    Settle { issue: u64 },
    /// Start a worker on a frontier issue, into a free slot.
    Dispatch { issue: u64 },
    /// Refresh my heartbeat, if it is due.
    Heartbeat,
}

/// How a step ended, as the pass reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It did what it names.
    Done,
    /// A peer holds the issue: nothing was started, and no slot was taken.
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
    /// Whether the pass changed anything: a pass that only looked, or only
    /// refreshed its heartbeat, did no work.
    pub fn did_work(&self) -> bool {
        self.did.iter().any(|(step, outcome)| {
            !matches!(step, TickStep::Heartbeat) && matches!(outcome, Outcome::Done)
        })
    }
}

/// Runs one pass over `set` under a bound of `concurrency` claims: settle the
/// claims of closed issues, dispatch frontier issues into the free slots,
/// lowest number first, then refresh the heartbeat. `carry_out` performs one
/// step and says how it ended; a step that fails leaves its claim held, and
/// the rest of the pass goes on.
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
    let held = settle_closed(set, held, &mut did, &mut carry_out);
    let held = fill_slots(set, concurrency, held, &mut did, &mut carry_out);
    if held > 0 {
        let step = TickStep::Heartbeat;
        did.push((step, carry_out(step)));
    }
    let tried = did.iter().filter(|(step, _)| matches!(step, TickStep::Dispatch { .. })).count();
    Pass { did, held, frontier_remaining: set.frontier.len().saturating_sub(tried) }
}

/// Settles every claim of mine whose issue is closed → the claims still held.
#[spec(fn(set: &WorkingSet, held: usize, did: &mut Vec<(TickStep, Outcome)>, carry_out: &mut F)
    -> usize{after: after <= held})]
fn settle_closed<F>(
    set: &WorkingSet,
    held: usize,
    did: &mut Vec<(TickStep, Outcome)>,
    carry_out: &mut F,
) -> usize
where
    F: FnMut(TickStep) -> Outcome,
{
    let mut slots = Slots::new(0, held);
    for mine in &set.mine {
        if mine.status != Status::Closed {
            continue;
        }
        let step = TickStep::Settle { issue: mine.number };
        let outcome = carry_out(step);
        did.push((step, outcome));
        if outcome == Outcome::Done && slots.held() > 0 {
            slots.free();
        }
    }
    slots.held()
}

/// Dispatches frontier issues while a slot is free → the claims held.
#[spec(fn(set: &WorkingSet, concurrency: usize, held: usize, did: &mut Vec<(TickStep, Outcome)>,
        carry_out: &mut F)
    -> usize{after: held <= after && (after <= concurrency || after == held)})]
fn fill_slots<F>(
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
    for issue in &set.frontier {
        // What stays true on every turn, which Flux does not find alone.
        qualifier!(now: int, limit: int, before: int; now <= limit || now == before);
        qualifier!(before: int, now: int; before <= now);
        if !slots.has_free() {
            break;
        }
        let step = TickStep::Dispatch { issue: issue.number };
        let outcome = carry_out(step);
        did.push((step, outcome));
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

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

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
                .map(|&number| Dispatchable { number, title: String::new() })
                .collect(),
            excluded: vec![],
            mine: mine
                .iter()
                .map(|&(number, status)| Mine { number, title: None, status, pr: None })
                .collect(),
            peer_live: vec![],
            stale: vec![],
            stale_closed: vec![],
            free_slots: 0,
            now: 0,
        }
    }

    use Outcome::*;
    use TickStep::*;

    #[test]
    fn a_pass_settles_then_dispatches_then_beats() {
        let set = set(&[5, 6, 7], &[(1, Status::NoPr), (2, Status::Closed)]);
        let pass = run_pass(&set, 3, 2, |_| Done);
        let steps: Vec<TickStep> = pass.did.iter().map(|(step, _)| *step).collect();
        assert_eq!(
            steps,
            [Settle { issue: 2 }, Dispatch { issue: 5 }, Dispatch { issue: 6 }, Heartbeat]
        );
        assert_eq!((pass.held, pass.frontier_remaining), (3, 1));
        assert!(pass.did_work());
    }

    #[test]
    fn a_claim_is_taken_only_into_a_free_slot() {
        let over = set(&[5, 6], &[(1, Status::NoPr), (2, Status::NoPr), (3, Status::NoPr)]);
        let pass = run_pass(&over, 2, 3, |_| Done);
        assert_eq!(pass.did, [(Heartbeat, Done)]);
        assert_eq!((pass.held, pass.frontier_remaining), (3, 2));
        assert!(!pass.did_work());
    }

    #[test]
    fn a_settling_that_failed_keeps_its_slot() {
        let set = set(&[5], &[(1, Status::Closed)]);
        let pass =
            run_pass(&set, 1, 1, |step| if matches!(step, Settle { .. }) { Failed } else { Done });
        assert_eq!(pass.did, [(Settle { issue: 1 }, Failed), (Heartbeat, Done)]);
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
        let steps: Vec<TickStep> = pass.did.iter().map(|(step, _)| *step).collect();
        assert_eq!(
            steps,
            [Dispatch { issue: 5 }, Dispatch { issue: 6 }, Dispatch { issue: 7 }, Heartbeat]
        );
        assert_eq!((pass.held, pass.frontier_remaining), (2, 1));
    }

    #[test]
    fn a_fleet_holding_nothing_does_not_beat() {
        let pass = run_pass(&set(&[], &[]), 2, 0, |_| Done);
        assert_eq!(pass.did, []);
        assert!(!pass.did_work());
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

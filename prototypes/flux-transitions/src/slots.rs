//! Rule 3 — `concurrency` bounds the claims a fleet holds: a tick takes a
//! claim it does not hold only into a free slot, so afterwards
//! held ≤ max(concurrency, held before).

use flux_rs::{attrs::*, macros::qualifier};

#[spec(fn(concurrency: usize, held: usize) -> usize{free: (held >= concurrency => free == 0) && (held < concurrency => held + free == concurrency)})]
pub fn free_slots(concurrency: usize, held: usize) -> usize {
    if held >= concurrency { 0 } else { concurrency - held }
}

/// Takes candidates one by one while a slot is free; returns the claims held.
#[spec(fn(concurrency: usize, held: usize, candidates: usize)
    -> usize{after: held <= after && (after <= concurrency || after == held)})]
pub fn take(concurrency: usize, held: usize, candidates: usize) -> usize {
    let free = free_slots(concurrency, held);
    let mut now_held = held;
    let mut taken = 0;
    while taken < candidates && taken < free {
        // What stays true on every turn, which Flux does not find alone:
        // the claims held are the ones held before plus the ones taken.
        qualifier!(now: int, before: int, taken: int; now == before + taken);
        qualifier!(taken: int, free: int; taken <= free);
        now_held += 1;
        taken += 1;
    }
    now_held
}

//! Misuses Flux must refuse, and what callers may rely on. Each misuse is the
//! smallest program that breaks one rule the crate states; `#[should_fail]`
//! makes the check fail if Flux ever accepts it. Compiled only under Flux.
#![allow(dead_code)]

use flux_rs::attrs::*;

/// A `Vec` has a length, so nothing may write to whatever index it is handed.
#[should_fail]
fn a_write_to_whatever_index(numbers: &mut Vec<u64>, at: usize) {
    numbers[at] = 0;
}

/// Nor read the first of a `Vec` that may be empty.
#[should_fail]
fn the_first_of_a_vec_that_may_be_empty(numbers: &Vec<u64>) -> u64 {
    numbers[0]
}

/// What callers may rely on: each is checked by Flux, and stops passing when
/// the spec it reads is weakened.
mod relied_on {
    use flux_rs::attrs::*;

    use crate::lease::{stamp_due, stamp_stale};
    use crate::position::Count;
    use crate::team::Stage;

    /// A count is never past its breaker.
    #[spec(fn(&Count[@c]) -> u64{v: v <= c.limit})]
    fn a_count_is_under_its_breaker(count: &Count) -> u64 {
        count.times()
    }

    /// A stage's reject leads back, to a stage of its workflow.
    #[spec(fn(&Stage[@s]) -> Option<usize{v: v < s.at && v < s.n}>)]
    fn a_reject_leads_back_to_a_stage(stage: &Stage) -> Option<usize> {
        stage.reject()
    }

    /// A heartbeat no older than the lease is live.
    #[spec(fn(stamp: u64, now: u64) requires stamp <= now && now - stamp <= crate::lease::LEASE)]
    fn a_heartbeat_within_the_lease_is_live(stamp: u64, now: u64) {
        flux_rs::assert(!stamp_stale(stamp, now));
    }

    /// A stamp further ahead than skew allows is stale, not fresh for being new.
    #[spec(fn(stamp: u64, now: u64) requires stamp > now && stamp - now > crate::lease::SKEW)]
    fn a_stamp_from_the_future_is_stale(stamp: u64, now: u64) {
        flux_rs::assert(stamp_stale(stamp, now));
    }

    /// A heartbeat that is not due is live to every peer, and stays live for
    /// two thirds of a lease: an owner that beats when due is never read as dead.
    #[spec(fn(stamp: u64, now: u64, later: u64)
        requires now <= later && later - now <= crate::lease::LEASE - crate::lease::BEAT_EVERY)]
    fn a_beat_not_due_outlives_the_wait(stamp: u64, now: u64, later: u64) {
        if !stamp_due(stamp, now) && stamp <= now {
            flux_rs::assert(!stamp_stale(stamp, later));
        }
    }
}

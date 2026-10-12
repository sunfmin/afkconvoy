//! Misuses Flux must refuse. Each is the smallest program that breaks one rule
//! the crate states; `#[should_fail]` makes the check fail if Flux ever accepts
//! it. Compiled only under Flux.
#![allow(dead_code)]

use flux_rs::attrs::*;

use crate::config::Config;

/// A config's concurrency is never zero, so nothing may claim to hold one that is.
#[should_fail]
#[spec(fn(&Config) -> usize[0])]
fn a_config_with_no_slots(config: &Config) -> usize {
    config.concurrency()
}

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

mod team {
    use flux_rs::attrs::*;

    use crate::team::Stage;

    /// A stage's reject leads back, so none may be read as leading to the
    /// stage itself or past it.
    #[should_fail]
    #[spec(fn(&Stage[@s]) -> Option<usize{v: v >= s.at}>)]
    fn a_reject_that_leads_forward(stage: &Stage) -> Option<usize> {
        stage.reject()
    }
}

mod lease {
    use flux_rs::attrs::*;

    use crate::lease::{proved, stamp_stale};

    /// A heartbeat is not live merely for having been read.
    #[should_fail]
    #[spec(fn(stamp: u64, now: u64))]
    fn a_heartbeat_assumed_live(stamp: u64, now: u64) {
        let _ = (stamp, now);
        proved(!stamp_stale(stamp, now));
    }

    /// A stamp from the future is not fresh for being new.
    #[should_fail]
    #[spec(fn(stamp: u64, now: u64) requires stamp > now)]
    fn a_future_stamp_read_as_live(stamp: u64, now: u64) {
        proved(!stamp_stale(stamp, now));
    }
}

mod position {
    use flux_rs::attrs::*;

    use crate::position::Count;

    /// Rework is counted under its breaker, so no count may be read as past it.
    #[should_fail]
    #[spec(fn(&Count[@c]) -> u64{v: v > c.limit})]
    fn a_count_past_its_breaker(count: &Count) -> u64 {
        count.times()
    }
}

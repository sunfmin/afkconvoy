//! How long a test waits for an event, in every test crate of this workspace.
//!
//! A test's wall-clock bound is one of two kinds (sunfmin/control#1183):
//!
//! - **A wait for an event** — a file appears, a line is printed, a process
//!   starts, a transition is recorded. Its bound only catches a hang: the event
//!   is waited on, never slept for, so a test is held only as long as the event
//!   takes, and the bound is [`PATIENCE`], whatever the event. A loaded host
//!   takes far longer than a quiet one: on Studio64 another user's simulators
//!   kept the load at 100–280 on 16 cores, a transition that takes 7–13s alone
//!   passed 15s (sunfmin/control#1182), and one tick that leaves three issues
//!   took over 90s at a load of 200 on 14 cores (sunfmin/control#833).
//! - **An assertion that something does not block** — a command answers
//!   before a slow part of its work finishes, a wake is heard before a sleep
//!   would end. No number of seconds proves that on a loaded host; the test
//!   holds the slow side (a stand-in that waits for a file, a lock it keeps)
//!   until the fast side has answered, then releases it, and so proves the
//!   order. Such a test names no bound but [`PATIENCE`] either.
//!
//! What a test checks is a duration on purpose (a sleep was waited out, a
//! timeout was honoured) keeps its own lower bound: a loaded host only makes
//! that longer.

use std::fmt::Display;
use std::time::{Duration, Instant};

/// The one bound on a wait for an event. Generous, since it only catches what
/// never comes; within the limit for one test, 240 seconds
/// (`.config/nextest.toml`), so a hang fails by its own message first.
pub const PATIENCE: Duration = Duration::from_secs(180);

/// How often [`until`] looks again.
const BETWEEN_LOOKS: Duration = Duration::from_millis(20);

/// The instant a wait for an event begun now gives up at.
pub fn deadline() -> Instant {
    Instant::now() + PATIENCE
}

/// Looks for an event until `got` answers it, every 20 milliseconds, and
/// panics naming `what` once [`PATIENCE`] has passed without it.
pub fn until<T>(what: impl Display, got: impl FnMut() -> Option<T>) -> T {
    until_told(what, got, String::new)
}

/// [`until`], whose panic also says what `told` says then: what the test
/// waited on did instead, so that a wait that runs out names its cause
/// (sunfmin/control#1348).
pub fn until_told<T>(
    what: impl Display,
    mut got: impl FnMut() -> Option<T>,
    told: impl FnOnce() -> String,
) -> T {
    let until = deadline();
    loop {
        if let Some(it) = got() {
            return it;
        }
        if Instant::now() >= until {
            panic!("{what}: did not happen within {PATIENCE:?}\n{}", told());
        }
        std::thread::sleep(BETWEEN_LOOKS);
    }
}

/// [`until`] for an event that is only there or not.
pub fn until_true(what: impl Display, mut done: impl FnMut() -> bool) {
    until(what, || done().then_some(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_that_comes_is_answered_as_it_comes() {
        let mut looks = 0;
        let answer = until("the third look", || {
            looks += 1;
            (looks == 3).then_some(looks)
        });
        assert_eq!(answer, 3);
    }

    #[test]
    fn the_bound_is_within_the_limit_for_one_test() {
        assert!(PATIENCE < Duration::from_secs(240));
        assert!(deadline() > Instant::now() + PATIENCE - Duration::from_secs(1));
    }
}

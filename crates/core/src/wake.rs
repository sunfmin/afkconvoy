//! The wake: a signal every command that changes fleet state sends its host's
//! `run` as it ends — a worker's once its outcome is on GitHub — ending the
//! sleep between ticks early. It names the command, for the `run` to log, and
//! carries no state — the tick it opens reads GitHub like any other — so a wake that is
//! lost costs only the wait it would have saved. The `run` that hears one
//! answers it (`Answer`): which tick it starts, or which it follows.

use flux_rs::attrs::*;

/// The name the `run` of a repo listens under, in the repo's git dir: one per
/// repo on a host, shared by every worktree.
pub const SOCKET: &str = "convoy-wake.sock";

/// What a wake the `run` sends itself carries: nothing.
pub const SIGNAL: &[u8] = &[];

/// What a `run` answers a wake as it hears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// No tick was in progress: tick `n` starts now.
    Starts(u64),
    /// Tick `n` is in progress: the next starts as it ends.
    Follows(u64),
}

impl Answer {
    /// The answer `ticking` makes, the tick in progress if one is, `last` the
    /// last tick to have started: none before the first.
    pub fn of(ticking: Option<u64>, last: u64) -> Answer {
        match ticking {
            Some(n) => Answer::Follows(n),
            None => Answer::Starts(last.saturating_add(1)),
        }
    }

    /// What crosses the socket back.
    pub fn said(self) -> String {
        match self {
            Answer::Starts(n) => format!("tick {n}"),
            Answer::Follows(n) => format!("after_tick {n}"),
        }
    }

    /// The answer `said` wrote, if `bytes` are one.
    pub fn read(bytes: &[u8]) -> Option<Answer> {
        let (key, n) = std::str::from_utf8(bytes).ok()?.split_once(' ')?;
        let n = n.parse().ok()?;
        match key {
            "tick" => Some(Answer::Starts(n)),
            "after_tick" => Some(Answer::Follows(n)),
            _ => None,
        }
    }
}

/// Where one turn of the loop stands: whether its tick has started reading.
#[refined_by(reading: bool)]
struct Turn {
    #[field(bool[reading])]
    reading: bool,
}

impl Turn {
    #[spec(fn() -> Turn[false])]
    fn open() -> Turn {
        Turn { reading: false }
    }

    /// Forgets the wakes heard so far: the tick about to start answers them.
    /// Only before it reads anything — a wake that arrives once it is reading
    /// may be about an outcome it has already read past, and must be kept.
    #[spec(fn(&Turn[false], forget: F))]
    fn forget<F: FnOnce()>(&self, forget: F) {
        forget()
    }

    #[spec(fn(self: &mut Turn[false], tick: F) -> T ensures self: Turn[true])]
    fn tick<T, F: FnOnce() -> T>(&mut self, tick: F) -> T {
        self.reading = true;
        tick()
    }
}

/// One turn of `run`'s loop: `forget` the wakes heard before it, then `tick`.
/// A wake that arrives during the tick is still waiting when the loop goes to
/// rest, so the next tick starts at once.
pub fn turn<T>(forget: impl FnOnce(), tick: impl FnOnce() -> T) -> T {
    let mut turn = Turn::open();
    turn.forget(forget);
    turn.tick(tick)
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// The wakes forgotten after the tick: one that arrived while it ran —
    /// about an outcome the tick read past — is lost with them.
    #[should_fail]
    fn wakes_forgotten_after_the_tick<A: FnOnce(), B: FnOnce()>(forget: A, tick: B) {
        let mut turn = Turn::open();
        turn.tick(tick);
        turn.forget(forget);
    }

    /// Two ticks in one turn: the second reads with no chance given to rest.
    #[should_fail]
    fn a_turn_that_ticks_twice<A: FnOnce(), B: FnOnce()>(first: A, second: B) {
        let mut turn = Turn::open();
        turn.tick(first);
        turn.tick(second);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn a_turn_forgets_before_it_ticks_and_hands_back_what_the_tick_made() {
        let order = RefCell::new(vec![]);
        let made = turn(
            || order.borrow_mut().push("forget"),
            || {
                order.borrow_mut().push("tick");
                7
            },
        );
        assert_eq!(made, 7);
        assert_eq!(*order.borrow(), ["forget", "tick"]);
    }

    #[test]
    fn an_answer_names_the_tick_it_starts_or_follows_and_reads_back() {
        assert_eq!(Answer::of(None, 0), Answer::Starts(1));
        assert_eq!(Answer::of(None, 4), Answer::Starts(5));
        assert_eq!(Answer::of(Some(4), 4), Answer::Follows(4));
        for answer in [Answer::Starts(1), Answer::Follows(u64::MAX)] {
            assert_eq!(Answer::read(answer.said().as_bytes()), Some(answer));
        }
        for junk in [&b""[..], b"tick", b"tick x", b"next 3", &[0xff, b' ', b'1']] {
            assert_eq!(Answer::read(junk), None);
        }
    }

    #[test]
    fn a_wake_carries_nothing() {
        assert!(SIGNAL.is_empty());
    }
}

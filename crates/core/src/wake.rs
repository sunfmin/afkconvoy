//! The wake: a contentless signal a worker sends its host's `run` once its
//! outcome is on GitHub, ending the sleep between ticks early. It carries no
//! state — the tick it opens reads GitHub like any other — so a wake that is
//! lost costs only the wait it would have saved.

use flux_rs::attrs::*;

/// The name the `run` of a repo listens under, in the repo's git dir: one per
/// repo on a host, shared by every worktree.
pub const SOCKET: &str = "convoy-wake.sock";

/// What a wake carries: nothing.
pub const SIGNAL: &[u8] = &[];

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

    #[spec(fn(self: &strg Turn[false], tick: F) -> T ensures self: Turn[true])]
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
    fn a_wake_carries_nothing() {
        assert!(SIGNAL.is_empty());
    }
}

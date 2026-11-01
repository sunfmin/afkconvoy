//! How every fleet instance of the host runner stands, held by the host
//! runner itself (docs/adr/0023): whether each is starting, in a tick, at
//! work or quiet, as each says it as its ticks begin and end. A move onto a
//! newer release reads them, and holds new ticks back while it prepares
//! (docs/adr/0032). No fleet instance asks another.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use afkconvoy_core::host_runner::Stands;

use crate::stop::Stop;

/// How often a tick held back by a move looks whether its `run` was told to
/// stop.
const LOOKS_EVERY: Duration = Duration::from_millis(250);

/// How every fleet instance of one host runner stands: each from its
/// `Standing::open` until that is dropped. A clone is the same.
#[derive(Clone, Default)]
pub struct Standings {
    held: Arc<(Mutex<Held>, Condvar)>,
}

#[derive(Default)]
struct Held {
    /// Each fleet instance, by the key it opened with.
    each: BTreeMap<u64, One>,
    /// The key the next instance to open takes.
    next: u64,
    /// No new tick begins while the host runner prepares a release move.
    paused: bool,
}

/// One fleet instance, as it last said it stands.
struct One {
    instance: String,
    /// How many ticks it began, and how many it ended.
    began: u64,
    ended: u64,
    /// Whether its last tick held a claim, had something on the landing
    /// train, or failed whole: so until its first has ended.
    at_work: bool,
}

impl Standings {
    /// Holds back every tick not begun yet, until `resume`.
    pub fn pause(&self) {
        self.held().paused = true;
    }

    pub fn resume(&self) {
        self.held().paused = false;
        self.held.1.notify_all();
    }

    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Calls `with` with how every instance stands now, as a move of their
    /// host runner reads it — each by its id: `Ticking` from the start of a
    /// tick until it ends, else `AtWork` or `Quiet` as its last tick left it —
    /// and no instance begins a tick until `with` returns.
    pub fn held_still<T>(&self, with: impl FnOnce(&BTreeMap<String, Stands>) -> T) -> T {
        let held = self.held();
        let stands = |one: &One| match one {
            One { began: 0, .. } => Stands::Starting,
            One { began, ended, .. } if began > ended => Stands::Ticking,
            One { at_work: true, .. } => Stands::AtWork,
            One { at_work: false, .. } => Stands::Quiet,
        };
        let each = held.each.values().map(|one| (one.instance.clone(), stands(one)));
        with(&each.collect())
    }
}

/// A fleet instance's place among the standings: it stands among the others
/// from `open` until it is dropped, which takes it from among them.
pub struct Standing {
    standings: Standings,
    key: u64,
}

impl Standing {
    /// Stands among `standings` as the fleet instance `instance`, starting:
    /// at work until its first tick ends. Called before the first tick.
    pub fn open(instance: &str, standings: &Standings) -> Standing {
        let key = {
            let mut held = standings.held();
            let key = held.next;
            held.next += 1;
            let one = One { instance: instance.to_string(), began: 0, ended: 0, at_work: true };
            held.each.insert(key, one);
            key
        };
        Standing { standings: standings.clone(), key }
    }

    /// Says that this instance begins a tick, without a release move's pause.
    #[cfg(test)]
    pub fn begins(&self) {
        if let Some(one) = self.standings.held().each.get_mut(&self.key) {
            one.began += 1;
        }
    }

    /// Begins a tick only while the host runner permits it → whether it
    /// began one. A release move pauses here, then stops the instance without
    /// entering another tick.
    pub fn begins_unless_stopped(&self, stop: &Stop) -> bool {
        let mut held = self.standings.held();
        // A checkout is put on the list by its first tick. Let that finish
        // even when a move is pending, so the replacement finds it there.
        while held.paused
            && held.each.get(&self.key).is_some_and(|one| one.began > 0)
            && !stop.told()
        {
            held = self
                .standings
                .held
                .1
                .wait_timeout(held, LOOKS_EVERY)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        if stop.told() {
            return false;
        }
        if let Some(one) = held.each.get_mut(&self.key) {
            one.began += 1;
        }
        true
    }

    /// Says that this instance ended a tick, `at_work` or quiet after it: a
    /// tick that failed whole is at work.
    pub fn ticked(&self, at_work: bool) {
        if let Some(one) = self.standings.held().each.get_mut(&self.key) {
            one.at_work = at_work;
            one.ended += 1;
        }
        self.standings.held.1.notify_all();
    }
}

impl Drop for Standing {
    fn drop(&mut self) {
        self.standings.held().each.remove(&self.key);
        self.standings.held.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_move_lets_a_tick_end_and_holds_the_next_until_the_instance_stops() {
        let standings = Standings::default();
        let alpha = Standing::open("alpha", &standings);
        standings.held_still(|each| assert_eq!(each["alpha"], Stands::Starting));
        let stop = Stop::never();
        assert!(alpha.begins_unless_stopped(&stop));
        standings.pause();
        standings.held_still(|each| assert_eq!(each["alpha"], Stands::Ticking));
        alpha.ticked(true);
        standings.held_still(|each| assert_eq!(each["alpha"], Stands::AtWork));
        let (sent, received) = std::sync::mpsc::channel();
        let stopping = stop.clone();
        let waiting =
            std::thread::spawn(move || sent.send(alpha.begins_unless_stopped(&stopping)).unwrap());
        assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
        stop.tell();
        assert!(!received.recv_timeout(Duration::from_secs(2)).unwrap(), "no new tick began");
        waiting.join().unwrap();
    }

    #[test]
    fn each_instance_stands_among_its_runner_s_others_and_drops_out_as_it_ends() {
        let standings = Standings::default();
        let alpha = Standing::open("alpha", &standings);
        let beta = Standing::open("beta", &standings);
        let _gamma = Standing::open("gamma", &Standings::default());
        let every = || standings.held_still(|each| each.clone());
        assert_eq!(every().keys().collect::<Vec<_>>(), ["alpha", "beta"], "not another runner's");
        beta.begins();
        beta.ticked(false);
        assert_eq!(every()["beta"], Stands::Quiet);
        drop(beta);
        assert_eq!(every().keys().collect::<Vec<_>>(), ["alpha"]);
        drop(alpha);
        assert!(every().is_empty());
    }
}

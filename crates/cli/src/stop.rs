//! Being told to stop: the SIGTERM or SIGINT a `run` hears, and finishes the
//! step it is in before it obeys; or one fleet instance of the process told
//! alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;

/// Whether a fleet instance was told to stop: by its own flag, or with every
/// other fleet instance of this process by the process's, which SIGTERM and
/// SIGINT set.
#[derive(Clone)]
pub struct Stop {
    own: Arc<AtomicBool>,
    process: Arc<AtomicBool>,
}

impl Stop {
    /// Hears SIGTERM and SIGINT from now on: the first is kept for whoever
    /// asks `told`, and a second ends the process at once, as either did
    /// before anything listened. Where the signals cannot be heard they end
    /// the process as they always did, which costs only the goodbye. This is
    /// the process's: its `tell` tells every instance made from it.
    pub fn listen() -> Stop {
        let told = Arc::new(AtomicBool::new(false));
        for signal in [SIGTERM, SIGINT] {
            // In this order: the second signal finds the flag the first set.
            let heard = flag::register_conditional_shutdown(signal, 1, Arc::clone(&told))
                .and_then(|_| flag::register(signal, Arc::clone(&told)));
            if let Err(error) = heard {
                eprintln!("warning: this run cannot hear signal {signal}, and ends on it: {error}");
            }
        }
        Stop { own: Arc::clone(&told), process: told }
    }

    /// Nobody tells a command that makes one tick to stop: it ends by itself.
    pub fn never() -> Stop {
        let told = Arc::new(AtomicBool::new(false));
        Stop { own: Arc::clone(&told), process: told }
    }

    /// The stop of one fleet instance of this process: told by its own
    /// `tell`, or by the process's.
    pub fn instance(&self) -> Stop {
        Stop { own: Arc::new(AtomicBool::new(false)), process: Arc::clone(&self.process) }
    }

    /// Tells it: one instance's tells that instance alone; the process's
    /// tells every one.
    pub fn tell(&self) {
        self.own.store(true, Ordering::Relaxed);
    }

    pub fn told(&self) -> bool {
        self.own.load(Ordering::Relaxed) || self.process.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_is_told_by_its_own_tell_or_the_processs_and_tells_no_other() {
        let process = Stop::never();
        let (alpha, beta) = (process.instance(), process.instance());
        assert!(!alpha.told() && !beta.told() && !process.told());
        alpha.tell();
        assert!(alpha.told(), "by its own");
        assert!(!beta.told() && !process.told(), "its own tells no other");
        assert!(alpha.clone().told(), "a clone is the same stop");
        process.tell();
        assert!(beta.told() && process.told(), "the process's tells every one");
        assert!(process.instance().told(), "one made after it too");
    }
}

//! Being told to stop: the SIGTERM or SIGINT a `run` hears, and finishes the
//! step it is in before it obeys.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;

/// Whether this process was told to stop.
#[derive(Clone)]
pub struct Stop {
    told: Arc<AtomicBool>,
}

impl Stop {
    /// Hears SIGTERM and SIGINT from now on: the first is kept for whoever
    /// asks `told`, and a second ends the process at once, as either did
    /// before anything listened. Where the signals cannot be heard they end
    /// the process as they always did, which costs only the goodbye.
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
        Stop { told }
    }

    /// Nobody tells a command that makes one tick to stop: it ends by itself.
    pub fn never() -> Stop {
        Stop { told: Arc::new(AtomicBool::new(false)) }
    }

    pub fn told(&self) -> bool {
        self.told.load(Ordering::Relaxed)
    }
}

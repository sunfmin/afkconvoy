//! The real CLI entry point with advancing termination time. Only tests of a
//! stand-in terminal that never closes use this executable; OS process tests
//! use afkconvoy itself. No environment variable changes production timing.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use afkconvoy::orca::{TerminationClock, with_termination_clock};

struct Clock(Cell<Duration>);

impl TerminationClock for Clock {
    fn now(&self) -> Duration {
        self.0.get()
    }

    fn sleep(&self, interval: Duration) {
        // These four scenarios return the same live terminal forever. Move
        // to the deadline after the first unsuccessful observation: another
        // complete OS/runtime refresh must still refuse the replacement.
        // Replaying fifty identical subprocess polls costs more than sleep
        // on a loaded host and proves no additional state transition here.
        self.0.set(self.0.get() + interval.max(Duration::from_secs(5)));
    }
}

fn main() -> std::process::ExitCode {
    with_termination_clock(Rc::new(Clock(Cell::new(Duration::ZERO))), || {
        afkconvoy::main(std::env::args_os())
    })
}

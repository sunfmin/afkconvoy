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
        self.0.set(self.0.get() + interval);
    }
}

fn main() -> std::process::ExitCode {
    with_termination_clock(Rc::new(Clock(Cell::new(Duration::ZERO))), || {
        afkconvoy::main(std::env::args_os())
    })
}

//! How fast the host creates processes (sunfmin/control#1319). Process
//! creation is paid in the kernel and in the daemons each exec wakes, not in
//! any process's CPU: a host can be 39% in the kernel, with the processes
//! `top` sees adding up to a sixth of its cores.
//!
//! The kernel hands out process ids one after the other, wrapping at a
//! maximum. A process started now has the next one: its id is the counter,
//! read with no root and no special tool, and the difference between two
//! readings is the processes created between them.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The counter's maximum where the host does not say: macOS's.
const MAX: u64 = 99_999;

/// How long a reading of the host's rate on its own watches the counter.
pub const SPAN: Duration = Duration::from_millis(500);

/// The process id counter at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    /// The id the last process created was given.
    pub pid: u64,
    /// The id the counter wraps at.
    pub max: u64,
}

impl Counter {
    /// The processes created from `earlier` to this reading, the counter
    /// having wrapped at most once between them.
    pub fn since(self, earlier: Counter) -> u64 {
        if self.pid >= earlier.pid {
            self.pid - earlier.pid
        } else {
            self.pid + self.max.saturating_sub(earlier.pid)
        }
    }

    /// Whether `pid` was given out after `earlier` was read and no later than
    /// this reading, the counter wrapping at most once between them.
    pub fn made_since(self, earlier: Counter, pid: u64) -> bool {
        if self.pid >= earlier.pid {
            pid > earlier.pid && pid <= self.pid
        } else {
            pid > earlier.pid || pid <= self.pid
        }
    }
}

/// The counter now: the id of a process started just now, which does
/// nothing. `None`: none could be started.
pub fn counter() -> Option<Counter> {
    if let Some(stand_in) = std::env::var_os(FAKE) {
        let pid = std::fs::read_to_string(stand_in).ok()?.trim().parse().ok()?;
        return Some(Counter { pid, max: MAX });
    }
    let max = std::fs::read_to_string("/proc/sys/kernel/pid_max")
        .ok()
        .and_then(|said| said.trim().parse().ok())
        .unwrap_or(MAX);
    let mut child = ["/usr/bin/true", "/bin/true"].iter().find_map(|program| {
        Command::new(program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    })?;
    let pid = u64::from(child.id());
    let _ = child.wait();
    Some(Counter { pid, max })
}

/// A test world's stand-in for the counter: the file it names says the id.
const FAKE: &str = "AFKCONVOY_FAKE_PID_COUNTER";

/// Processes a second, from `created` processes over `elapsed`.
pub fn per_second(created: u64, elapsed: Duration) -> u64 {
    (created as f64 / elapsed.as_secs_f64().max(0.001)).round() as u64
}

/// The host's process creation rate, in processes a second, over `SPAN`
/// watched now; `None`: the counter cannot be read.
pub fn rate() -> Option<u64> {
    let (first, at) = (counter()?, Instant::now());
    std::thread::sleep(SPAN);
    let second = counter()?;
    // The two readings' own processes are two of what was created.
    Some(per_second(second.since(first).saturating_sub(1), at.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_processes_created_are_the_counters_difference_across_its_wrap() {
        let at = |pid| Counter { pid, max: 100 };
        assert_eq!(at(150 - 100 + 30).since(at(20)), 60);
        assert_eq!(at(10).since(at(95)), 15, "wrapped at 100");
        assert!(at(10).made_since(at(95), 99));
        assert!(at(10).made_since(at(95), 3));
        assert!(!at(10).made_since(at(95), 50));
        assert!(at(60).made_since(at(20), 21) && !at(60).made_since(at(20), 20));
        assert_eq!(per_second(1500, Duration::from_secs(1)), 1500);
    }

    #[test]
    fn this_host_s_creation_rate_is_read() {
        let first = counter().expect("a process can be started here");
        let second = counter().unwrap();
        assert!(second.since(first) >= 1, "{first:?} {second:?}");
        assert!(rate().is_some());
    }
}

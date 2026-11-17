//! What the host runner's own process spends of a core, averaged over the
//! last hour: its threads only — the watch's checks, the asking session's,
//! the ticks it holds — not the processes it starts. Its job is a look every
//! few minutes; between looks it should cost almost nothing
//! (sunfmin/control#938). `doctor` says it when it is past the budget.
//!
//! The runner's start-up is not counted: it brings up its fleets and takes
//! over claims in a burst that says nothing of its steady state
//! (sunfmin/control#1320). The average starts at the first reading taken
//! `SETTLED` after the first one, and covers at most `WINDOW` from there.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The most of one core the host runner's own process may average, in
/// thousandths of a core: 5%.
pub const BUDGET_PERMILLE: u64 = 50;

/// The span the average is over.
pub const WINDOW: Duration = Duration::from_secs(3600);

/// The start-up span left out of the average, and the least span an average
/// is said over: a runner just started has not shown its steady state yet.
pub const SETTLED: Duration = Duration::from_secs(600);

/// How often a reading is kept.
pub const NOTED_EVERY: Duration = Duration::from_secs(60);

/// The host runner's average use of a core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Average {
    /// Thousandths of one core.
    pub permille: u64,
    /// The seconds it is averaged over, at most `WINDOW`'s.
    pub over_seconds: u64,
    /// The host runner's process.
    #[serde(default)]
    pub pid: u32,
}

impl Average {
    /// Whether it is past the budget, over a span long enough to say so.
    pub fn over_budget(&self) -> bool {
        self.permille > BUDGET_PERMILLE && self.over_seconds >= SETTLED.as_secs()
    }
}

/// Readings of the process's CPU time, each at a wall second, the oldest
/// first, none older than `WINDOW` before the newest but the one that
/// starts it. None is kept from the first `SETTLED` after the first one
/// offered: that is the start-up.
#[derive(Debug, Default)]
pub struct Window {
    /// The wall second of the first reading offered, the process's start.
    began: Option<u64>,
    readings: VecDeque<(u64, Duration)>,
}

impl Window {
    /// Keeps the reading `used` at `at`, once `NOTED_EVERY` passed since the
    /// last one and the start-up is over.
    pub fn note(&mut self, at: u64, used: Duration) {
        let began = *self.began.get_or_insert(at);
        if at < began + SETTLED.as_secs() {
            return;
        }
        if self.readings.back().is_some_and(|(last, _)| at < last + NOTED_EVERY.as_secs()) {
            return;
        }
        self.readings.push_back((at, used));
        // Keep the newest reading at least `WINDOW` old, so the span is whole.
        while self.readings.get(1).is_some_and(|(second, _)| at >= second + WINDOW.as_secs()) {
            self.readings.pop_front();
        }
    }

    /// The average from the oldest reading kept to `used` at `at`.
    pub fn average(&self, at: u64, used: Duration) -> Option<Average> {
        let (from, before) = self.readings.front()?;
        let over_seconds = at.checked_sub(*from).filter(|over| *over > 0)?;
        let spent = used.saturating_sub(*before);
        let permille = (spent.as_millis() as u64) / over_seconds;
        Some(Average { permille, over_seconds, pid: std::process::id() })
    }
}

/// The window the host runner keeps; a clone is the same.
#[derive(Debug, Clone, Default)]
pub struct Shared(Arc<Mutex<Window>>);

impl Shared {
    /// Keeps this process's reading now.
    pub fn note(&self) {
        if let Some(used) = used() {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).note(crate::clock(), used);
        }
    }

    /// This process's average now; `None` before a minute's readings.
    pub fn average(&self) -> Option<Average> {
        let used = used()?;
        self.0.lock().unwrap_or_else(|e| e.into_inner()).average(crate::clock(), used)
    }
}

/// The CPU time this process has used, user and system, all its threads.
pub fn used() -> Option<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` is a `rusage`, which the kernel fills for this process.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: zeroed, then written by the kernel: every field is a number.
    let usage = unsafe { usage.assume_init() };
    let time = |t: libc::timeval| {
        Duration::from_secs(t.tv_sec.max(0) as u64) + Duration::from_micros(t.tv_usec.max(0) as u64)
    };
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_794_634_486;

    #[test]
    fn the_average_is_the_cpu_time_spent_over_the_wall_time_kept() {
        let mut window = Window::default();
        assert_eq!(window.average(AT, Duration::ZERO), None, "nothing read yet");
        window.note(AT, Duration::ZERO);
        assert_eq!(window.average(AT + 60, Duration::ZERO), None, "the start-up is not kept");
        let start = AT + SETTLED.as_secs();
        window.note(start, Duration::from_secs(10));
        // 30 s of CPU over 600 s: 5% of a core, the budget, not past it.
        let average = window.average(start + 600, Duration::from_secs(40)).unwrap();
        assert_eq!((average.permille, average.over_seconds), (50, 600));
        assert!(!average.over_budget());
        let past = window.average(start + 600, Duration::from_secs(41)).unwrap();
        assert!(past.over_budget(), "{past:?}");
        let young = window.average(start + 60, Duration::from_secs(40)).unwrap();
        assert!(!young.over_budget(), "a minute says no steady state: {young:?}");
    }

    /// Notes a runner minute by minute until `minutes`, `used_at` giving its
    /// CPU time at a second since start; the average at each of `looks`.
    fn looks(used_at: impl Fn(u64) -> u64, looks: &[u64]) -> Vec<Average> {
        let mut window = Window::default();
        let mut found = Vec::new();
        for minute in 0..=*looks.iter().max().unwrap() {
            let at = minute * 60;
            window.note(AT + at, Duration::from_secs(used_at(at)));
            if looks.contains(&minute)
                && let Some(a) = window.average(AT + at, Duration::from_secs(used_at(at)))
            {
                found.push(a);
            }
        }
        found
    }

    #[test]
    fn the_start_up_burst_is_not_counted() {
        // 70 s of CPU in the first 2 minutes, 1% after.
        let used = |s: u64| if s <= 120 { s * 70 / 120 } else { 70 + (s - 120) / 100 };
        let found = looks(used, &[13, 30, 60]);
        assert_eq!(found.len(), 3);
        for a in &found {
            assert!(!a.over_budget(), "{a:?}");
        }
        assert!(found.iter().all(|a| a.permille <= 20), "{found:?}");
    }

    #[test]
    fn a_steady_overrun_is_said_once_settled_past_the_start_up() {
        let used = |s: u64| s * 8 / 100;
        let found = looks(used, &[5, 15, 30]);
        // At 5 minutes nothing is kept yet; at 15 only 5 minutes are averaged.
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(!found[0].over_budget(), "{:?}", found[0]);
        let late = looks(used, &[21, 30]);
        assert!(late.iter().all(|a| a.over_budget()), "{late:?}");
    }

    #[test]
    fn readings_are_kept_a_minute_apart_and_for_an_hour() {
        let mut window = Window::default();
        for minute in 0..180 {
            // Busy for the first hour, idle since.
            let used = Duration::from_secs(minute.min(60) * 30);
            window.note(AT + minute * 60, used);
            window.note(AT + minute * 60 + 1, used);
        }
        assert!(window.readings.len() <= 62, "{}", window.readings.len());
        let now = AT + 180 * 60;
        let average = window.average(now, Duration::from_secs(60 * 30)).unwrap();
        assert_eq!(average.permille, 0, "the busy hour is gone: {average:?}");
        assert!(average.over_seconds >= WINDOW.as_secs(), "{average:?}");
    }

    #[test]
    fn this_process_reads_its_own_cpu_time() {
        let before = used().unwrap();
        let mut spin = 0u64;
        let began = std::time::Instant::now();
        while began.elapsed() < Duration::from_millis(50) {
            spin = spin.wrapping_add(1);
        }
        assert!(spin > 0);
        assert!(used().unwrap() > before);
    }
}

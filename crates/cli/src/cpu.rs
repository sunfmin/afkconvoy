//! How busy the host's cores are, as admission reads it (docs/adr/0084,
//! sunfmin/control#1168): the share of all its cores' time not idle between
//! two readings of the kernel's CPU ticks — `host_statistics`'
//! `HOST_CPU_LOAD_INFO` on macOS, the first line of `/proc/stat` elsewhere.
//! macOS's load average counts more than the threads that want a core, and
//! on a host whose cores are mostly idle another account's simulators keep
//! it near 100; the ticks count what the cores did.
//!
//! A process keeps its last reading: the next is said over the time since,
//! up to `WINDOW`, so the host runner and a fleet reading every tick wait for
//! nothing. A process with no reading that recent reads twice, `SHORTEST`
//! apart, or longer until a tick has passed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The longest span a reading is said over, the load average's.
pub const WINDOW: Duration = Duration::from_secs(60);

/// The shortest: a reading sooner than this after the last says the last's.
pub const SHORTEST: Duration = Duration::from_millis(250);

/// The longest a process with no recent reading waits for the ticks to move.
pub const LONGEST: Duration = Duration::from_secs(3);

/// A test world's stand-in: the file it names says the cores' busy share in
/// percent, or holds nothing, and they are not read.
const FAKE: &str = "AFKCONVOY_FAKE_CPU_BUSY";

/// The kernel's CPU ticks since boot, summed over the cores: those not
/// idle, and all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticks {
    pub busy: u64,
    pub total: u64,
}

impl Ticks {
    /// The share of the ticks from `earlier` to these that were busy, in
    /// percent; `None`: none passed.
    pub fn busy_since(self, earlier: Ticks) -> Option<u64> {
        let total = self.total.checked_sub(earlier.total).filter(|total| *total > 0)?;
        let busy = self.busy.saturating_sub(earlier.busy).min(total);
        Some((busy as f64 * 100.0 / total as f64).round() as u64)
    }
}

/// The last reading this process took, and what it said.
#[derive(Clone, Copy)]
struct Last {
    at: Instant,
    ticks: Ticks,
    said: Option<u64>,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

/// The share of the host's cores' time busy, in percent, over the time since
/// this process last read it, up to `WINDOW`; `None`: it cannot be read.
/// macOS moves its ticks on in steps, at times a second apart: a reading
/// that finds none passed since a recent one says that one's, and one with
/// none recent waits for them, up to `LONGEST`.
pub fn busy() -> Option<u64> {
    if let Some(fake) = std::env::var_os(FAKE) {
        return std::fs::read_to_string(fake).ok()?.trim().parse().ok();
    }
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let recent = (*last).filter(|last| last.at.elapsed() <= WINDOW);
    if let Some(recent) = recent
        && recent.at.elapsed() < SHORTEST
    {
        return recent.said;
    }
    let earlier = match recent {
        Some(recent) => recent.ticks,
        None => ticks()?,
    };
    let waited = Instant::now();
    loop {
        if recent.is_none() {
            std::thread::sleep(SHORTEST);
        }
        let now = ticks()?;
        if let Some(said) = now.busy_since(earlier) {
            *last = Some(Last { at: Instant::now(), ticks: now, said: Some(said) });
            return Some(said);
        }
        if let Some(recent) = recent {
            return recent.said;
        }
        if waited.elapsed() >= LONGEST {
            return None;
        }
    }
}

/// The kernel's ticks over the last minute, as the host runner keeps them
/// for its exclusive queue (docs/adr/0087): a sample at most each second,
/// and the busy share since the oldest of the last `WINDOW`. A host runner
/// that has read for less says it over what it has.
#[derive(Debug, Default)]
pub struct Minute {
    samples: std::collections::VecDeque<(u64, Ticks)>,
}

impl Minute {
    /// How busy the cores were, in percent, from the second the reading's
    /// span began to `now`, in epoch seconds → the share, and that second;
    /// `None`: the ticks cannot be read, or none passed yet. A test world's
    /// stand-in (`AFKCONVOY_FAKE_CPU_BUSY`) says it over the whole window.
    pub fn read(&mut self, now: u64) -> Option<(u64, u64)> {
        let window = WINDOW.as_secs();
        if let Some(fake) = std::env::var_os(FAKE) {
            let busy = std::fs::read_to_string(fake).ok()?.trim().parse().ok()?;
            return Some((busy, now.saturating_sub(window)));
        }
        if self.samples.back().is_none_or(|(at, _)| *at < now)
            && let Some(ticks) = ticks()
        {
            self.samples.push_back((now, ticks));
        }
        self.said(now)
    }

    /// What the samples say at `now`: those older than the newest one of a
    /// window ago are dropped, and the share is said since the oldest left.
    fn said(&mut self, now: u64) -> Option<(u64, u64)> {
        let start = now.saturating_sub(WINDOW.as_secs());
        while self.samples.get(1).is_some_and(|(at, _)| *at <= start) {
            self.samples.pop_front();
        }
        let (since, first) = *self.samples.front()?;
        let (_, last) = *self.samples.back()?;
        Some((last.busy_since(first)?, since))
    }
}

/// The kernel's CPU ticks now.
#[cfg(target_os = "macos")]
pub fn ticks() -> Option<Ticks> {
    let mut info = libc::host_cpu_load_info { cpu_ticks: [0; libc::CPU_STATE_MAX as usize] };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: `info` is the `host_cpu_load_info` `HOST_CPU_LOAD_INFO` fills,
    // and `count` says its size in `integer_t`s.
    let answered = unsafe {
        libc::host_statistics(host(), libc::HOST_CPU_LOAD_INFO, (&raw mut info).cast(), &mut count)
    };
    if answered != libc::KERN_SUCCESS {
        return None;
    }
    let tick = |state: libc::c_int| u64::from(info.cpu_ticks[state as usize]);
    let idle = tick(libc::CPU_STATE_IDLE);
    let busy =
        tick(libc::CPU_STATE_USER) + tick(libc::CPU_STATE_SYSTEM) + tick(libc::CPU_STATE_NICE);
    Some(Ticks { busy, total: busy + idle })
}

/// The host's port, asked for once: each `mach_host_self` takes a right more.
#[cfg(target_os = "macos")]
fn host() -> libc::mach_port_t {
    // libc's own is deprecated for the `mach2` crate: this one call is not
    // worth a crate.
    unsafe extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
    }
    static HOST: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    // SAFETY: `mach_host_self` takes no argument and only returns a port.
    *HOST.get_or_init(|| unsafe { mach_host_self() })
}

/// The kernel's CPU ticks now.
#[cfg(not(target_os = "macos"))]
pub fn ticks() -> Option<Ticks> {
    proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

/// The ticks the first line of `/proc/stat` says: `cpu user nice system idle
/// iowait irq softirq steal …`, idle and waiting on I/O the idle ones.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn proc_stat(said: &str) -> Option<Ticks> {
    let line = said.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> =
        line.split_whitespace().skip(1).map(|field| field.parse().ok()).collect::<Option<_>>()?;
    let idle = fields.get(3)? + fields.get(4).copied().unwrap_or(0);
    let total: u64 = fields.iter().take(8).sum();
    Some(Ticks { busy: total.saturating_sub(idle), total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_busy_share_is_of_the_ticks_between_two_readings() {
        let earlier = Ticks { busy: 1000, total: 5000 };
        assert_eq!(Ticks { busy: 1400, total: 6000 }.busy_since(earlier), Some(40));
        assert_eq!(Ticks { busy: 1950, total: 6000 }.busy_since(earlier), Some(95));
        assert_eq!(earlier.busy_since(earlier), None, "no tick passed");
    }

    #[test]
    fn a_minute_says_the_share_since_its_oldest_sample_of_the_last_minute() {
        let mut minute = Minute::default();
        let sample = |at, busy, total| (at, Ticks { busy, total });
        minute.samples.extend([sample(90, 0, 0), sample(100, 0, 0), sample(130, 10, 100)]);
        minute.samples.push_back(sample(161, 60, 200));
        assert_eq!(minute.said(161), Some((30, 100)), "since the newest of a minute ago");
        assert_eq!(minute.said(200), Some((50, 130)), "only the last minute's");
        assert_eq!(Minute::default().said(5), None, "nothing read");
        let read = Minute::default().read(crate::clock());
        if std::env::var_os(FAKE).is_none() {
            assert!(read.is_none_or(|(busy, _)| busy <= 100), "{read:?}");
        }
    }

    #[test]
    fn proc_stat_is_read_as_linux_says_it() {
        let said = "cpu  100 20 30 800 50 0 0 0 0 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        assert_eq!(proc_stat(said), Some(Ticks { busy: 150, total: 1000 }));
        assert_eq!(proc_stat("intr 1 2\n"), None);
    }

    #[test]
    fn this_host_s_cores_are_read() {
        let earlier = ticks().expect("the kernel says its CPU ticks");
        std::thread::sleep(Duration::from_millis(50));
        let now = ticks().unwrap();
        assert!(now.total >= earlier.total && now.busy <= now.total, "{earlier:?} {now:?}");
        let busy = busy().expect("a busy share is read");
        assert!(busy <= 100, "{busy}");
    }
}

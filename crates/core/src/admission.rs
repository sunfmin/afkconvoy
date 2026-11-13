//! Admission: a tick starts no new worker while its host is busy — past a
//! share of its load, below its share of available memory, or low on free disk
//! (docs/adr/0019) — while a usage window of the account it would start on
//! would run out before it resets, or while the host's exclusive queue would
//! take longer than its bound to drain (docs/adr/0072). It alone decides each
//! start: no number of workers bounds a fleet. Whether admission holds is decided here, from what the
//! effect layer read of the host and the shares it is held to; reading the
//! host is the effect layer's.
//!
//! Admission holds back only what starts a worker into a free slot: workers
//! already running, the landing train and the worker a declared verdict
//! starts are never held back.

use serde::Serialize;

/// The shares of the host past which admission holds, each in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shares {
    /// The 1-minute load average, as a share of the host's cores: admission
    /// holds while the load is past it.
    pub load: u64,
    /// The memory free, as a share of the host's memory: admission holds
    /// while less is free.
    pub memory: u64,
    /// The disk free on the checkout's volume, as a share of its size:
    /// admission holds while less is free.
    pub disk: u64,
}

/// The shares a host that says none is held to: a load of twice its cores, a
/// tenth of its memory free, a hundredth of its disk free.
pub const SHARES: Shares = Shares { load: 200, memory: 10, disk: 1 };

/// What the effect layer read of the host. `None`: it could not be read, and
/// holds nothing back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Reading {
    /// Its logical cores.
    pub cores: u64,
    /// Its 1-minute load average, in hundredths.
    pub load: Option<u64>,
    /// The share of its memory free, in percent: what the kernel counts as
    /// available, so that it falls as memory pressure rises.
    pub memory_free: Option<u64>,
    /// The disk of the checkout's volume.
    pub disk: Option<Disk>,
}

/// A volume's free space and size, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Disk {
    pub free: u64,
    pub size: u64,
}

/// The thresholds `shares` come to on a host read as `reading`: what holds
/// admission, in the units it is read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Thresholds {
    /// The load past which admission holds, in hundredths.
    pub load: u64,
    /// The share of memory free under which it holds, in percent.
    pub memory_free: u64,
    /// The bytes free under which it holds; `None`: the disk was not read.
    pub disk_free: Option<u64>,
}

/// The thresholds `shares` come to on the host `reading` read.
pub fn thresholds(shares: Shares, reading: &Reading) -> Thresholds {
    Thresholds {
        // Cores times a share in percent is the load in hundredths.
        load: reading.cores.saturating_mul(shares.load),
        memory_free: shares.memory,
        disk_free: reading.disk.map(|disk| share_of(disk.size, shares.disk)),
    }
}

/// `percent` of `bytes`: divided first, for the product may not fit.
fn share_of(bytes: u64, percent: u64) -> u64 {
    (bytes / 100).saturating_mul(percent)
}

/// One threshold the host is past, with what was read of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Hold {
    /// The load, in hundredths, is past `limit`.
    Load { load: u64, limit: u64 },
    /// The share of memory free, in percent, is under `floor`.
    Memory { free: u64, floor: u64 },
    /// The bytes free on the checkout's volume are under `floor`.
    Disk { free: u64, floor: u64 },
    /// The usage window `window` of the account of the harness `account`,
    /// `used` percent used, would at its present rate, with one more worker
    /// beside the `running` ones, be `projected` percent used as it resets in
    /// `resets_in` seconds: past all of it.
    Window {
        account: String,
        window: String,
        used: u64,
        projected: u64,
        running: usize,
        resets_in: u64,
    },
    /// The commands waiting in the host's exclusive queue would take `drain`
    /// seconds to run, by their recent run times: past `bound`.
    Queue { drain: u64, bound: u64 },
    /// afkconvoy's own processes on the host take `used` hundredths of a
    /// core, at or past `limit`, the share of the host's cores it may use
    /// (`Cap`): a start would take it over.
    OwnCores { used: u64, limit: u64 },
    /// afkconvoy's own processes on the host hold `resident` bytes, at or
    /// past `limit`, the share of the host's memory it may use (`Cap`).
    OwnMemory { resident: u64, limit: u64 },
}

/// The share of the host afkconvoy may use itself, each in percent, so that
/// a co-resident system keeps the rest; `None`: no cap (sunfmin/control#896).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Cap {
    /// Of the host's cores.
    pub cores: Option<u64>,
    /// Of the host's memory.
    pub memory: Option<u64>,
}

/// What afkconvoy's own processes on the host use now: its host runner, the
/// fleets' workers and what runs in its exclusive queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Own {
    /// The cores they take, in hundredths of a core.
    pub cpu: u64,
    /// The bytes they hold resident.
    pub resident: u64,
}

/// The limits `cap` comes to on a host of `cores` cores and `memory` bytes:
/// hundredths of a core, and bytes; `None` where it caps nothing, or the
/// memory was not read.
pub fn cap_limits(cap: Cap, cores: u64, memory: Option<u64>) -> (Option<u64>, Option<u64>) {
    let cpu = cap.cores.map(|share| cores.saturating_mul(share));
    let resident = cap.memory.zip(memory).map(|(share, memory)| share_of(memory, share));
    (cpu, resident)
}

/// Whether afkconvoy's own use `own` on a host of `cores` cores and `memory`
/// bytes leaves room for a start under `cap`: none, and it does. A start
/// adds to its use, so a use at its limit already holds; what could not be
/// read holds nothing back.
pub fn cap_holds(cap: Cap, own: Option<Own>, cores: u64, memory: Option<u64>) -> Vec<Hold> {
    let Some(own) = own else { return vec![] };
    let (cpu, resident) = cap_limits(cap, cores, memory);
    let mut holds = vec![];
    if let Some(limit) = cpu
        && own.cpu >= limit
    {
        holds.push(Hold::OwnCores { used: own.cpu, limit });
    }
    if let Some(limit) = resident
        && own.resident >= limit
    {
        holds.push(Hold::OwnMemory { resident: own.resident, limit });
    }
    holds
}

/// The longest, in seconds, the host's exclusive queue may take to drain
/// before admission starts no new worker, where the host says none: a new
/// worker would only wait behind its gate.
pub const QUEUE_BOUND: u64 = 900;

/// One usage window of an account a checkout's workers start on, as its
/// engine reported it last (GLOSSARY: Usage window).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Window {
    /// The harness whose account it is, by its key.
    pub account: String,
    /// The engine's own name for it.
    pub window: String,
    /// How much of it is used, in percent.
    pub used: u64,
    /// How long it is, in seconds.
    pub length: u64,
    /// How long until it resets, in seconds.
    pub resets_in: u64,
}

/// How much of `window` would be used as it resets, in percent, at the rate
/// it was used at since it began, with one more worker beside `running`:
/// each running worker uses its share of that rate, and the new one as much
/// again. `None`: no rate can be told — the window has only just begun, or
/// says it resets later than its length.
pub fn projected(window: &Window, running: usize) -> Option<u64> {
    let elapsed = window.length.checked_sub(window.resets_in).filter(|elapsed| *elapsed > 0)?;
    let workers = running.max(1) as u128;
    let more = (window.used as u128)
        .saturating_mul(window.resets_in as u128)
        .saturating_mul(running as u128 + 1)
        .checked_div((elapsed as u128).saturating_mul(workers))?;
    Some(u64::try_from((window.used as u128).saturating_add(more)).unwrap_or(u64::MAX))
}

/// The windows of `windows` that would run out before they reset with one
/// more worker beside the `running` ones on their account (`projected`):
/// none, and the accounts admit a start. A window already used up holds too.
pub fn window_holds(windows: &[Window], running: impl Fn(&str) -> usize) -> Vec<Hold> {
    let mut holds = vec![];
    for window in windows {
        let running = running(&window.account);
        let projected = match projected(window, running) {
            Some(projected) => projected,
            None if window.used >= 100 => window.used,
            None => continue,
        };
        if projected > 100 || window.used >= 100 {
            holds.push(Hold::Window {
                account: window.account.clone(),
                window: window.window.clone(),
                used: window.used,
                projected,
                running,
                resets_in: window.resets_in,
            });
        }
    }
    holds
}

/// Whether the host's exclusive queue, `drain` seconds from empty, is past
/// `bound`: a new worker would only wait behind its gate. What could not be
/// told holds nothing back; at its bound holds nothing.
pub fn queue_hold(drain: Option<u64>, bound: u64) -> Option<Hold> {
    drain.filter(|drain| *drain > bound).map(|drain| Hold::Queue { drain, bound })
}

/// How long the host's exclusive queue takes to drain, in seconds: each
/// lane's commands one after another, as many at once as it has places, the
/// longest lane's. Each command is `(lane, places, seconds)`: what is left of
/// a running one, the whole of a waiting one, by its recent run times.
pub fn drain(commands: &[(String, u64, u64)]) -> u64 {
    let mut lanes: std::collections::BTreeMap<&str, (u64, u64)> = Default::default();
    for (lane, places, seconds) in commands {
        let entry = lanes.entry(lane.as_str()).or_insert((*places, 0));
        entry.1 = entry.1.saturating_add(*seconds);
    }
    lanes
        .values()
        .map(|(places, seconds)| seconds.checked_div(*places).unwrap_or(*seconds))
        .max()
        .unwrap_or(0)
}

/// The thresholds a host read as `reading` is past under `shares`, load
/// first, then memory, then disk: none, and admission lets new workers start.
/// What could not be read holds nothing back. Past means strictly past: a
/// load at its limit, or free memory or disk at its floor, holds nothing.
pub fn holds(shares: Shares, reading: &Reading) -> Vec<Hold> {
    let limits = thresholds(shares, reading);
    let mut holds = vec![];
    if let Some(load) = reading.load
        && load > limits.load
    {
        holds.push(Hold::Load { load, limit: limits.load });
    }
    if let Some(free) = reading.memory_free
        && free < limits.memory_free
    {
        holds.push(Hold::Memory { free, floor: limits.memory_free });
    }
    if let (Some(disk), Some(floor)) = (reading.disk, limits.disk_free)
        && disk.free < floor
    {
        holds.push(Hold::Disk { free: disk.free, floor });
    }
    holds
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    /// A host of 10 cores and 1000 GiB of disk, at ease.
    fn idle() -> Reading {
        Reading {
            cores: 10,
            load: Some(100),
            memory_free: Some(60),
            disk: Some(Disk { free: 500 * GIB, size: 1000 * GIB }),
        }
    }

    #[test]
    fn a_host_at_ease_holds_nothing_back() {
        assert_eq!(holds(SHARES, &idle()), []);
    }

    #[test]
    fn the_defaults_are_shares_of_the_host() {
        let limits = thresholds(SHARES, &idle());
        assert_eq!(limits.load, 2000, "twice the 10 cores, in hundredths");
        assert_eq!(limits.memory_free, 10);
        assert_eq!(limits.disk_free, Some(1000 * GIB / 100));
    }

    #[test]
    fn a_load_holds_only_once_past_its_limit() {
        let at = Reading { load: Some(2000), ..idle() };
        assert_eq!(holds(SHARES, &at), []);
        let past = Reading { load: Some(2001), ..idle() };
        assert_eq!(holds(SHARES, &past), [Hold::Load { load: 2001, limit: 2000 }]);
    }

    #[test]
    fn memory_holds_only_once_under_its_floor() {
        let at = Reading { memory_free: Some(10), ..idle() };
        assert_eq!(holds(SHARES, &at), []);
        let under = Reading { memory_free: Some(9), ..idle() };
        assert_eq!(holds(SHARES, &under), [Hold::Memory { free: 9, floor: 10 }]);
    }

    #[test]
    fn disk_holds_only_once_under_its_floor() {
        let floor = 1000 * GIB / 100;
        let at = Reading { disk: Some(Disk { free: floor, size: 1000 * GIB }), ..idle() };
        assert_eq!(holds(SHARES, &at), []);
        let under = Reading { disk: Some(Disk { free: floor - 1, size: 1000 * GIB }), ..idle() };
        assert_eq!(holds(SHARES, &under), [Hold::Disk { free: floor - 1, floor }]);
    }

    #[test]
    fn every_threshold_past_is_named_load_first() {
        let busy = Reading {
            cores: 10,
            load: Some(9000),
            memory_free: Some(3),
            disk: Some(Disk { free: GIB, size: 1000 * GIB }),
        };
        let by: Vec<&str> = holds(SHARES, &busy)
            .iter()
            .map(|hold| match hold {
                Hold::Load { .. } => "load",
                Hold::Memory { .. } => "memory",
                Hold::Disk { .. } => "disk",
                _ => "other",
            })
            .collect();
        assert_eq!(by, ["load", "memory", "disk"]);
    }

    #[test]
    fn what_was_not_read_holds_nothing_back() {
        let unread = Reading { cores: 1, load: None, memory_free: None, disk: None };
        assert_eq!(holds(Shares { load: 0, memory: 100, disk: 100 }, &unread), []);
    }

    #[test]
    fn the_host_says_its_own_shares() {
        let strict = Shares { load: 5, memory: 70, disk: 60 };
        assert_eq!(holds(strict, &idle()).len(), 3);
        let lax = Shares { load: 10_000, memory: 0, disk: 0 };
        let busy = Reading { load: Some(9000), memory_free: Some(0), ..idle() };
        assert_eq!(holds(lax, &busy), []);
    }

    /// A five-hour window `used` percent used, resetting in `resets_in`
    /// seconds.
    fn five_hour(used: u64, resets_in: u64) -> Window {
        Window {
            account: "codex".into(),
            window: "primary".into(),
            used,
            length: 5 * 3600,
            resets_in,
        }
    }

    #[test]
    fn a_window_is_projected_at_its_present_rate_with_room_for_one_more() {
        // 40% in the first half: alone, 80% as it resets.
        assert_eq!(projected(&five_hour(40, 9000), 0), Some(80));
        // Two at work make that rate; a third adds half of it again.
        assert_eq!(projected(&five_hour(40, 9000), 2), Some(100));
        assert_eq!(projected(&five_hour(40, 9000), 3), Some(93));
        assert_eq!(projected(&five_hour(10, 5 * 3600), 1), None, "only just begun");
        assert_eq!(projected(&five_hour(10, 6 * 3600), 1), None, "longer than it is");
    }

    #[test]
    fn a_window_holds_only_once_it_would_run_out_before_it_resets() {
        let windows = [five_hour(40, 9000)];
        assert_eq!(window_holds(&windows, |_| 2), [], "at 100% it runs out as it resets");
        let held = window_holds(&[five_hour(60, 9000)], |_| 1);
        assert_eq!(
            held,
            [Hold::Window {
                account: "codex".into(),
                window: "primary".into(),
                used: 60,
                projected: 180,
                running: 1,
                resets_in: 9000,
            }]
        );
        let spent = window_holds(&[five_hour(100, 5 * 3600)], |_| 0);
        assert_eq!(spent.len(), 1, "a window used up holds, rate or none");
        assert_eq!(window_holds(&[], |_| 9), [], "a harness that reports none");
    }

    #[test]
    fn a_queue_holds_only_once_it_takes_longer_than_its_bound_to_drain() {
        assert_eq!(queue_hold(Some(QUEUE_BOUND), QUEUE_BOUND), None);
        assert_eq!(
            queue_hold(Some(QUEUE_BOUND + 1), QUEUE_BOUND),
            Some(Hold::Queue { drain: QUEUE_BOUND + 1, bound: QUEUE_BOUND })
        );
        assert_eq!(queue_hold(None, 0), None, "a queue that could not be read");
    }

    #[test]
    fn a_queue_drains_lane_by_lane_as_many_at_once_as_each_has_places() {
        let commands = [
            ("worker".to_string(), 2, 600),
            ("worker".to_string(), 2, 600),
            ("worker".to_string(), 2, 600),
            ("fast".to_string(), 1, 200),
        ];
        assert_eq!(drain(&commands), 900);
        assert_eq!(drain(&[]), 0);
        assert_eq!(drain(&[("fast".to_string(), 0, 300)]), 300, "a lane of no place runs one");
    }

    #[test]
    fn a_cap_holds_a_start_that_would_take_afkconvoy_s_own_use_over_it() {
        let half = Cap { cores: Some(50), memory: Some(50) };
        let limit = share_of(16 * GIB, 50);
        let under = Own { cpu: 499, resident: limit - 1 };
        assert_eq!(cap_holds(half, Some(under), 10, Some(16 * GIB)), []);
        let at = Own { cpu: 500, resident: limit };
        assert_eq!(
            cap_holds(half, Some(at), 10, Some(16 * GIB)),
            [Hold::OwnCores { used: 500, limit: 500 }, Hold::OwnMemory { resident: limit, limit }]
        );
        let busy = Own { cpu: 900, resident: 15 * GIB };
        assert_eq!(cap_holds(Cap::default(), Some(busy), 10, Some(16 * GIB)), [], "no cap");
        assert_eq!(cap_holds(half, None, 10, Some(16 * GIB)), [], "not read");
        assert_eq!(
            cap_holds(half, Some(busy), 10, None),
            [Hold::OwnCores { used: 900, limit: 500 }],
            "memory not read: only the cores"
        );
    }

    #[test]
    fn shares_that_overflow_hold_at_the_most_there_is() {
        let huge = Reading { cores: u64::MAX, load: Some(u64::MAX), ..idle() };
        assert_eq!(holds(Shares { load: 200, ..SHARES }, &huge), []);
    }
}

//! Admission: a tick starts no new worker while its host is busy — past a
//! share of its load, under memory pressure, or low on free disk
//! (docs/adr/0019). Whether admission holds is decided here, from what the
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
/// tenth of its memory free, a twentieth of its disk free.
pub const SHARES: Shares = Shares { load: 200, memory: 10, disk: 5 };

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Hold {
    /// The load, in hundredths, is past `limit`.
    Load { load: u64, limit: u64 },
    /// The share of memory free, in percent, is under `floor`.
    Memory { free: u64, floor: u64 },
    /// The bytes free on the checkout's volume are under `floor`.
    Disk { free: u64, floor: u64 },
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
        assert_eq!(limits.disk_free, Some(1000 * GIB / 100 * 5));
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
        let floor = 1000 * GIB / 100 * 5;
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

    #[test]
    fn shares_that_overflow_hold_at_the_most_there_is() {
        let huge = Reading { cores: u64::MAX, load: Some(u64::MAX), ..idle() };
        assert_eq!(holds(Shares { load: 200, ..SHARES }, &huge), []);
    }
}

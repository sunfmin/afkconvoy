//! The disk a host builds on (sunfmin/control#1126): how much of it the
//! host's builds need kept free, which of its worktrees' `target/` dirs may be
//! reclaimed to keep it, and when its free space falls fast enough to be an
//! episode. Decided here, from what the effect layer read; reading the disk,
//! the worktrees and their processes is the effect layer's.
//!
//! A build fills the disk while it runs: a fixed share of the volume holds
//! back nothing that is already building. So the floor is what the builds
//! that may run at once take, measured: the largest fresh build on record on
//! the host, times the lanes of its exclusive queue and the workers that may
//! build beside them. Admission starts no worker under it, and the queue
//! grants no new heavy command; the `target/` dirs no one builds in are
//! reclaimed first, before anything is refused.

use serde::{Deserialize, Serialize};

/// The most of a volume the measured floor may ask kept free, in percent: a
/// host whose builds would ask more still builds, one at a time as space
/// returns, rather than never.
pub const RESERVE_MOST: u64 = 25;

/// How long since a worktree's `target/` was last written it still counts as
/// building, in seconds: cargo writes into it while it builds, and a link or
/// a test binary can take a few minutes.
pub const BUILDING_WITHIN: u64 = 600;

/// How long a worktree's `target/` may go unwritten before it is idle, in
/// seconds, where its worker still runs: six hours.
pub const IDLE_AFTER: u64 = 6 * 3600;

/// The fall of the disk's free space, in bytes a minute, past which it is an
/// episode whatever the floor: 2 GiB a minute empties 100 GB in under an hour.
pub const FALLING: u64 = 2 << 30;

/// How far back the fall of the disk's free space is read, in seconds.
pub const FALL_OVER: u64 = 300;

/// The bytes the builds that may run at once take: `build`, the largest fresh
/// build on record, times `at_once`, how many may build at once.
pub fn reserve(build: u64, at_once: u64) -> u64 {
    build.saturating_mul(at_once)
}

/// The bytes free under which the disk of `size` bytes holds new work back:
/// `share` percent of it, or the measured `reserve` where that is more — but
/// never more than `RESERVE_MOST` percent of it.
pub fn floor(size: u64, share: u64, reserve: Option<u64>) -> u64 {
    let shared = share_of(size, share);
    let measured = match reserve {
        Some(reserve) => reserve.min(share_of(size, RESERVE_MOST)),
        None => 0,
    };
    shared.max(measured)
}

/// `percent` of `bytes`: divided first, for the product may not fit.
pub fn share_of(bytes: u64, percent: u64) -> u64 {
    (bytes / 100).saturating_mul(percent)
}

/// One worktree's `target/`, as the effect layer read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    /// The worktree it is in.
    pub worktree: String,
    /// When it was last written, in epoch seconds; `None`: it could not be told.
    pub written: Option<u64>,
    /// Whether a command of the exclusive queue runs in the worktree now.
    pub queued: bool,
    /// Whether its worker's processes run: `false` once the worker ended —
    /// its issue settled, or it waits with no process of its own.
    pub worker: bool,
}

/// Why a `target/` may be reclaimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// No worker runs in its worktree.
    Ended,
    /// Nothing wrote it for `IDLE_AFTER`.
    Idle,
}

impl Target {
    /// Whether something builds in it now: a queue command runs there, or it
    /// was written within `BUILDING_WITHIN` of `now`.
    pub fn building(&self, now: u64) -> bool {
        self.queued || self.written.is_some_and(|at| now.saturating_sub(at) < BUILDING_WITHIN)
    }

    /// Why it may be reclaimed at `now`, its worktree idle after `idle_after`
    /// seconds; `None`: it may not. One that builds never may.
    pub fn reclaimable(&self, now: u64, idle_after: u64) -> Option<Why> {
        if self.building(now) {
            return None;
        }
        if !self.worker {
            return Some(Why::Ended);
        }
        match self.written {
            Some(at) if now.saturating_sub(at) < idle_after => None,
            _ => Some(Why::Idle),
        }
    }
}

/// Those of `targets` to reclaim at `now`, in the order to reclaim them:
/// those whose worker ended first, then the idle, each the longest unwritten
/// first. One that builds is never among them.
pub fn to_reclaim(targets: &[Target], now: u64, idle_after: u64) -> Vec<(usize, Why)> {
    let mut chosen: Vec<(usize, Why, u64)> = targets
        .iter()
        .enumerate()
        .filter_map(|(at, target)| {
            let why = target.reclaimable(now, idle_after)?;
            Some((at, why, target.written.unwrap_or(0)))
        })
        .collect();
    chosen.sort_by_key(|(_, why, written)| (*why != Why::Ended, *written));
    chosen.into_iter().map(|(at, why, _)| (at, why)).collect()
}

/// How fast the disk's free space fell, in bytes a minute, by `samples` —
/// each `(when, free)`, oldest first — over the last `over` seconds of them:
/// from the oldest of those to the newest. `None`: they span under a minute.
/// Free space that rose fell by nothing.
pub fn fall(samples: &[(u64, u64)], over: u64) -> Option<u64> {
    let (newest_at, newest_free) = *samples.last()?;
    let since = newest_at.saturating_sub(over);
    let (oldest_at, oldest_free) = *samples.iter().find(|(at, _)| *at >= since)?;
    let span = newest_at.saturating_sub(oldest_at);
    if span < 60 {
        return None;
    }
    let fell = oldest_free.saturating_sub(newest_free) as u128;
    Some(u64::try_from(fell * 60 / span as u128).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn the_floor_is_what_the_builds_at_once_take_never_under_its_share_nor_past_a_quarter() {
        let size = 460 * GIB;
        assert_eq!(floor(size, 1, None), share_of(size, 1), "nothing measured: the share");
        let twelve = reserve(5 * GIB, 12);
        assert_eq!(floor(size, 1, Some(twelve)), 60 * GIB, "five GiB, twelve at once");
        assert_eq!(floor(size, 1, Some(reserve(5 * GIB, 40))), share_of(size, RESERVE_MOST));
        assert_eq!(floor(size, 30, Some(twelve)), share_of(size, 30), "a larger share stands");
    }

    fn target(written: Option<u64>, queued: bool, worker: bool) -> Target {
        Target { worktree: "/w".into(), written, queued, worker }
    }

    #[test]
    fn a_target_someone_builds_in_is_never_reclaimed() {
        let now = 100_000;
        assert_eq!(target(Some(now - 60), false, false).reclaimable(now, IDLE_AFTER), None);
        assert_eq!(target(Some(0), true, false).reclaimable(now, IDLE_AFTER), None, "a gate runs");
        assert_eq!(
            target(Some(now - BUILDING_WITHIN), false, false).reclaimable(now, IDLE_AFTER),
            Some(Why::Ended)
        );
        assert_eq!(target(Some(now - 3600), false, true).reclaimable(now, IDLE_AFTER), None);
        assert_eq!(
            target(Some(now - IDLE_AFTER), false, true).reclaimable(now, IDLE_AFTER),
            Some(Why::Idle)
        );
        assert_eq!(target(None, false, true).reclaimable(now, IDLE_AFTER), Some(Why::Idle));
    }

    #[test]
    fn ended_worktrees_are_reclaimed_first_then_the_idle_longest_unwritten_first() {
        let now = 100_000;
        let targets = [
            target(Some(now - 7 * 3600), false, true),
            target(Some(now - 2000), false, false),
            target(Some(now - 9 * 3600), false, true),
            target(Some(now - 5), false, false),
            target(Some(now - 5000), false, false),
        ];
        assert_eq!(
            to_reclaim(&targets, now, IDLE_AFTER),
            [(4, Why::Ended), (1, Why::Ended), (2, Why::Idle), (0, Why::Idle)]
        );
    }

    #[test]
    fn the_fall_is_read_over_its_window_in_bytes_a_minute() {
        let samples = [(0, 500 * GIB), (100, 95 * GIB), (160, 92 * GIB), (400, 80 * GIB)];
        assert_eq!(fall(&samples, FALL_OVER), Some(3 * GIB), "15 GiB in the last five minutes");
        assert_eq!(fall(&samples[..2], FALL_OVER), Some(243 * GIB));
        assert_eq!(fall(&[(0, 10), (30, 0)], FALL_OVER), None, "under a minute");
        assert_eq!(fall(&[(0, 10), (60, 20)], FALL_OVER), Some(0), "it rose");
        assert_eq!(fall(&[], FALL_OVER), None);
    }
}

//! Ages and the claim lease: every "how old is this stamp" the fleet acts on.

use flux_rs::attrs::*;

/// A claim is live while its owner's heartbeat is this fresh, in seconds.
pub const LEASE: u64 = 4500;
/// How far ahead of the reader a stamp may be and still count as "just now".
pub const SKEW: u64 = 300;
/// An owner refreshes its heartbeat once it is older than this: a third of the lease.
pub const BEAT_EVERY: u64 = 1500;

/// Whether a heartbeat stamped `stamp` says its owner is dead at `now`: older
/// than the lease, or stamped further ahead of `now` than `SKEW` — a stamp ahead
/// by no more than that is ordinary skew between two hosts and reads as just now. The fleet's one lease
/// comparison: a heartbeat exactly `LEASE` old is live to everyone.
#[spec(fn(stamp: u64, now: u64) -> bool[
    (stamp > now && stamp - now > SKEW) || (stamp <= now && now - stamp > LEASE)])]
pub fn stamp_stale(stamp: u64, now: u64) -> bool {
    if stamp <= now { now - stamp > LEASE } else { stamp - now > SKEW }
}

/// Whether an owner whose heartbeat is stamped `stamp` must refresh it at `now`.
#[spec(fn(stamp: u64, now: u64) -> bool[
    (stamp > now && stamp - now > SKEW) || (stamp <= now && now - stamp > BEAT_EVERY)])]
pub fn stamp_due(stamp: u64, now: u64) -> bool {
    if stamp <= now { now - stamp > BEAT_EVERY } else { stamp - now > SKEW }
}

/// A heartbeat that is missing is an owner that never beat: stale.
pub fn is_stale(last_beat: Option<u64>, now: u64) -> bool {
    match last_beat {
        Some(stamp) => stamp_stale(stamp, now),
        None => true,
    }
}

/// Whether my own heartbeat is due: never beaten, or `stamp_due`.
pub fn heartbeat_due(last_beat: Option<u64>, now: u64) -> bool {
    match last_beat {
        Some(stamp) => stamp_due(stamp, now),
        None => true,
    }
}

/// A heartbeat that is not due is live to every peer, and stays live for two
/// thirds of a lease: an owner that beats when due is never read as dead.
#[spec(fn(stamp: u64, now: u64, later: u64)
    requires now <= later && later - now <= LEASE - BEAT_EVERY)]
pub fn a_beat_not_due_outlives_the_wait(stamp: u64, now: u64, later: u64) {
    if !stamp_due(stamp, now) && stamp <= now {
        proved(!stamp_stale(stamp, later));
    }
}

/// A fact Flux has proved where this is called.
#[spec(fn(bool[true]))]
pub fn proved(_: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lease_boundary_is_live() {
        assert!(!is_stale(Some(1000), 1000 + LEASE));
        assert!(is_stale(Some(1000), 1001 + LEASE));
        assert!(is_stale(None, 5), "an owner that never beat");
        assert!(is_stale(Some(1301), 1000), "stamped from the future");
        assert!(!is_stale(Some(1300), 1000), "skew reads as just now");
    }

    #[test]
    fn a_beat_is_due_at_a_third_of_the_lease() {
        assert!(!heartbeat_due(Some(1000), 1000 + BEAT_EVERY));
        assert!(heartbeat_due(Some(1000), 1001 + BEAT_EVERY));
        assert!(heartbeat_due(None, 5));
    }
}

//! Ages and the claim lease: every "how old is this stamp" the fleet acts on.

use flux_rs::attrs::*;

/// A fleet instance is live while its heartbeat is this fresh, in seconds:
/// half an hour. Past it, whatever it holds — its claims, and its members of
/// a candidate chain — is any other host's to take.
pub const LEASE: u64 = 1800;
/// How far ahead of the reader a stamp may be and still count as "just now":
/// the gate's, which believes a record by the same skew.
pub const SKEW: u64 = afkconvoy_gate::gate::SKEW;
/// An owner refreshes its heartbeat once it is older than this: a third of the lease.
pub const BEAT_EVERY: u64 = 600;

/// A heartbeat that holds nothing is forgotten once it is this old, in seconds:
/// a day, whichever host stamped it.
pub const FORGOTTEN: u64 = 86_400;

/// Whether a heartbeat stamped `stamp` says its owner is dead at `now`: older
/// than the lease, or stamped further ahead of `now` than `SKEW` — a stamp ahead
/// by no more than that is ordinary skew between two hosts and reads as just now. The fleet's one lease
/// comparison: a heartbeat exactly `LEASE` old is live to everyone.
#[spec(fn(stamp: u64, now: u64) -> bool[older_than(stamp, now, LEASE)])]
pub fn stamp_stale(stamp: u64, now: u64) -> bool {
    if stamp <= now { now - stamp > LEASE } else { stamp - now > SKEW }
}

/// Whether an owner whose heartbeat is stamped `stamp` must refresh it at `now`.
#[spec(fn(stamp: u64, now: u64) -> bool[older_than(stamp, now, BEAT_EVERY)])]
pub fn stamp_due(stamp: u64, now: u64) -> bool {
    if stamp <= now { now - stamp > BEAT_EVERY } else { stamp - now > SKEW }
}

/// Whether a heartbeat stamped `stamp` is forgotten at `now`: older than
/// `FORGOTTEN`, or stamped further ahead of `now` than `SKEW`.
#[spec(fn(stamp: u64, now: u64) -> bool[older_than(stamp, now, FORGOTTEN)])]
pub fn stamp_forgotten(stamp: u64, now: u64) -> bool {
    if stamp <= now { now - stamp > FORGOTTEN } else { stamp - now > SKEW }
}

// A stamp is older than `limit` at `now`, or further ahead of it than `SKEW`.
defs! {
    fn older_than(stamp: int, now: int, limit: int) -> bool {
        (stamp > now && stamp - now > SKEW) || (stamp <= now && now - stamp > limit)
    }
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

/// How old a stamp another clock wrote is, in seconds; `None` when it is no
/// evidence of anything: further ahead of `now` than `SKEW`. A stamp ahead by
/// no more than that is ordinary skew between two hosts and reads as "just
/// now", never as a negative age.
#[spec(fn(stamp: u64, now: u64) -> Option<u64{age: age <= now}>)]
pub fn stamp_age(stamp: u64, now: u64) -> Option<u64> {
    if stamp <= now {
        Some(now - stamp)
    } else if stamp - now <= SKEW {
        Some(0)
    } else {
        None
    }
}

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
    fn a_heartbeat_is_forgotten_after_a_day() {
        assert!(!stamp_forgotten(1000, 1000 + FORGOTTEN));
        assert!(stamp_forgotten(1000, 1001 + FORGOTTEN));
        assert!(stamp_forgotten(1301, 1000), "stamped from the future");
        assert!(!stamp_forgotten(1300, 1000), "skew reads as just now");
    }

    #[test]
    fn a_beat_is_due_at_a_third_of_the_lease() {
        assert!(!heartbeat_due(Some(1000), 1000 + BEAT_EVERY));
        assert!(heartbeat_due(Some(1000), 1001 + BEAT_EVERY));
        assert!(heartbeat_due(None, 5));
    }
}

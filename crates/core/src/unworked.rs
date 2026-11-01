//! A ready issue nobody has taken. Time comes from the tracker, never from
//! when a host first saw it; any claim leaves recovery to the lease takeover.

/// A ready issue must have waited strictly longer than an hour.
pub const WAIT: u64 = 3600;

/// Seconds waited since both readiness and the last blocker closing. An
/// open blocker, a claim, a missing timestamp or a future timestamp proves
/// no wait. Removing and applying the ready label starts the wait again.
pub fn waited(
    ready: Option<u64>,
    blockers: &[Option<u64>],
    claimed: bool,
    now: u64,
) -> Option<u64> {
    if claimed || blockers.iter().any(Option::is_none) {
        return None;
    }
    let since = blockers.iter().flatten().copied().fold(ready?, u64::max);
    now.checked_sub(since).filter(|age| *age > WAIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_and_every_blocker_must_have_stood_for_over_an_hour() {
        let now = 20_000;
        assert_eq!(waited(Some(now - 59 * 60), &[], false, now), None);
        assert_eq!(waited(Some(now - WAIT), &[], false, now), None);
        assert_eq!(waited(Some(now - 61 * 60), &[], false, now), Some(3660));
        assert_eq!(waited(Some(1), &[Some(2), None], false, now), None);
        assert_eq!(waited(Some(1), &[Some(now - 59 * 60)], false, now), None);
        assert_eq!(waited(Some(1), &[Some(now - 61 * 60)], false, now), Some(3660));
        assert_eq!(waited(Some(now - 59 * 60), &[Some(1)], false, now), None);
        assert_eq!(waited(Some(1), &[], true, now), None);
        assert_eq!(waited(None, &[], false, now), None);
        assert_eq!(waited(Some(now + 1), &[], false, now), None);
    }
}

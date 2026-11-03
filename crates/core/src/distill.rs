//! The watch's weekly **distillation** of the daybook: when a look reads
//! the daybook for it, and when what a span of days holds calls for a round.
//! The look's reading and the round are the effect layer's.

/// A distillation starts a round at most once a week, unless the operator
/// asks for one.
pub const EVERY: u64 = 7 * 24 * 60 * 60;

/// A due distillation whose span held nothing is read again after this long.
pub const READ_AGAIN: u64 = 60 * 60;

/// Distillation item identities: `daybook-distill:<tracker>:<since>:<until>`.
pub const ITEM: &str = "daybook-distill:";

/// Whether a look reads the daybook for a distillation now: a week after the
/// last round that took one started, or when the operator asked; and, while
/// unasked, not again within an hour of a reading that found nothing.
pub fn due(started: Option<u64>, asked: bool, read: Option<u64>, now: u64) -> bool {
    if asked {
        return true;
    }
    let weekly = match started {
        Some(started) => now.saturating_sub(started) >= EVERY,
        None => true,
    };
    let fresh = match read {
        Some(read) => now.saturating_sub(read) >= READ_AGAIN,
        None => true,
    };
    weekly && fresh
}

/// Whether a span's entries call for a distillation: a lesson, or two
/// entries without one, enough for a fault to recur.
pub fn holds(lessons: usize, unlessoned: usize) -> bool {
    lessons > 0 || unlessoned >= 2
}

/// The item identity of a distillation of `tracker`'s daybook over the days
/// `since` to `until`.
pub fn item(tracker: &str, since: &str, until: &str) -> String {
    format!("{ITEM}{tracker}:{since}:{until}")
}

/// The last day a distillation item's span covers.
pub fn until_of(id: &str) -> Option<&str> {
    id.strip_prefix(ITEM)?.rsplit(':').next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distillation_is_read_weekly_or_when_asked_and_not_hourly_when_empty() {
        assert!(due(None, false, None, 0));
        assert!(!due(Some(100), false, None, 100 + EVERY - 1));
        assert!(due(Some(100), false, None, 100 + EVERY));
        assert!(due(Some(100), true, None, 101), "the operator's ask needs no week");
        let now = 100 + EVERY;
        assert!(!due(Some(100), false, Some(now - 1), now), "an empty span is read hourly");
        assert!(due(Some(100), false, Some(now - READ_AGAIN), now));
        assert!(due(Some(100), true, Some(now), now));
    }

    #[test]
    fn a_lesson_or_two_plain_entries_call_for_a_round() {
        assert!(!holds(0, 0));
        assert!(!holds(0, 1));
        assert!(holds(0, 2));
        assert!(holds(1, 0));
    }

    #[test]
    fn an_item_says_its_span() {
        let id = item("acme/control", "2026-10-27", "2026-11-02");
        assert_eq!(id, "daybook-distill:acme/control:2026-10-27:2026-11-02");
        assert_eq!(until_of(&id), Some("2026-11-02"));
        assert_eq!(until_of("kit-research:x"), None);
    }
}

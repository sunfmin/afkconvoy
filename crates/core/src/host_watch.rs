//! The host runner's watch: which items of a doctor look call for one, and
//! whether the watch has reached its bound. The look and its sessions are the
//! effect layer's.

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

/// One watch may last an hour, including every continuation of it.
pub const TIME_LIMIT: u64 = 3600;

/// One thing an operator would act on. Its identity is stable while its
/// episode lasts; its reason is what the watch is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub reason: String,
}

impl Item {
    pub fn new(id: impl Into<String>, reason: impl Into<String>) -> Item {
        Item { id: id.into(), reason: reason.into() }
    }
}

/// The items that should start a watch now. A running watch defers every new
/// item; without one, only items not already watched in their current episode
/// are returned. Items already watched stay covered until a later look no
/// longer contains them.
pub fn pending(current: &[Item], watched: &[String], running: bool) -> Vec<Item> {
    if running {
        return vec![];
    }
    current.iter().filter(|item| !watched.contains(&item.id)).cloned().collect()
}

/// The watched identities that are still in their episodes after a look.
pub fn covered(watched: &[String], current: &[Item]) -> Vec<String> {
    watched.iter().filter(|id| current.iter().any(|item| &item.id == *id)).cloned().collect()
}

/// Whether a watch started at `started` has used its whole hour by `now`.
#[spec(fn(started: u64, now: u64) -> bool)]
pub fn expired(started: u64, now: u64) -> bool {
    now.saturating_sub(started) >= TIME_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> Item {
        Item::new(id, format!("act on {id}"))
    }

    #[test]
    fn an_item_starts_at_most_one_watch_until_its_episode_ends() {
        let current = vec![item("finding:one"), item("human:#7")];
        let due = pending(&current, &[], false);
        assert_eq!(due, current);
        let watched: Vec<String> = due.iter().map(|item| item.id.clone()).collect();
        assert!(pending(&current, &watched, false).is_empty());
        assert!(pending(&current, &[], true).is_empty(), "a running watch defers even new items");

        let with_new = [current.clone(), vec![item("tick:9")]].concat();
        assert_eq!(pending(&with_new, &watched, false), [item("tick:9")]);
        let covered = covered(&watched, &current[..1]);
        assert_eq!(covered, ["finding:one"]);
        assert_eq!(pending(&with_new, &covered, false), [item("human:#7"), item("tick:9")]);
    }

    #[test]
    fn continuations_share_one_hour() {
        assert!(!expired(100, 100 + TIME_LIMIT - 1));
        assert!(expired(100, 100 + TIME_LIMIT));
        assert!(expired(100, u64::MAX));
        assert!(!expired(100, 50));
    }
}

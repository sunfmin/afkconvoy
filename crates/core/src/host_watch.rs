//! The host runner's watch: which items of a doctor look call for one, the
//! line its terminal runs, what its transcript says of how it went, and
//! whether it has reached its bound. The look and its sessions are the effect
//! layer's.

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

use crate::line::Line;

/// One watch may last an hour, including every continuation of it.
pub const TIME_LIMIT: u64 = 3600;

/// What of a session's transcript is kept as what it did: its last lines,
/// each cut to a length a log line can hold.
pub const DID_LINES: usize = 80;
const DID_CHARS: usize = 1000;

/// What a watch's session prints once its harness exited, with the exit status
/// after it, on a line of its own.
pub const EXITED: &str = "afkconvoy watch: the harness exited";

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

/// The line a watch's terminal runs: `line`, a harness's watch, with all it
/// prints kept in `transcript` too, then how it exited; then the terminal's
/// shell exits, and the terminal with it.
pub fn session(line: &str, transcript: &str) -> String {
    Line::new()
        .fixed("{ ")
        .launch(line)
        .fixed("; printf '%s %s\\n' '")
        .fixed(EXITED)
        .fixed("' \"$?\"; } 2>&1 | tee -a ")
        .word(transcript)
        .fixed("; exit")
        .done()
}

/// How the harness of a session whose transcript is `transcript` exited:
/// `None` while it has not.
pub fn exited(transcript: &str) -> Option<i32> {
    let status = transcript.lines().rev().find_map(|line| line.trim().strip_prefix(EXITED))?;
    Some(status.trim().parse().unwrap_or(-1))
}

/// What a session printed, as what it did: its transcript's last lines, but
/// for the line that says how it exited.
pub fn did(transcript: &str) -> Vec<String> {
    let printed: Vec<String> = transcript
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(EXITED))
        .map(|line| line.chars().take(DID_CHARS).collect())
        .collect();
    let skip = printed.len().saturating_sub(DID_LINES);
    printed.into_iter().skip(skip).collect()
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

    #[test]
    fn a_session_keeps_what_it_printed_and_says_how_its_harness_exited() {
        let line = session("claude --print 'look'", "/cache/watch/it's.log");
        assert!(line.starts_with("{ claude --print 'look'; printf "), "{line}");
        assert!(line.ends_with(" 2>&1 | tee -a '/cache/watch/it'\\''s.log'; exit"), "{line}");

        assert_eq!(exited("started\nchecked #7\n"), None);
        let printed = format!("checked #7\n\nanswered #7\n{EXITED} 0\n");
        assert_eq!(exited(&printed), Some(0));
        assert_eq!(did(&printed), ["checked #7", "answered #7"]);
        assert_eq!(exited(&format!("Usage limit reached\n{EXITED} 1\n")), Some(1));
    }

    #[test]
    fn what_a_session_did_is_its_last_lines() {
        let long: String = (0..DID_LINES + 5).map(|n| format!("line {n}\n")).collect();
        let did = did(&long);
        assert_eq!(did.len(), DID_LINES);
        assert_eq!(did[0], "line 5");
        assert_eq!(super::did(&"x".repeat(DID_CHARS * 2))[0].len(), DID_CHARS);
    }
}

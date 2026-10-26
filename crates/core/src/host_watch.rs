//! The host runner's watch: which items of a doctor look call for one, the
//! line its terminal runs, what its transcript says of how it went, and
//! whether it has reached its bound. The look and its sessions are the effect
//! layer's.

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

use crate::line::Line;

/// One watch may last an hour, including every continuation of it.
pub const TIME_LIMIT: u64 = 3600;

/// Research alone starts a watch at most once a day. Other work takes it too.
pub const RESEARCH_EVERY: u64 = 24 * 60 * 60;
/// Research item identities, distinct from findings that hold work up.
pub const RESEARCH_ITEM: &str = "kit-research:";

/// Bounded terminal evidence retained to continue a session: its last lines,
/// each cut to a length a log line can hold.
pub const EVIDENCE_LINES: usize = 80;
const EVIDENCE_CHARS: usize = 1000;

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
/// item. Without one, new episodes start it and take all pending research
/// too. Research alone starts it once a day, including during an unchanged
/// episode. Other watched items stay covered until absent from a later look.
pub fn pending(
    current: &[Item],
    watched: &[String],
    running: bool,
    research_started: Option<u64>,
    now: u64,
) -> Vec<Item> {
    let mut due = vec![];
    if running {
        return due;
    }
    for item in current {
        if !item.id.starts_with(RESEARCH_ITEM) && !watched.contains(&item.id) {
            due.push(item.clone());
        }
    }
    let research_due = match research_started {
        Some(started) => now.saturating_sub(started) >= RESEARCH_EVERY,
        None => true,
    };
    if !due.is_empty() || research_due {
        for item in current {
            if item.id.starts_with(RESEARCH_ITEM) {
                due.push(item.clone());
            }
        }
    }
    due
}

/// The watched identities that are still in their episodes after a look.
pub fn covered(watched: &[String], current: &[Item]) -> Vec<String> {
    let mut covered = vec![];
    for id in watched {
        for item in current {
            if &item.id == id {
                covered.push(id.clone());
                break;
            }
        }
    }
    covered
}

/// Whether a watch started at `started` has used its whole hour by `now`.
#[spec(fn(started: u64, now: u64) -> bool)]
pub fn expired(started: u64, now: u64) -> bool {
    now.saturating_sub(started) >= TIME_LIMIT
}

/// A marker present only in this watch session's processes, never in the
/// terminal's shell or a human's other sessions in the same directory.
pub const SESSION_ENV: &str = "AFKCONVOY_WATCH_SESSION";
/// The watch's wrapper shell has this unique argument even on hosts where
/// `ps` cannot read a process's environment. It stays until the harness ends.
pub const SESSION_PROCESS: &str = "afkconvoy-watch-session-";

/// Runs the interactive harness on the terminal's own input and output.
/// Only its exit status goes to `transcript`; its shell and screen stay.
pub fn session(line: &str, transcript: &str, identity: &str) -> String {
    let child = Line::new()
        .fixed("export ")
        .fixed(SESSION_ENV)
        .fixed("=")
        .word(identity)
        .fixed(" && ")
        .launch(line)
        .fixed("; afkconvoy_status=$?; exit \"$afkconvoy_status\"")
        .done();
    Line::new()
        .fixed("sh -c ")
        .word(&child)
        .fixed(" ")
        .word(&format!("{SESSION_PROCESS}{identity}"))
        .fixed("; printf '%s %s\\n' '")
        .fixed(EXITED)
        .fixed("' \"$?\" > ")
        .word(transcript)
        .own()
}

/// How the harness of a session whose transcript is `transcript` exited:
/// `None` while it has not.
pub fn exited(transcript: &str) -> Option<i32> {
    let status = transcript.lines().rev().find_map(|line| line.trim().strip_prefix(EXITED))?;
    Some(status.trim().parse().unwrap_or(-1))
}

/// The last lines of terminal evidence, excluding
/// for the line that says how it exited.
pub fn evidence(transcript: &str) -> Vec<String> {
    let mut did: Vec<String> = vec![];
    for line in transcript.lines().rev() {
        if did.len() >= EVIDENCE_LINES {
            break;
        }
        let line = line.trim();
        if !line.is_empty() && !line.starts_with(EXITED) {
            did.push(line.chars().take(EVIDENCE_CHARS).collect());
        }
    }
    did.reverse();
    did
}

/// The watch's identity in a guarded command's environment.
pub const WATCH_ENV: &str = "AFKCONVOY_WATCH";

/// What under the home folder cannot be rebuilt, or is expensive to fetch
/// again — model weights, downloads, media, Docker's images, the user's own
/// files: a watch never deletes it, and the guard refuses a deletion
/// it reads there.
pub const KEPT: [&str; 13] = [
    ".cache/huggingface",
    ".cache/modelscope",
    ".ollama",
    ".lmstudio",
    ".docker",
    "Library/Containers/com.docker.docker",
    "Downloads",
    "Movies",
    "Music",
    "Pictures",
    "Documents",
    "Desktop",
    "Library/Mobile Documents",
];

/// Whether `relative` — a path under the home folder, relative to it, its
/// parts parted by `/` — is in what is kept (`KEPT`), or holds some of it. A
/// part a shell pattern stands in (`*`, `?`, `[`) may be any.
pub fn kept(relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').filter(|part| !part.is_empty()).collect();
    let pattern = |part: &str| part.contains(['*', '?', '[']);
    KEPT.iter().any(|root| {
        let root: Vec<&str> = root.split('/').collect();
        parts.iter().zip(root.iter()).all(|(part, root)| part == root || pattern(part))
    })
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
        let due = pending(&current, &[], false, None, 0);
        assert_eq!(due, current);
        let watched: Vec<String> = due.iter().map(|item| item.id.clone()).collect();
        assert!(pending(&current, &watched, false, None, 0).is_empty());
        assert!(
            pending(&current, &[], true, None, 0).is_empty(),
            "a running watch defers even new items"
        );

        let with_new = [current.clone(), vec![item("tick:9")]].concat();
        assert_eq!(pending(&with_new, &watched, false, None, 0), [item("tick:9")]);
        let covered = covered(&watched, &current[..1]);
        assert_eq!(covered, ["finding:one"]);
        assert_eq!(
            pending(&with_new, &covered, false, None, 0),
            [item("human:#7"), item("tick:9")]
        );
    }

    #[test]
    fn research_is_daily_on_its_own_and_joins_every_other_watch() {
        let research = item("kit-research:codex");
        let current = [research.clone()];
        let watched = [research.id.clone()];
        assert_eq!(pending(&current, &[], false, None, 100), current);
        assert!(pending(&current, &[], false, Some(100), 99).is_empty());
        assert!(pending(&current, &watched, false, Some(100), 100 + RESEARCH_EVERY - 1).is_empty());
        assert_eq!(pending(&current, &watched, false, Some(100), 100 + RESEARCH_EVERY), current);
        let urgent = item("human:#7");
        assert_eq!(
            pending(&[research.clone(), urgent.clone()], &watched, false, Some(100), 101),
            [urgent, research]
        );
        assert!(pending(&current, &[], true, None, u64::MAX).is_empty());
    }

    #[test]
    fn continuations_share_one_hour() {
        assert!(!expired(100, 100 + TIME_LIMIT - 1));
        assert!(expired(100, 100 + TIME_LIMIT));
        assert!(expired(100, u64::MAX));
        assert!(!expired(100, 50));
    }

    #[test]
    fn a_session_keeps_its_terminal_and_records_only_how_its_harness_exited() {
        let line = session("claude", "/cache/watch/it's.log", "watch-1");
        assert!(line.starts_with("sh -c "), "{line}");
        assert!(line.contains(SESSION_ENV), "{line}");
        assert!(!line.contains("tee") && !line.ends_with("; exit"), "{line}");
        assert!(line.ends_with(" > '/cache/watch/it'\\''s.log'"), "{line}");

        assert_eq!(exited("started\nchecked #7\n"), None);
        let printed = format!("checked #7\n\nanswered #7\n{EXITED} 0\n");
        assert_eq!(exited(&printed), Some(0));
        assert_eq!(evidence(&printed), ["checked #7", "answered #7"]);
        assert_eq!(exited(&format!("Usage limit reached\n{EXITED} 1\n")), Some(1));
    }

    #[test]
    fn terminal_evidence_is_bounded_to_its_last_lines() {
        let long: String = (0..EVIDENCE_LINES + 5).map(|n| format!("line {n}\n")).collect();
        let did = evidence(&long);
        assert_eq!(did.len(), EVIDENCE_LINES);
        assert_eq!(did[0], "line 5");
        assert_eq!(super::evidence(&"x".repeat(EVIDENCE_CHARS * 2))[0].len(), EVIDENCE_CHARS);
    }
}

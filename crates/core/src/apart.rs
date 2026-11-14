//! Which issues of the frontier would be worked apart from what is in
//! flight: the files an issue names, against the files the issues already
//! claimed name. Parallel work on the same files is what makes a landing
//! candidate chain conflict, so a pass starts the issues apart from it first, where
//! that is known (docs/adr/0076, sunfmin/control#914).
//!
//! An issue names a file in backticks — `crates/core/src/plan.rs`,
//! `docs/adr/`, `GLOSSARY.md` — as issues here do. What names none is
//! known to overlap nothing, and keeps its place.

use std::collections::BTreeSet;

/// The files and directories `body` names in backticks: a word with a `/`
/// in it, or a name with an extension, of the characters a path here is
/// written in. A line number after it (`plan.rs:42`) is dropped; a command,
/// a link, an issue reference (`owner/repo#7`) or a version is no file.
pub fn named_files(body: &str) -> BTreeSet<String> {
    let mut named = BTreeSet::new();
    // Every other part is inside backticks, the first outside; a part is
    // quoted only once a backtick closes it, so the last never is.
    let mut parts = body.split('`');
    parts.next();
    let mut inside = true;
    let mut quoted: Option<&str> = None;
    for part in parts {
        if let Some(closed) = quoted.take()
            && let Some(file) = file_of(closed)
        {
            named.insert(file);
        }
        if inside {
            quoted = Some(part);
        }
        inside = !inside;
    }
    named
}

/// `word` as a file it names, or `None` where it names none.
fn file_of(word: &str) -> Option<String> {
    let word = word.trim();
    let word = match word.rsplit_once(':') {
        Some((path, line)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => path,
        _ => word,
    };
    let word = word.strip_prefix("./").unwrap_or(word);
    if word.is_empty() || word.starts_with('-') || word.contains("://") {
        return None;
    }
    for byte in word.bytes() {
        if !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')) {
            return None;
        }
    }
    let name = word.trim_end_matches('/').rsplit('/').next().unwrap_or(word);
    let extension = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => "",
    };
    let has_extension = extension.bytes().any(|b| b.is_ascii_alphabetic());
    if !word.contains('/') && !has_extension {
        return None;
    }
    Some(word.trim_end_matches('/').to_string())
}

/// Whether `a` and `b` are the same file, or one is a directory the other
/// is in.
fn overlap(a: &str, b: &str) -> bool {
    a == b || within(a, b) || within(b, a)
}

/// Whether `file` is under the directory `dir`.
fn within(file: &str, dir: &str) -> bool {
    file.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

/// Whether any of `files` overlaps any of `in_flight`.
pub fn overlaps(files: &BTreeSet<String>, in_flight: &BTreeSet<String>) -> bool {
    for file in files {
        for other in in_flight {
            if overlap(file, other) {
                return true;
            }
        }
    }
    false
}

/// The issues of `issues` — each with the files it names — whose files
/// overlap those `in_flight` names.
pub fn overlapping(issues: &[(u64, BTreeSet<String>)], in_flight: &BTreeSet<String>) -> Vec<u64> {
    let mut found = vec![];
    for (number, files) in issues {
        if overlaps(files, in_flight) {
            found.push(*number);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(files: &[&str]) -> BTreeSet<String> {
        files.iter().map(|file| file.to_string()).collect()
    }

    #[test]
    fn an_issue_names_the_files_it_quotes() {
        let body = "Change `crates/core/src/plan.rs:42` and `GLOSSARY.md`, beside `docs/adr/`. \
                    Run `afkconvoy gate --fast`, see `sunfmin/control#896`, \
                    since `v0.1.40` and `0.5.76`; `https://x.y/z` is no file, nor `plan`.";
        assert_eq!(named_files(body), set(&["GLOSSARY.md", "crates/core/src/plan.rs", "docs/adr"]),);
        assert!(named_files("no backticks at all").is_empty());
        assert!(named_files("an `unclosed.rs").is_empty(), "never closed: no file");
    }

    #[test]
    fn an_issue_overlaps_what_is_in_flight_by_file_or_directory() {
        let in_flight = set(&["crates/core/src/plan.rs", "docs/adr"]);
        let issues = [
            (3, set(&["crates/core/src/plan.rs"])),
            (4, set(&["docs/adr/0076-x.md"])),
            (5, set(&["crates/core/src/planning.rs"])),
            (6, set(&[])),
            (7, set(&["crates/core"])),
        ];
        assert_eq!(overlapping(&issues, &in_flight), [3, 4, 7]);
    }
}

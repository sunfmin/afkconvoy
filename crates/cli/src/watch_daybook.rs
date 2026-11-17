//! Each round the watch reports through `afkconvoy watched` goes to the
//! **daybook** as one entry, of session kind `watch`: what held work back,
//! what it did, what it verified and what it left, and links to the issues
//! and PRs it touched. Human questions keep going to the asking session
//! (`watch_notify`, `host_asking`); the entry only says they were raised,
//! with their links, and asks nothing.
//!
//! The entry is written to the host's own daybook (docs/adr/0118) when the
//! report is read, at once or not at all: a failure is the watch's to say,
//! and nothing waits to retry it. A report read again writes nothing more.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use afkconvoy_core::config::RepoName;
use anyhow::{Result, bail};
use serde::Deserialize;

use crate::daybook::{Kind, Said};
use crate::git::Repo;
use crate::host_watch::Watch;
use crate::watch_report::Report;

/// Writes the entry of the round `watch` reported as `report` to this host's
/// daybook, once: a report read again writes nothing more.
pub fn reported(watch: &Watch, report: &Report) -> Result<()> {
    write_once(&crate::daybook::today(), &watch.id, &watch.directory, &said(watch, report))
}

/// How the entry of the round `round` begins: what tells it is written.
fn opening(round: &str) -> String {
    format!("Watch round `{round}`.")
}

/// Appends `said`, the entry of the round `round` the watch ran in
/// `directory`, to the day `day` of this host's daybook, where that day holds
/// no entry of the round yet.
fn write_once(day: &str, round: &str, directory: &Path, said: &Said) -> Result<()> {
    let opening = opening(round);
    let read = crate::daybook::read(Some(day), None, None)?;
    let entries = read["entries"].as_array().into_iter().flatten();
    if entries.into_iter().any(|entry| entry["text"].as_str().is_some_and(|t| t.contains(&opening)))
    {
        return Ok(());
    }
    crate::daybook::append(day, crate::clock(), &repo(directory)?, Kind::Watch, said)?;
    Ok(())
}

/// Where an older release queued the entries it had not written yet, under
/// the cache directory.
const OLD_QUEUE: &str = "watch/daybook.json";

/// What an older release's queue holds that is still to be written.
#[derive(Deserialize)]
struct OldQueue {
    #[serde(default)]
    queued: BTreeMap<String, OldEntry>,
}

#[derive(Deserialize)]
struct OldEntry {
    watch: String,
    day: String,
    directory: PathBuf,
    said: Said,
}

/// Writes each entry an older release left queued in `watch/daybook.json`
/// to this host's daybook, once, and moves the file aside once no host
/// runner of an older release holds the host (`host_db::converge_file`).
/// Run again, it changes nothing.
pub fn take_in_old_queue() -> Result<()> {
    let cache = crate::copies::cache()?;
    let file = cache.join(OLD_QUEUE);
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let old: OldQueue = serde_json::from_slice(&bytes)?;
    for entry in old.queued.values() {
        write_once(&entry.day, &entry.watch, &entry.directory, &entry.said)?;
    }
    // Recorded as taken in only as it was written from: one an older runner
    // changed meanwhile is written again first.
    crate::host_db::converge_file(&mut crate::host_db::open()?, &file, &cache, |_, now| {
        if now != bytes.as_slice() {
            bail!("it changed while its entries were written");
        }
        Ok(())
    })
}

/// Every GitHub link to an issue or a PR in `text`, as it is written.
fn links_in(text: &str) -> Vec<String> {
    let mut links = vec![];
    for (at, _) in text.match_indices("https://github.com/") {
        let link: String = text[at..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || "/:._-#".contains(*c))
            .collect();
        let link = link.trim_end_matches(['.', ':', '#']);
        let parts: Vec<&str> = link.trim_start_matches("https://github.com/").split('/').collect();
        if let [_, _, "issues" | "pull", number, ..] = parts.as_slice()
            && number.split('#').next().is_some_and(|n| n.parse::<u64>().is_ok())
        {
            links.push(link.to_string());
        }
    }
    links
}

/// What the entry of the round `watch` reported as `report` says: its
/// actions, decisions, verification and what it left under `Did`, what held
/// work back under `Why`, how to undo each decision under `Undo`, and the
/// links of what it touched. A human question is noted
/// as raised, by its link (else its condition) — never its question or answer.
pub fn said(watch: &Watch, report: &Report) -> Said {
    let list = |items: &[String]| {
        items.iter().map(|item| format!("- {}", item.trim())).collect::<Vec<_>>().join("\n")
    };
    let mut did = opening(&watch.id);
    let mut part = |heading: &str, items: &[String], none: &str| {
        let body = if items.is_empty() { format!("- {none}") } else { list(items) };
        did.push_str(&format!("\n\n**{heading}**\n\n{body}"));
    };
    part("Actions", &report.did, "none");
    let decided: Vec<String> = report
        .decided
        .iter()
        .map(|decided| format!("{} — because {}", decided.chose.trim(), decided.why.trim()))
        .collect();
    if !decided.is_empty() {
        part("Decided by the recommended answer", &decided, "");
    }
    part("Verified", &report.verified, "nothing");
    if !report.pending.is_empty() {
        part("Left pending", &report.pending, "");
    }
    let mut links: Vec<String> = report
        .held_back
        .iter()
        .chain(&report.did)
        .chain(&report.verified)
        .chain(&report.pending)
        .map(String::as_str)
        .chain(report.decided.iter().flat_map(|decided| {
            [decided.chose.as_str(), decided.why.as_str(), decided.undo.as_str()]
        }))
        .flat_map(links_in)
        .collect();
    if !report.human.is_empty() {
        let raised: Vec<String> = report
            .human
            .iter()
            .map(|human| match crate::watch_report::condition(watch, human) {
                Ok(condition) => match crate::watch_report::issue_url(watch, &condition) {
                    // A human wait's condition carries its answer line:
                    // its link alone says what it is.
                    Some(url) => {
                        links.push(url.clone());
                        url
                    }
                    None => format!("`{condition}`"),
                },
                Err(_) => "a question naming no condition of the round".to_string(),
            })
            .collect();
        did.push_str(&format!(
            "\n\n**Raised for the operator** — asked in the asking session, not here\n\n{}",
            list(&raised)
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    links.retain(|link| seen.insert(link.clone()));
    Said {
        did: Some(did),
        why: (!report.held_back.is_empty()).then(|| list(&report.held_back)),
        undo: (!report.decided.is_empty()).then(|| {
            list(
                &report
                    .decided
                    .iter()
                    .map(|decided| format!("{}: {}", decided.chose.trim(), decided.undo.trim()))
                    .collect::<Vec<_>>(),
            )
        }),
        lesson: None,
        links,
    }
}

/// The tracker whose daybook a watch outside any checkout writes to: the
/// one most checkouts on this host's list go by, the first by name of a tie.
fn host_tracker() -> Result<RepoName> {
    let listed = crate::checkout_list::List::of_host()?.read()?.checkouts;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in listed {
        let tracker =
            entry.tracker.as_ref().and_then(afkconvoy_core::tracker::Said::repo).cloned().or_else(
                || {
                    Repo::at(&entry.checkout)
                        .ok()
                        .and_then(|repo| crate::checkout_list::named_repo(&repo).ok())
                },
            );
        if let Some(tracker) = tracker {
            *counts.entry(tracker.as_str().to_string()).or_default() += 1;
        }
    }
    let most = counts.values().copied().max().unwrap_or(0);
    counts
        .into_iter()
        .find(|(_, count)| *count == most && most > 0)
        .and_then(|(name, _)| RepoName::read(&name))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the watch runs in no checkout and this host's checkout list names no tracker: \
                 no daybook to write to"
            )
        })
}

/// The repo of the round the watch ran in `directory`: that of its
/// checkout, as `afkconvoy daybook` run there would say, else the host's
/// tracker.
fn repo(directory: &Path) -> Result<RepoName> {
    match Repo::at(directory) {
        Ok(checkout) => crate::daybook::repo_of(Ok(checkout), None),
        Err(_) => host_tracker(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::watch_report::Human;

    fn watch(look: Value) -> Watch {
        serde_json::from_value(json!({
            "id": "1793620314-1-2", "directory": "/nowhere", "look": look, "items": [],
            "started": 0, "harness": "claude-code", "model": null,
            "terminal": "", "transcript": "/nowhere/t",
        }))
        .unwrap()
    }

    fn report(human: Vec<Human>) -> Report {
        Report {
            held_back: vec!["gate red on https://github.com/acme/dev/pull/12.".into()],
            did: vec!["reran the gate of https://github.com/acme/dev/pull/12".into()],
            verified: vec![
                "https://github.com/acme/tracker/issues/7#issuecomment-1 is green".into(),
            ],
            decided: vec![],
            human,
            pending: vec![],
        }
    }

    fn looked() -> Value {
        json!({"checkouts": [{"checkout": "/c", "doctor": {"waiting_on_human": [{
            "issue": 9, "since": 1, "url": "https://github.com/acme/tracker/issues/9",
            "why": "a ruling", "answer": "afkconvoy rule 9"}]}}]})
    }

    #[test]
    fn an_entry_says_the_rounds_actions_verification_and_links() {
        let said = said(&watch(json!({})), &report(vec![]));
        let did = said.did.clone().unwrap();
        assert!(did.starts_with("Watch round `1793620314-1-2`."), "{did}");
        assert!(did.contains("**Actions**\n\n- reran the gate"), "{did}");
        assert!(
            did.contains("**Verified**\n\n- https://github.com/acme/tracker/issues/7"),
            "{did}"
        );
        assert!(!did.contains("Raised"), "{did}");
        assert_eq!(
            said.why.as_deref().unwrap(),
            "- gate red on https://github.com/acme/dev/pull/12."
        );
        assert_eq!(
            said.links,
            [
                "https://github.com/acme/dev/pull/12",
                "https://github.com/acme/tracker/issues/7#issuecomment-1"
            ]
        );
        let entry = crate::daybook::entry("acme/control", Kind::Watch, &said);
        assert!(entry.starts_with("**Repo** `acme/control` · **Session** watch\n"), "{entry}");
    }

    #[test]
    fn a_human_question_is_linked_and_never_asked() {
        let watch = watch(looked());
        let items = crate::host_watch::actionable(&watch.look);
        assert_eq!(items.len(), 1, "{items:?}");
        let human = Human {
            condition: Some(items[0].id.clone()),
            question: "Merge the release now?".into(),
            answer: "afkconvoy rule 9 --merge".into(),
        };
        let said = said(&watch, &report(vec![human]));
        let did = said.did.as_deref().unwrap();
        assert!(did.contains("**Raised for the operator**"), "{did}");
        assert!(did.contains("https://github.com/acme/tracker/issues/9"), "{did}");
        let entry = crate::daybook::entry("acme/control", Kind::Watch, &said);
        assert!(!entry.contains("Merge the release"), "{entry}");
        assert!(!entry.contains("afkconvoy rule"), "{entry}");
        assert!(!entry.contains('?'), "asks nothing: {entry}");
        assert!(said.links.contains(&"https://github.com/acme/tracker/issues/9".to_string()));
    }

    #[test]
    fn a_decision_by_the_recommended_answer_is_recorded_with_its_undo() {
        let mut report = report(vec![]);
        report.decided = vec![crate::watch_report::Decided {
            chose: "deleted the stale branch of https://github.com/acme/dev/pull/12".into(),
            why: "the recommended answer: nothing takes it up".into(),
            undo: "push the kept copy back: git push origin backup/stale:stale".into(),
        }];
        let said = said(&watch(json!({})), &report);
        let did = said.did.as_deref().unwrap();
        assert!(
            did.contains("**Decided by the recommended answer**\n\n- deleted the stale"),
            "{did}"
        );
        assert!(did.contains("because the recommended answer"), "{did}");
        assert_eq!(
            said.undo.as_deref().unwrap(),
            "- deleted the stale branch of https://github.com/acme/dev/pull/12: push the kept \
             copy back: git push origin backup/stale:stale"
        );
        let entry = crate::daybook::entry("acme/control", Kind::Watch, &said);
        assert!(entry.contains("### Undo\n\n- deleted the stale branch"), "{entry}");
    }

    #[test]
    fn links_are_issues_and_prs_only() {
        assert_eq!(
            links_in(
                "see https://github.com/a/b/pull/3, https://github.com/a/b and https://github.com/a/b/issues/x"
            ),
            ["https://github.com/a/b/pull/3"]
        );
    }

    /// This host's checkout list names one tracker, `acme/tracker`: the
    /// daybook a watch outside any checkout writes to.
    fn a_tracker() {
        let list = json!({"checkouts": [{"checkout": "/c", "tracker": "acme/tracker"}]});
        crate::host_db::put(&crate::checkout_list::STORE, &list).unwrap();
    }

    /// The texts of today's entries of this host's daybook.
    fn written(day: &str) -> Vec<String> {
        let read = crate::daybook::read(Some(day), None, None).unwrap();
        read["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["text"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_reported_round_is_written_at_once_and_once() {
        a_tracker();
        let watch = watch(json!({}));
        reported(&watch, &report(vec![])).unwrap();
        reported(&watch, &report(vec![])).unwrap();
        let entries = written(&crate::daybook::today());
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert!(entries[0].contains("Watch round `1793620314-1-2`."), "{}", entries[0]);
        assert!(entries[0].starts_with("**Repo** `acme/tracker` · **Session** watch"));
    }

    #[test]
    fn what_an_older_release_left_queued_is_written_once_and_its_file_moved_aside() {
        a_tracker();
        let said = |round: &str| json!({"did": format!("Watch round `{round}`."), "links": []});
        // As an older release kept it: `queued` still to be written,
        // `written` already gone to GitHub.
        let old = json!({
            "queued": {
                "1-1-1": {"watch": "1-1-1", "day": "2026-11-01", "directory": "/nowhere",
                    "said": said("1-1-1"), "queued": 1, "failures": 3, "next": 9,
                    "error": "GitHub said no"},
                "2-2-2": {"watch": "2-2-2", "day": "2026-11-02", "directory": "/nowhere",
                    "said": said("2-2-2"), "queued": 2},
            },
            "written": ["0-0-0"],
            "last": null,
        });
        let file = crate::copies::cache().unwrap().join(OLD_QUEUE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, old.to_string()).unwrap();
        // One of them reached this daybook already: it is not written twice.
        write_once(
            "2026-11-01",
            "1-1-1",
            Path::new("/nowhere"),
            &serde_json::from_value(said("1-1-1")).unwrap(),
        )
        .unwrap();

        take_in_old_queue().unwrap();
        take_in_old_queue().unwrap();
        for (day, round) in [("2026-11-01", "1-1-1"), ("2026-11-02", "2-2-2")] {
            let entries = written(day);
            assert_eq!(entries.len(), 1, "{day}: {entries:?}");
            assert!(entries[0].contains(&opening(round)), "{}", entries[0]);
        }
        assert!(!file.exists());
        assert!(file.with_extension("json.migrated").is_file());
    }
}

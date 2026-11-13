//! Each round the watch reports through `afkconvoy watched` goes to the
//! **daybook** as one entry, of session kind `watch`: what held work back,
//! what it did, what it verified and what it left, and links to the issues
//! and PRs it touched. Human questions keep going to the asking session
//! (`watch_notify`, `host_asking`); the entry only says they were raised,
//! with their links, and asks nothing.
//!
//! The round never waits on GitHub. Reading a report queues its entry in a
//! durable outbox beside the watch's reports; a thread of the host runner's
//! own appends it, and retries one that failed, later and later, until it is
//! written. While one is failing, `doctor` names it (`findings`).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use afkconvoy_core::config::RepoName;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::daybook::{Kind, Said, Whose};
use crate::git::Repo;
use crate::host_watch::Watch;
use crate::watch_report::Report;

/// The longest a failed entry waits before it is tried again, in seconds.
pub const RETRY_MAX: u64 = 3600;
/// The first wait after a failure, doubled at each one after it.
const RETRY_FIRST: u64 = 60;
/// How many rounds' ids the outbox keeps once written, so a report read
/// again is not written twice.
const KEPT: usize = 200;

/// One round's entry, queued until it is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Queued {
    pub watch: String,
    /// The host's local date the round reported on: the discussion it goes to.
    pub day: String,
    /// Where the watch ran: the checkout whose repo and tracker it is of,
    /// where it is one.
    pub directory: PathBuf,
    pub said: Said,
    pub queued: u64,
    #[serde(default)]
    pub failures: u64,
    /// Not tried again before this, in epoch seconds.
    #[serde(default)]
    pub next: u64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Outbox {
    queued: BTreeMap<String, Queued>,
    /// Rounds already written, newest last.
    written: VecDeque<String>,
    last: Option<Value>,
}

fn directory() -> Result<PathBuf> {
    let directory = crate::copies::cache()?.join("watch");
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn change<T>(f: impl FnOnce(&mut Outbox) -> T) -> Result<T> {
    let directory = directory()?;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("daybook.lock"))?;
    lock.lock()?;
    let mut outbox = read()?;
    let answer = f(&mut outbox);
    let temporary = directory.join(format!("daybook-{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&outbox)?)?;
    std::fs::rename(temporary, directory.join("daybook.json"))?;
    Ok(answer)
}

fn read() -> Result<Outbox> {
    match std::fs::read(crate::copies::cache()?.join("watch/daybook.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Outbox::default()),
        Err(error) => Err(error.into()),
    }
}

/// Queues the entry of the round `watch` reported as `report`, once: a
/// report read again queues nothing more.
pub fn reported(watch: &Watch, report: &Report) -> Result<()> {
    let queued = Queued {
        watch: watch.id.clone(),
        day: crate::daybook::today(),
        directory: watch.directory.clone(),
        said: said(watch, report),
        queued: crate::clock(),
        failures: 0,
        next: 0,
        error: None,
    };
    change(|outbox| {
        if !outbox.written.contains(&queued.watch) {
            outbox.queued.entry(queued.watch.clone()).or_insert(queued);
        }
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
    let mut did = format!("Watch round `{}`.", watch.id);
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
        let tracker = entry.tracker.or_else(|| {
            Repo::at(&entry.checkout)
                .ok()
                .and_then(|repo| crate::checkout_list::named_repo(&repo).ok())
        });
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

/// Whose daybook the round of `queued` goes to: that of the checkout the
/// watch ran in, as `afkconvoy daybook` run there would say, else the host's.
fn whose(queued: &Queued) -> Result<Whose> {
    match Repo::at(&queued.directory) {
        Ok(checkout) => Whose::of(Ok(checkout), None, None),
        Err(_) => {
            let tracker = host_tracker()?;
            Whose::of(Err(anyhow::anyhow!("no checkout")), Some(tracker.clone()), Some(tracker))
        }
    }
}

/// Writes the entry of `queued` → the comment's URL.
fn write(queued: &Queued) -> Result<String> {
    let answer = whose(queued)?.append(&queued.day, Kind::Watch, &queued.said)?;
    Ok(answer["url"].as_str().unwrap_or_default().to_string())
}

/// The wait after the `failures`th failure in a row.
fn backoff(failures: u64) -> u64 {
    RETRY_FIRST.saturating_mul(1 << failures.saturating_sub(1).min(16)).min(RETRY_MAX)
}

/// Appends the oldest entry due by `now` with `append`, recording what came
/// of it; `true` where one was tried. The attempt is claimed before it is
/// made, so a runner that dies in it tries again only once its wait is over.
fn step(now: u64, append: impl Fn(&Queued) -> Result<String>) -> Result<bool> {
    let due = |outbox: &Outbox| {
        outbox
            .queued
            .values()
            .filter(|queued| queued.next <= now)
            .min_by_key(|queued| (queued.queued, queued.watch.clone()))
            .cloned()
    };
    if due(&read()?).is_none() {
        return Ok(false);
    }
    let Some(queued) = change(|outbox| {
        let queued = due(outbox)?;
        let claimed = outbox.queued.get_mut(&queued.watch)?;
        claimed.next = now + backoff(claimed.failures + 1);
        Some(queued)
    })?
    else {
        return Ok(false);
    };
    let result = append(&queued);
    change(|outbox| match &result {
        Ok(url) => {
            outbox.queued.remove(&queued.watch);
            outbox.written.push_back(queued.watch.clone());
            while outbox.written.len() > KEPT {
                outbox.written.pop_front();
            }
            outbox.last = Some(json!({"watch": queued.watch, "url": url, "at": crate::clock()}));
        }
        Err(error) => {
            if let Some(failed) = outbox.queued.get_mut(&queued.watch) {
                failed.failures = failed.failures.saturating_add(1);
                failed.next = now + backoff(failed.failures);
                failed.error = Some(format!("{error:#}"));
            }
        }
    })?;
    Ok(true)
}

/// Writes queued entries until `ended`, saying each failure to `say`.
pub fn run(ended: &AtomicBool, say: impl Fn(String)) {
    while !ended.load(Ordering::Relaxed) {
        match step(crate::clock(), write) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => say(format!("daybook outbox failed: {error:#}")),
        }
        for _ in 0..20 {
            if ended.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// What `doctor.host.daybook` shows: the entries not yet written and the last
/// one that was.
pub fn json() -> Value {
    match read() {
        Ok(outbox) => json!({
            "queued": outbox.queued.values().collect::<Vec<_>>(),
            "last": outbox.last,
        }),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

/// One finding for each round whose entry failed to be written.
pub fn findings() -> Vec<crate::doctor::Finding> {
    let unwritten = |identity: Value, evidence: String| {
        crate::doctor::finding(
            crate::doctor::Kind::DaybookUnwritten,
            identity,
            "a watch round's daybook entry is not written".into(),
            evidence,
            "fix what its error says (a missing `Daybook` category is created at \
             https://github.com/<tracker>/discussions/categories/new); the host runner retries \
             it by itself"
                .into(),
        )
    };
    match read() {
        Ok(outbox) => outbox
            .queued
            .values()
            .filter(|queued| queued.failures > 0)
            .map(|queued| {
                unwritten(
                    json!(["daybook", queued.watch]),
                    format!(
                        "round {} of {}: {} failure(s), next try at {}; last error: {}",
                        queued.watch,
                        queued.day,
                        queued.failures,
                        queued.next,
                        queued.error.as_deref().unwrap_or("none")
                    ),
                )
            })
            .collect(),
        Err(error) => vec![unwritten(
            json!(["daybook-cache"]),
            format!("its outbox cannot be read: {error:#}"),
        )],
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn each_reported_round_is_queued_once_and_a_failure_is_retried_later() {
        let watch = watch(json!({}));
        reported(&watch, &report(vec![])).unwrap();
        reported(&watch, &report(vec![])).unwrap();
        assert_eq!(read().unwrap().queued.len(), 1);
        assert!(findings().is_empty());

        let failed = step(1000, |_| anyhow::bail!("no `Daybook` category")).unwrap();
        assert!(failed);
        let queued = read().unwrap().queued[&watch.id].clone();
        assert_eq!((queued.failures, queued.next), (1, 1000 + RETRY_FIRST));
        let found = findings();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, crate::doctor::Kind::DaybookUnwritten);
        assert!(found[0].evidence.contains("no `Daybook` category"), "{}", found[0].evidence);

        // not due yet: nothing is tried
        assert!(!step(1001, |_| panic!("tried before its wait")).unwrap());
        assert!(step(1000 + RETRY_FIRST, |_| anyhow::bail!("again")).unwrap());
        assert_eq!(read().unwrap().queued[&watch.id].next, 1000 + RETRY_FIRST + 2 * RETRY_FIRST);

        assert!(
            step(10_000, |_| Ok("https://github.com/acme/control/discussions/1#c".into())).unwrap()
        );
        let outbox = read().unwrap();
        assert!(outbox.queued.is_empty());
        assert!(findings().is_empty());
        // the same report read again is not written twice
        reported(&watch, &report(vec![])).unwrap();
        assert!(read().unwrap().queued.is_empty());
    }

    #[test]
    fn the_wait_doubles_up_to_its_bound() {
        assert_eq!(backoff(1), RETRY_FIRST);
        assert_eq!(backoff(2), 2 * RETRY_FIRST);
        assert_eq!(backoff(40), RETRY_MAX);
    }
}

//! Tracker-wide work nobody has taken. Doctor only reads; the host runner
//! reconciles reports under a leased tracker ref, then observes the finding.
//! A report's opening host remains its watch owner until recovery, even
//! after a restart. No local observation time or checkout list is evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use afkconvoy_core::claims::{Claim, read_heartbeat};
use afkconvoy_core::config::RepoName;
use afkconvoy_core::layout::Layout;
use afkconvoy_core::lease::stamp_stale;
use afkconvoy_core::tracker::{Tracker, repo_labels};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::checkout_list::{List, named_repo};
use crate::doctor::{Finding, Kind, finding};
use crate::gh::GitHub;
use crate::git::Repo;
use crate::remote::{self, Push};

const MARKER: &str = "<!--convoy:unworked ";
const CLAIM: &str = "convoy:unworked-claim ";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Waiting {
    issue: u64,
    seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Report {
    repo: String,
    host: String,
    waiting: Vec<Waiting>,
}

impl Report {
    fn read(body: &str) -> Option<Self> {
        serde_json::from_str(body.lines().next()?.strip_prefix(MARKER)?.strip_suffix("-->")?).ok()
    }
}

pub(crate) fn is_report(body: &str) -> bool {
    Report::read(body).is_some()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OpenReport {
    issue: u64,
    report: Report,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Beat {
    host: String,
    at: u64,
}

/// Everything needed to render a finding or decide a report transition.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Observed {
    checkout: PathBuf,
    tracker: String,
    repo: String,
    escalate: String,
    waiting: Vec<Waiting>,
    reports: Vec<OpenReport>,
    newest: Option<Beat>,
    /// The host whose heartbeat proves recovery, or whose claims cover all
    /// issues named in a report. An unknown claim owner is not a recovery.
    recovered: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Issue {
    number: u64,
    body: String,
    labels: Vec<String>,
    created: u64,
}

fn time(value: &Value) -> Option<u64> {
    value.as_str().and_then(|s| crate::doctor::watching::since(s).ok())
}

fn rows(gh: &GitHub, endpoint: &str, jq: &str) -> Result<Vec<Value>> {
    let out = gh.read(&["api", "--paginate", endpoint, "--jq", jq])?;
    out.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

/// One additional paged query per tracker, including existing reports.
fn issues(gh: &GitHub, tracker: &str) -> Result<Vec<Issue>> {
    rows(gh, &format!("repos/{tracker}/issues?state=open&per_page=100"),
        ".[] | select(.pull_request == null) | {number, body: (.body // \"\"), labels: [.labels[].name], created: (.created_at | fromdateiso8601)}")?
        .into_iter().map(|v| Ok(serde_json::from_value(v)?)).collect()
}

fn private(gh: &GitHub, tracker: &str) -> Result<()> {
    let named = RepoName::read(tracker).context("invalid tracker")?;
    let reader =
        GitHub::new(gh.dir.clone(), Tracker::Other { tracker: named.clone(), repo: named });
    let visibility = reader.visibility()?;
    ensure!(
        visibility.trim() == "private",
        "tracker {tracker} is not private; no report may be written"
    );
    Ok(())
}

fn subject(gh: &GitHub, repo: &str, sha: &str) -> Result<String> {
    // Share a SHA's subject across trackers in this look; failed reads can
    // recover at the next look.
    static SUBJECTS: crate::look::Reading<Result<String, String>> =
        crate::look::Reading::per_look("foreign record subject");
    SUBJECTS
        .read(&format!("{repo}/{sha}"), || {
            gh.read(&["api", &format!("repos/{repo}/git/commits/{sha}"), "--jq", ".message"])
                .map_err(|e| format!("{e:#}"))
        })
        .map_err(anyhow::Error::msg)
}

fn url(repo: &str) -> String {
    format!("https://github.com/{repo}.git")
}

fn ready_since(
    gh: &GitHub,
    tracker: &str,
    issue: &Issue,
    labels: &[String],
) -> Result<Option<u64>> {
    let timeline =
        rows(gh, &format!("repos/{tracker}/issues/{}/timeline?per_page=100", issue.number), ".[]")?;
    // Only labels still present matter. Reapplication replaces the earlier
    // time; with several dispatch labels use the first currently ready one.
    Ok(labels
        .iter()
        .filter_map(|label| {
            timeline
                .iter()
                .rev()
                .find(|e| e["event"] == "labeled" && e["label"]["name"] == *label)
                .and_then(|e| time(&e["created_at"]))
        })
        .min())
}

fn blockers(gh: &GitHub, tracker: &str, issue: &Issue) -> Result<Vec<Option<u64>>> {
    let mut edges = rows(
        gh,
        &format!("repos/{tracker}/issues/{}/dependencies/blocked_by?per_page=100", issue.number),
        ".[]",
    );
    // A tracker without native dependencies may use the documented fallback.
    if edges.as_ref().is_err_and(|e| e.to_string().contains("HTTP 404")) {
        edges = Ok(vec![]);
    }
    let mut times = edges?
        .iter()
        .map(|e| if e["state"] == "closed" { time(&e["closed_at"]) } else { None })
        .collect::<Vec<_>>();
    for line in issue.body.lines().filter_map(|line| line.trim().strip_prefix("Blocked by:")) {
        for number in line
            .split([',', ' '])
            .filter_map(|word| word.trim().strip_prefix('#')?.parse::<u64>().ok())
        {
            let out = gh.read(&["api", &format!("repos/{tracker}/issues/{number}")])?;
            let edge: Value = serde_json::from_str(&out)?;
            times.push(if edge["state"] == "closed" { time(&edge["closed_at"]) } else { None });
        }
    }
    Ok(times)
}

fn observe(
    checkout: PathBuf,
    gh: &GitHub,
    tracker: &str,
    repo: &str,
    open: &[Issue],
    now: u64,
) -> Observed {
    let mut seen = Observed {
        checkout,
        tracker: tracker.into(),
        repo: repo.into(),
        escalate: String::new(),
        waiting: vec![],
        reports: open
            .iter()
            .filter_map(|i| {
                let report = Report::read(&i.body)?;
                report
                    .repo
                    .eq_ignore_ascii_case(repo)
                    .then_some(OpenReport { issue: i.number, report })
            })
            .collect(),
        newest: None,
        recovered: None,
        error: None,
    };
    if let Err(error) = read_repo(&mut seen, gh, open, now) {
        seen.error = Some(format!("{error:#}"));
    }
    seen
}

fn read_repo(seen: &mut Observed, gh: &GitHub, open: &[Issue], now: u64) -> Result<()> {
    let named = RepoName::read(&seen.repo).context("invalid repo label")?;
    let text = gh
        .file_of(&named, crate::config::FILE)?
        .context("repo has no fleet config; its dispatch labels are unknown")?;
    let config = crate::config::parse(&text)?;
    seen.escalate = config.escalate_label.clone();
    let flows = if let Some(bound) = &config.bound {
        let copies = crate::copies::Copies::of_host()?;
        let kept = crate::team::keep(gh, &copies, &bound.team, &config.labels())?;
        seen.escalate = kept.team.escalate_label.clone();
        Some(kept.team.flow_labels())
    } else {
        None
    };
    let mut waiting = vec![];
    for issue in open {
        let owned = repo_labels(&issue.labels);
        if owned.len() != 1
            || !owned[0].eq_ignore_ascii_case(&format!("repo:{}", seen.repo))
            || Report::read(&issue.body).is_some()
            || issue.labels.contains(&seen.escalate)
        {
            continue;
        }
        let labels = match &flows {
            Some(flows) => {
                issue.labels.iter().filter(|label| flows.takes(label)).cloned().collect()
            }
            None => issue
                .labels
                .iter()
                .filter(|l| **l == config.ready_label)
                .cloned()
                .collect::<Vec<_>>(),
        };
        if labels.is_empty() || now.saturating_sub(issue.created) <= afkconvoy_core::unworked::WAIT
        {
            continue;
        }
        let ready = ready_since(gh, &seen.tracker, issue, &labels)?;
        if ready.is_none_or(|at| now.saturating_sub(at) <= afkconvoy_core::unworked::WAIT) {
            continue;
        }
        let blockers = blockers(gh, &seen.tracker, issue)?;
        if let Some(seconds) = afkconvoy_core::unworked::waited(ready, &blockers, false, now) {
            waiting.push(Waiting { issue: issue.number, seconds });
        }
    }
    // Young or blocked work alone never asks a repo's heartbeat remote.
    if waiting.is_empty() && seen.reports.is_empty() {
        return Ok(());
    }
    let refs = remote::foreign_work_refs(&seen.checkout, &url(&seen.repo))
        .context("repo's remote could not be read")?;
    let mut claims = BTreeMap::new();
    let tracker = if seen.tracker.eq_ignore_ascii_case(&seen.repo) {
        Tracker::Own
    } else {
        Tracker::Other {
            tracker: RepoName::read(&seen.tracker).context("invalid tracker")?,
            repo: named,
        }
    };
    for (name, sha) in refs {
        let Some(kept) = Layout::ALL.into_iter().find_map(|layout| layout.kept(&name)) else {
            continue;
        };
        if kept.starts_with("heartbeat/") {
            let message = subject(gh, &seen.repo, &sha)?;
            let beat = read_heartbeat(&message, sha).context("unreadable heartbeat record")?;
            if seen.newest.as_ref().is_none_or(|newest| beat.ts > newest.at) {
                seen.newest = Some(Beat { host: beat.host.clone(), at: beat.ts });
            }
            if !stamp_stale(beat.ts, now) {
                seen.recovered = Some(beat.host);
            }
        } else if let Some(key) = kept.strip_prefix("claim/")
            && let afkconvoy_core::tracker::Claimed::Issue(number) = tracker.claimed(key)
        {
            // Any claim excludes the issue, even if its subject is malformed.
            let message = subject(gh, &seen.repo, &sha)?;
            let claim = Claim::read(number, &message, sha);
            claims.insert(number, claim.owner.map(|owner| owner.host));
        }
    }
    waiting.retain(|wait| !claims.contains_key(&wait.issue));
    seen.waiting = waiting;
    if seen.recovered.is_none() && !seen.reports.is_empty() {
        let reported: BTreeSet<_> =
            seen.reports.iter().flat_map(|r| r.report.waiting.iter().map(|w| w.issue)).collect();
        if reported.iter().all(|n| claims.get(n).is_some_and(Option::is_some)) {
            seen.recovered = Some(
                reported
                    .iter()
                    .filter_map(|n| claims.get(n)?.as_deref())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }
    Ok(())
}

/// Read every tracker named by the checkout list once, including repos that
/// have never been on this host. Errors remain evidence, never an empty remote.
pub(crate) fn read(now: u64) -> Result<Value> {
    let listed = List::of_host()?.read()?.checkouts;
    let mut trackers = BTreeMap::new();
    for entry in listed {
        let Ok(repo) = Repo::at(&entry.checkout) else { continue };
        let tracker = entry.tracker.or_else(|| named_repo(&repo).ok());
        if let Some(tracker) = tracker {
            trackers.entry(tracker.to_string().to_lowercase()).or_insert(repo.dir);
        }
    }
    let mut found = vec![];
    for (tracker, checkout) in trackers {
        let gh = GitHub::new(checkout.clone(), Tracker::Own);
        let open = private(&gh, &tracker).and_then(|()| issues(&gh, &tracker));
        let open = match open {
            Ok(open) => open,
            Err(error) => {
                found.push(Observed {
                    checkout,
                    repo: tracker.clone(),
                    tracker,
                    escalate: String::new(),
                    waiting: vec![],
                    reports: vec![],
                    newest: None,
                    recovered: None,
                    error: Some(format!("{error:#}")),
                });
                continue;
            }
        };
        let repos: BTreeSet<String> = open
            .iter()
            .filter_map(|issue| {
                let labels = repo_labels(&issue.labels);
                if labels.len() == 1 {
                    Some(labels[0][5..].to_lowercase())
                } else {
                    Report::read(&issue.body).map(|r| r.repo)
                }
            })
            .collect();
        for repo in repos {
            found.push(observe(checkout.clone(), &gh, &tracker, &repo, &open, now));
        }
    }
    Ok(serde_json::to_value(found)?)
}

fn finding_of(seen: &Observed, host: &str, now: u64) -> Option<Finding> {
    let identity = json!(["unworked-repo", seen.tracker, seen.repo]);
    if let Some(error) = &seen.error {
        return Some(finding(
            Kind::RepoUnreadable,
            identity,
            format!("{} readiness or remote could not be read", seen.repo),
            error.clone(),
            format!("gh repo view {}", seen.repo),
        ));
    }
    let report = seen.reports.iter().find(|r| r.report.host == host)?;
    Some(finding(
        Kind::RepoUnworked,
        identity,
        format!("no fleet has taken the waiting issues of {}", seen.repo),
        format!(
            "{}/issues/{}; {}; waiting: {}",
            format_args!("https://github.com/{}", seen.tracker),
            report.issue,
            heartbeat(seen, now),
            serde_json::to_string(&report.report.waiting).unwrap_or_default()
        ),
        format!("gh issue view {} --repo {}", report.issue, seen.tracker),
    ))
}

fn heartbeat(seen: &Observed, now: u64) -> String {
    seen.newest.as_ref().map_or_else(
        || "no heartbeat on the repo's remote".into(),
        |b| format!("newest heartbeat: host {}, age {} seconds", b.host, now.saturating_sub(b.at)),
    )
}

pub(crate) fn findings(value: &Value, host: &str, now: u64) -> Vec<Finding> {
    if let Some(error) = value.get("error") {
        return vec![finding(
            Kind::RepoUnreadable,
            json!(["unworked-trackers"]),
            "tracker readiness or remote could not be read".into(),
            error.to_string(),
            "afkconvoy doctor --all".into(),
        )];
    }
    serde_json::from_value::<Vec<Observed>>(value.clone())
        .unwrap_or_default()
        .iter()
        .filter_map(|seen| finding_of(seen, host, now))
        .collect()
}

/// A human notification links to the report even though its repo is absent
/// from the checkout list. The condition is the same one the finding owns.
pub(crate) fn issue_url(value: &Value, condition: &str) -> Option<String> {
    let rows: Vec<Observed> = serde_json::from_value(value.clone()).ok()?;
    for seen in rows {
        let Some(report) = seen.reports.first() else { continue };
        let Some(finding) = finding_of(&seen, &report.report.host, 0) else { continue };
        if crate::host_watch::scoped_finding_id("host", &finding.watch_id) == condition {
            return Some(format!("https://github.com/{}/issues/{}", seen.tracker, report.issue));
        }
    }
    None
}

trait Reports {
    fn refresh(&self, seen: &Observed, now: u64) -> Result<Observed>;
    fn take(&self, seen: &Observed, host: &str, now: u64) -> Result<Option<String>>;
    fn release(&self, seen: &Observed, claim: &str) -> Result<()>;
    fn open(&self, seen: &Observed, report: &Report, now: u64) -> Result<u64>;
    fn close(&self, seen: &Observed, issue: u64, host: &str) -> Result<()>;
}

/// Recheck inside the atomic claim, including the open-report lookup. An
/// interrupted write is recovered by that lookup before any further create.
fn settle(backend: &impl Reports, seen: &Observed, host: &str, now: u64) -> Result<Observed> {
    if seen.error.is_some()
        || (seen.reports.is_empty() && (seen.waiting.is_empty() || seen.recovered.is_some()))
        || (!seen.reports.is_empty() && seen.recovered.is_none())
    {
        return Ok(seen.clone());
    }
    let Some(claim) = backend.take(seen, host, now)? else { return Ok(seen.clone()) };
    let done = (|| {
        let mut fresh = backend.refresh(seen, now)?;
        if fresh.error.is_some() {
            return Ok(fresh);
        }
        if let Some(host) = fresh.recovered.clone() {
            for report in &fresh.reports {
                backend.close(&fresh, report.issue, &host)?;
            }
            fresh.reports.clear();
        } else if fresh.reports.is_empty() && !fresh.waiting.is_empty() {
            let report = Report {
                repo: fresh.repo.clone(),
                host: host.into(),
                waiting: fresh.waiting.clone(),
            };
            let issue = backend.open(&fresh, &report, now)?;
            fresh.reports.push(OpenReport { issue, report });
        }
        Ok(fresh)
    })();
    // Leave the claim on uncertain failures: a later lease takeover must
    // inspect GitHub before retrying. A successful close/create releases it.
    if done.is_ok() {
        backend.release(seen, &claim)?;
    }
    done
}

struct GitReports;

impl GitReports {
    fn refname(seen: &Observed) -> String {
        format!("refs/heads/convoy-unworked/{}", seen.repo)
    }
}

impl Reports for GitReports {
    fn refresh(&self, seen: &Observed, now: u64) -> Result<Observed> {
        std::sync::Arc::new(crate::look::Look::new()).enter(|| {
            let gh = GitHub::new(seen.checkout.clone(), Tracker::Own);
            private(&gh, &seen.tracker)?;
            let open = issues(&gh, &seen.tracker)?;
            Ok(observe(seen.checkout.clone(), &gh, &seen.tracker, &seen.repo, &open, now))
        })
    }

    fn take(&self, seen: &Observed, host: &str, now: u64) -> Result<Option<String>> {
        let gh = GitHub::new(seen.checkout.clone(), Tracker::Own);
        let name = Self::refname(seen);
        let refs = remote::foreign_refs(&seen.checkout, &url(&seen.tracker), &[&name])?;
        let expected = if let Some((_, sha)) = refs.first() {
            let message = subject(&gh, &seen.tracker, sha)?;
            let record: Value = serde_json::from_str(
                message.trim().strip_prefix(CLAIM).context("invalid report claim")?,
            )?;
            let at = record["at"].as_u64().context("report claim has no time")?;
            if !stamp_stale(at, now) {
                return Ok(None);
            }
            sha.as_str()
        } else {
            ""
        };
        let repo = Repo::at(&seen.checkout)?;
        let sha = repo.record_commit(&format!(
            "{CLAIM}{}",
            json!({"host": host, "at": now, "id": crate::unique()})
        ))?;
        Ok(remote::foreign_push(
            &seen.checkout,
            &url(&seen.tracker),
            &Push::set(&sha, &name).leased(expected),
        )?
        .then_some(sha))
    }

    fn release(&self, seen: &Observed, claim: &str) -> Result<()> {
        remote::foreign_push(
            &seen.checkout,
            &url(&seen.tracker),
            &Push::delete(&Self::refname(seen)).leased(claim),
        )?;
        Ok(())
    }

    fn open(&self, seen: &Observed, report: &Report, now: u64) -> Result<u64> {
        let tracker = RepoName::read(&seen.tracker).context("invalid tracker")?;
        let repo = RepoName::read(&seen.repo).context("invalid repo")?;
        let gh = GitHub::new(seen.checkout.clone(), Tracker::Other { tracker, repo });
        let waits = report
            .waiting
            .iter()
            .map(|w| format!("- {}#{}: {} seconds", seen.tracker, w.issue, w.seconds))
            .collect::<Vec<_>>()
            .join("\n");
        let body = format!(
            "{MARKER}{}-->\n\nNo fleet has taken these ready issues of {} for more than an hour:\n\n{waits}\n\n{}.\n\nWhich host works this repo is the operator's decision. The watch must not add it to its host's checkout list. Opening host: {}. This report closes when claims or a live heartbeat show the work taken.",
            serde_json::to_string(report)?,
            seen.repo,
            heartbeat(seen, now),
            report.host
        );
        // An untouched repo may not yet have created its escalate label.
        crate::flows::create_missing(
            &gh,
            std::slice::from_ref(&seen.escalate),
            "afkconvoy: the fleet handed an issue that carries this to a human",
        )?;
        gh.open_issue(
            &format!("afkconvoy: no fleet has taken the ready issues of {}", seen.repo),
            &body,
            &[&seen.escalate],
        )
    }

    fn close(&self, seen: &Observed, issue: u64, host: &str) -> Result<()> {
        let gh = GitHub::new(
            seen.checkout.clone(),
            Tracker::Other {
                tracker: RepoName::read(&seen.tracker).context("invalid tracker")?,
                repo: RepoName::read(&seen.repo).context("invalid repo")?,
            },
        );
        let marker = "<!--convoy:unworked-taken-->";
        if !gh.comments(issue)?.iter().any(|comment| comment.body.starts_with(marker)) {
            gh.comment(issue, &format!("{marker}\nThe work was taken by host {host}: its claims or live heartbeat are on the repo's remote."))?;
        }
        gh.close_issue(issue)
    }
}

/// The only writer, called by the host runner after its read-only doctor
/// child returns. Persist the opening host's finding on every later look so
/// watch episode coverage and a human notification survive quiet looks.
pub(crate) fn reconcile(look: &mut Value, host: &str, now: u64) -> Result<()> {
    if !look["host"]["unworked_repos"].is_array() {
        return Ok(());
    }
    let rows: Vec<Observed> = serde_json::from_value(look["host"]["unworked_repos"].clone())?;
    let mut settled = vec![];
    for row in rows {
        settled.push(settle(&GitReports, &row, host, now)?);
    }
    look["host"]["unworked_repos"] = serde_json::to_value(settled)?;
    let mut found: Vec<Value> = look["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f["kind"] != "repo_unworked" && f["kind"] != "repo_unreadable")
        .collect();
    found.extend(
        findings(&look["host"]["unworked_repos"], host, now)
            .iter()
            .map(|f| serde_json::to_value(f).expect("finding")),
    );
    look["findings"] = json!(found);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Memory {
        seen: Mutex<Observed>,
        claim: Mutex<Option<String>>,
        opened: Mutex<usize>,
        closed: Mutex<Vec<(u64, String)>>,
        uncertain: bool,
    }

    fn candidate() -> Observed {
        Observed {
            checkout: "/listed".into(),
            tracker: "acme/control".into(),
            repo: "acme/remote".into(),
            escalate: "ready-for-human".into(),
            waiting: vec![Waiting { issue: 7, seconds: 3660 }],
            reports: vec![],
            newest: None,
            recovered: None,
            error: None,
        }
    }

    impl Memory {
        fn new() -> Self {
            Self {
                seen: Mutex::new(candidate()),
                claim: Mutex::new(None),
                opened: Mutex::new(0),
                closed: Mutex::new(vec![]),
                uncertain: false,
            }
        }
    }

    impl Reports for Memory {
        fn refresh(&self, _: &Observed, _: u64) -> Result<Observed> {
            Ok(self.seen.lock().unwrap().clone())
        }
        fn take(&self, _: &Observed, host: &str, _: u64) -> Result<Option<String>> {
            let mut held = self.claim.lock().unwrap();
            if held.is_some() {
                return Ok(None);
            }
            *held = Some(host.into());
            Ok(held.clone())
        }
        fn release(&self, _: &Observed, claim: &str) -> Result<()> {
            let mut held = self.claim.lock().unwrap();
            assert_eq!(held.as_deref(), Some(claim));
            *held = None;
            Ok(())
        }
        fn open(&self, _: &Observed, report: &Report, _: u64) -> Result<u64> {
            *self.opened.lock().unwrap() += 1;
            self.seen
                .lock()
                .unwrap()
                .reports
                .push(OpenReport { issue: 19, report: report.clone() });
            if self.uncertain {
                anyhow::bail!("lost create receipt");
            }
            Ok(19)
        }
        fn close(&self, _: &Observed, issue: u64, host: &str) -> Result<()> {
            self.closed.lock().unwrap().push((issue, host.into()));
            self.seen.lock().unwrap().reports.retain(|r| r.issue != issue);
            Ok(())
        }
    }

    #[test]
    fn two_hosts_with_the_same_old_look_open_one_report_and_only_its_owner_has_a_finding() {
        let backend = Memory::new();
        let stale = candidate();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| settle(&backend, &stale, "first", 10_000).unwrap());
            let second = scope.spawn(|| settle(&backend, &stale, "second", 10_000).unwrap());
            first.join().unwrap();
            second.join().unwrap();
        });
        let seen = backend.seen.lock().unwrap().clone();
        assert_eq!(*backend.opened.lock().unwrap(), 1);
        let owner = &seen.reports[0].report.host;
        let peer = if owner == "first" { "second" } else { "first" };
        let own = finding_of(&seen, owner, 10_000).unwrap();
        assert!(
            own.evidence.contains("7")
                && own.evidence.contains("3660")
                && own.evidence.contains("no heartbeat")
        );
        assert!(finding_of(&seen, peer, 10_000).is_none());
        // Serializing a restart's observation does not create a new report
        // or lose its owner's condition identity as its displayed age grows.
        let restarted: Observed =
            serde_json::from_value(serde_json::to_value(&seen).unwrap()).unwrap();
        let later = settle(&backend, &restarted, peer, 10_300).unwrap();
        assert_eq!(finding_of(&later, owner, 10_300).unwrap().watch_id, own.watch_id);
        assert_eq!(*backend.opened.lock().unwrap(), 1);
    }

    #[test]
    fn an_interrupted_create_is_found_before_a_later_host_can_create_again() {
        let mut backend = Memory::new();
        backend.uncertain = true;
        let stale = candidate();
        assert!(settle(&backend, &stale, "first", 10_000).is_err());
        assert!(backend.claim.lock().unwrap().is_some(), "uncertain write keeps its claim");
        // The remote lease has expired. The subsequent look reads GitHub,
        // including a report whose successful create lost its receipt.
        *backend.claim.lock().unwrap() = None;
        let later = settle(&backend, &stale, "second", 12_000).unwrap();
        assert_eq!(later.reports.len(), 1);
        assert_eq!(*backend.opened.lock().unwrap(), 1);
        assert!(finding_of(&later, "second", 12_000).is_none());
        assert!(finding_of(&later, "first", 12_000).is_some());
    }

    #[test]
    fn recovery_on_another_host_closes_the_report_and_names_the_host_that_took_it() {
        let backend = Memory::new();
        settle(&backend, &candidate(), "first", 10_000).unwrap();
        let mut recovered = backend.seen.lock().unwrap().clone();
        recovered.recovered = Some("taking-host".into());
        *backend.seen.lock().unwrap() = recovered.clone();
        let closed = settle(&backend, &recovered, "second", 10_300).unwrap();
        assert!(closed.reports.is_empty());
        assert!(finding_of(&closed, "first", 10_300).is_none());
        assert_eq!(*backend.closed.lock().unwrap(), [(19, "taking-host".into())]);
    }

    #[test]
    fn a_remote_error_or_a_live_heartbeat_never_opens_a_report() {
        let backend = Memory::new();
        let mut seen = candidate();
        seen.error = Some("repo's remote could not be read: denied".into());
        settle(&backend, &seen, "first", 10_000).unwrap();
        let finding = finding_of(&seen, "first", 10_000).unwrap();
        assert!(finding.what.contains("remote could not be read"));
        assert!(!finding.what.contains("no fleet has taken"));
        seen.error = None;
        seen.recovered = Some("busy-host".into());
        seen.newest = Some(Beat { host: "busy-host".into(), at: 10_000 - 29 * 60 });
        settle(&backend, &seen, "first", 10_000).unwrap();
        assert_eq!(*backend.opened.lock().unwrap(), 0);
    }
}

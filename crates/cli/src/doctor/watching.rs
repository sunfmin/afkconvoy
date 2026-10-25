//! A watching agent's interval: local steps from the retained tick log,
//! supplemented by GitHub records and endings. Boundaries are [since, until)
//! at the sources' one-second precision; the current second is never consumed.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};

use crate::gh::{Comment, GitHub};
use crate::git::Repo;
use crate::log::{self, Entry, Log, What};

/// A boundary accepted by `--since`. Restrict epoch values to the same civil
/// range as the UTC spelling, including validation of days and leap years.
pub fn since(raw: &str) -> Result<u64, String> {
    const LAST: u64 = 253_402_300_799;
    let bad = || "use epoch seconds or UTC YYYY-MM-DDTHH:MM:SSZ for --since".to_string();
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        return raw.parse::<u64>().ok().filter(|&at| at <= LAST).ok_or_else(bad);
    }
    if raw.len() != 20 || !raw.is_ascii() {
        return Err(bad());
    }
    let n = |start, end| raw.get(start..end).and_then(|s| s.parse::<i64>().ok());
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) =
        (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19))
    else {
        return Err(bad());
    };
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return Err(bad());
    }
    // Howard Hinnant's days_from_civil, inverse of log::utc.
    let year = year - i64::from(month <= 2);
    let era = year / 400;
    let of_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let days =
        era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + (153 * month + 2) / 5 + day
            - 1
            - 719_468;
    let at =
        u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).map_err(|_| bad())?;
    if format!("{}Z", log::utc(at).replace(' ', "T")) != raw {
        return Err(bad());
    }
    Ok(at)
}

#[derive(Debug, Clone, Serialize)]
struct Step {
    at: u64,
    source: &'static str,
    step: String,
    detail: Value,
}

#[derive(Default)]
struct Issue {
    title: Option<String>,
    pr: Option<u64>,
    steps: Vec<Step>,
}

#[derive(Serialize)]
struct Ticks {
    instance: String,
    first: u64,
    last: u64,
    count: u64,
    line: String,
    failed: bool,
    did: Value,
    errors: Value,
}

/// An issue is identified by the subject a line names, never a number buried
/// in its prose (a gate's queue count, or an inspection's PR, for example).
fn issue(on: &str) -> Option<u64> {
    on.split_whitespace().next()?.strip_prefix('#')?.parse().ok()
}

fn step(at: u64, name: &str, detail: Value) -> Step {
    Step { at, source: "tick_log", step: name.to_string(), detail }
}

fn record(comment: &Comment) -> Option<Step> {
    use afkconvoy_core::record::read_marker;
    let at = comment.at?;
    for (marker, name) in [
        ("convoy:stage", "stage_record"),
        ("convoy:verdict", "verdict"),
        ("convoy:intake", "intake"),
        ("convoy:inspected", "inspected"),
    ] {
        if read_marker(marker, &comment.body).is_some() {
            return Some(Step {
                at,
                source: "github",
                step: name.to_string(),
                detail: json!({"id": comment.id, "url": comment.url, "body": comment.body}),
            });
        }
    }
    None
}

/// Read both sources before making an interval. A failed source is an error,
/// not an empty interval: `doctor --all` then withholds its next boundary.
pub(super) fn read(repo: &Repo, host: &str, since: u64, until: u64) -> Result<Value> {
    let log = Log::of(repo)?;
    let entries = log.read().unwrap_or_default();
    let gh = GitHub::of(repo)?;
    let open = gh.open_prs()?;
    let recent = gh.watching_issues(since)?;
    let prs = gh.watching_prs(since)?;
    let mut issues: BTreeMap<u64, Issue> = BTreeMap::new();
    let mut read_comments = BTreeSet::new();
    let mut ended = vec![];
    for row in recent {
        let found = issues.entry(row.number).or_default();
        found.title = Some(row.title);
        read_comments.insert(row.number);
        found.steps.extend(row.comments.iter().filter_map(record));
        if let Some(at) = row.closed {
            let closed = Step {
                at,
                source: "github",
                step: "issue_closed".into(),
                detail: json!({"issue": row.number}),
            };
            ended.push(json!({"at": at, "issue": row.number, "pr": null,
                "step": closed.step, "detail": closed.detail}));
            found.steps.push(closed);
        }
    }
    for pr in open {
        for number in pr.closes {
            let found = issues.entry(number).or_default();
            found.pr = Some(pr.number);
            // The last known step must still be available when the issue's
            // last GitHub update predates this interval.
            if read_comments.insert(number) {
                found.title = Some(gh.issue(number)?.title);
                found.steps.extend(gh.comments(number)?.iter().filter_map(record));
            }
        }
    }
    for pr in prs {
        let (at, name) = match (pr.merged, pr.closed) {
            (Some(at), _) => (at, "pr_merged"),
            (_, Some(at)) => (at, "pr_closed"),
            _ => continue,
        };
        let closes = gh.tracker.closed_by(pr.body.as_deref().unwrap_or(""), &[], pr.of.as_deref());
        let detail = json!({"pr": pr.number, "title": pr.title, "issues": closes});
        ended.push(json!({"at": at, "issue": null, "pr": pr.number, "step": name,
            "detail": detail}));
        for number in closes {
            issues.entry(number).or_default().steps.push(Step {
                at,
                source: "github",
                step: name.into(),
                detail: detail.clone(),
            });
        }
    }
    let mut answer = interval(&entries, host, since, until, issues, ended);
    answer["log"] = json!({"file": log.file, "first": entries.first().map(|e| e.at),
        "retained_only": true});
    Ok(answer)
}

/// Interpret the retained lines without changing their source order at equal
/// timestamps. All instances of this host count, including commands written
/// while no fleet instance ran, starts, stops and failed ticks.
fn interval(
    entries: &[Entry],
    host: &str,
    since: u64,
    until: u64,
    mut issues: BTreeMap<u64, Issue>,
    mut ended: Vec<Value>,
) -> Value {
    let inside = |at| since <= at && at < until;
    let mut ticks: Vec<Ticks> = vec![];
    let mut events = vec![];
    let mut fold = false;
    for entry in entries.iter().filter(|e| e.host == host && e.at < until) {
        match &entry.what {
            What::Tick(tick) => {
                let did = tick.answer.as_ref().map(|a| a["did"].clone()).unwrap_or(json!([]));
                let mut errors =
                    tick.answer.as_ref().map(|a| a["errors"].clone()).unwrap_or(json!([]));
                let failed = tick.error.is_some()
                    || !tick.failed().is_empty()
                    || tick.line.to_ascii_uppercase().contains("FAILED");
                if let Some(error) = &tick.error {
                    errors = json!([{"error": error}]);
                }
                for (rows, failure) in [(&did, false), (&errors, true)] {
                    for row in rows.as_array().into_iter().flatten() {
                        if let Some(number) = row["issue"].as_u64() {
                            let name = row["do"].as_str().unwrap_or("tick");
                            issues.entry(number).or_default().steps.push(step(
                                entry.at,
                                name,
                                json!({"failed": failure, "tick": tick.count, "result": row}),
                            ));
                        }
                    }
                }
                if !inside(entry.at) {
                    continue;
                }
                let acted = did.as_array().is_some_and(|rows| !rows.is_empty());
                if !acted && !failed && !tick.held_back() {
                    fold = false;
                    continue;
                }
                if fold
                    && !failed
                    && let Some(last) = ticks.last_mut()
                    && !last.failed
                    && last.instance == entry.instance
                    && last.line == tick.line
                    && last.did == did
                    && last.errors == errors
                {
                    last.last = entry.at;
                    last.count += 1;
                } else {
                    ticks.push(Ticks {
                        instance: entry.instance.clone(),
                        first: entry.at,
                        last: entry.at,
                        count: 1,
                        line: tick.line.clone(),
                        failed,
                        did,
                        errors,
                    });
                }
                fold = true;
            }
            What::TickStarted { .. } | What::WokenDuring { .. } => {}
            what => {
                let (on, name) = match what {
                    What::Command { on, command, .. } => (on.as_deref(), command.as_str()),
                    What::Hot { worker, .. } => (Some(worker.as_str()), "hot"),
                    What::Cooled { worker, .. } => (Some(worker.as_str()), "cooled"),
                    What::Inspecting { worker, .. } => (Some(worker.as_str()), "inspecting"),
                    What::Inspected { worker, .. } => (Some(worker.as_str()), "inspected"),
                    What::InspectionDid { worker, .. } => (Some(worker.as_str()), "inspection_did"),
                    _ => (None, "event"),
                };
                if let Some(number) = on.and_then(issue) {
                    issues.entry(number).or_default().steps.push(step(
                        entry.at,
                        name,
                        json!({"line": entry.said(), "entry": what}),
                    ));
                }
                if inside(entry.at) {
                    events.push(json!({"at": entry.at, "instance": entry.instance,
                        "line": entry.said()}));
                }
            }
        }
    }
    let mut moved = vec![];
    for (number, mut issue) in issues {
        issue.steps.retain(|s| s.at < until);
        issue.steps.sort_by_key(|s| s.at);
        let last = issue.steps.last().map(|s| {
            let mut last = serde_json::to_value(s).unwrap();
            last["ago"] = json!(until.saturating_sub(s.at));
            last
        });
        issue.steps.retain(|s| inside(s.at));
        for intake in issue.steps.iter().filter(|s| s.source == "github" && s.step == "intake") {
            ended.push(json!({"at": intake.at, "issue": number, "pr": null,
                "step": intake.step, "detail": intake.detail}));
        }
        if !issue.steps.is_empty() || issue.pr.is_some() {
            moved.push(json!({"issue": number, "title": issue.title, "pr": issue.pr,
                "steps": issue.steps, "last_step": last}));
        }
    }
    ended.retain(|e| e["at"].as_u64().is_some_and(inside));
    ended.sort_by_key(|e| e["at"].as_u64());
    events.sort_by_key(|e| e["at"].as_u64());
    json!({"since": since, "until": until, "issues": moved, "ticks": ticks,
        "events": events, "ended": ended})
}

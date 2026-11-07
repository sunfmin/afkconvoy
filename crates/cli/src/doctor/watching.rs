//! A watching agent's interval: local steps from the retained tick log,
//! supplemented by GitHub records and endings. Boundaries are [since, until)
//! at the sources' one-second precision; the current second is never consumed.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Finding, Kind, finding};
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
    if log::stamp(at) != raw {
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

#[derive(Serialize, Deserialize)]
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
/// in its prose (a gate's queue count, or a watch's PR, for example).
fn issue(on: &str) -> Option<u64> {
    on.split_whitespace().next()?.strip_prefix('#')?.parse().ok()
}

fn step(at: u64, name: &str, detail: Value) -> Step {
    Step { at, source: "tick_log", step: name.to_string(), detail }
}

/// Maintenance and waiting are observations, not progress on an issue. In
/// particular, updating a status board must never replace a queued gate as
/// the issue's last step. Failures are represented by their errors instead.
fn moved(row: &Value) -> bool {
    !matches!(row["do"].as_str(), Some("heartbeat" | "board"))
        && !matches!(row["outcome"].as_str(), Some("nothing" | "waiting" | "failed"))
}

fn record(comment: &Comment) -> Option<Step> {
    use afkconvoy_core::record::read_marker;
    let at = comment.at?;
    for (marker, name) in [
        (afkconvoy_core::stage_record::STAGE_RECORD, "stage_record"),
        (afkconvoy_core::verdict::VERDICT, "verdict"),
        (afkconvoy_core::intake::INTAKE_RECORD, "intake"),
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
    let mut goals = vec![];
    for row in recent {
        goals.extend(misbehaved_goals(row.number, &row.comments));
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
    answer["goals"] = Value::Array(goals);
    answer["log"] = json!({"file": log.file, "first": entries.first().map(|e| e.at),
        "retained_only": true});
    Ok(answer)
}

/// The workers of issue `issue` whose work account, among `comments`, says
/// their goal misbehaved (sunfmin/control#548): an account written or
/// edited in the interval updates its issue, so each is said in the look
/// after it is.
fn misbehaved_goals(issue: u64, comments: &[Comment]) -> Vec<Value> {
    let Some((comment, account)) = comments.iter().find_map(|comment| {
        Some((comment, afkconvoy_core::account::read(&comment.body, issue).ok()?))
    }) else {
        return vec![];
    };
    account
        .workers
        .iter()
        .filter_map(|worker| {
            let evidence = worker.goal.as_ref()?.misbehaved()?;
            Some(json!({"issue": issue, "url": comment.url, "harness": worker.harness,
                "claim": worker.claim, "start": worker.start, "evidence": evidence}))
        })
        .collect()
}

/// Each worker of the interval `watching` whose goal misbehaved, as a
/// finding naming its issue and the account's evidence.
pub(super) fn goal_findings(watching: &Value) -> Vec<Finding> {
    let goals = watching["goals"].as_array().into_iter().flatten();
    goals
        .filter_map(|goal| {
            let issue = goal["issue"].as_u64()?;
            let harness = goal["harness"].as_str().unwrap_or("unknown");
            Some(finding(
                Kind::GoalMisbehaved,
                json!(["goal", issue, goal["claim"], goal["start"]]),
                format!("#{issue}: a `{harness}` worker's goal misbehaved"),
                format!(
                    "its work account {} says {}",
                    goal["url"].as_str().unwrap_or("unknown"),
                    goal["evidence"].as_str().unwrap_or_default()
                ),
                format!("afkconvoy show --issue {issue}"),
            ))
        })
        .collect()
}

/// Each failed tick of the interval `watching` whose failures no finding of
/// `found` already says, as a step that has not recovered: one condition is
/// one finding. A tick is its instance and the steps that failed in it.
pub(super) fn tick_findings(watching: &Value, found: &[Finding]) -> Vec<Finding> {
    let said: BTreeSet<&str> = found.iter().map(|finding| finding.watch_id.as_str()).collect();
    let ticks = watching["ticks"].as_array().into_iter().flatten();
    let ticks = ticks.filter_map(|tick| Ticks::deserialize(tick).ok()).filter(|tick| tick.failed);
    let mut findings: Vec<Finding> = vec![];
    for tick in ticks {
        let errors = tick.errors.as_array().map(Vec::as_slice).unwrap_or_default();
        let step = |row: &Value| {
            json!(["step", row["do"].as_str().unwrap_or("tick"), row["issue"], row["patrol"]])
        };
        let unsaid: Vec<&Value> = errors
            .iter()
            .filter(|row| !said.contains(format!("checkout:{}", step(row)).as_str()))
            .collect();
        if !errors.is_empty() && unsaid.is_empty() {
            continue;
        }
        let steps: Vec<Value> = unsaid.iter().map(|row| step(row)).collect();
        let identity = json!(["tick", tick.instance, steps]);
        let error = unsaid.iter().find_map(|row| row["error"].as_str()).unwrap_or_default();
        let fix = super::named_line(error)
            .unwrap_or_else(|| format!("afkconvoy plan --instance {}", tick.instance));
        let made = finding(
            Kind::TickFailed,
            identity,
            format!(
                "a tick failed since the last look: {} of {}, from {} to {} UTC: {}",
                if tick.count == 1 { "one".to_string() } else { format!("{} ticks", tick.count) },
                tick.instance,
                log::utc(tick.first),
                log::utc(tick.last),
                tick.line
            ),
            format!("its errors: {}", json!(unsaid)),
            fix,
        );
        if !findings.iter().any(|known| known.watch_id == made.watch_id) {
            findings.push(made);
        }
    }
    findings
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
                let did = json!(
                    tick.answer
                        .as_ref()
                        .and_then(|a| a["did"].as_array())
                        .into_iter()
                        .flatten()
                        .filter(|row| moved(row))
                        .collect::<Vec<_>>()
                );
                let mut errors =
                    tick.answer.as_ref().map(|a| a["errors"].clone()).unwrap_or(json!([]));
                // A failed step says `FAILED` in capitals; a failing cut need's
                // `not cutting here: <need> failed` is no failed tick, the
                // `cut-need` finding carries it (sunfmin/control#562).
                let failed = tick.error.is_some()
                    || !tick.failed().is_empty()
                    || tick.line.contains("FAILED");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Tick;
    use afkconvoy_core::harness::Harness;

    #[test]
    fn a_worker_whose_goal_misbehaved_is_a_finding_naming_its_issue_and_evidence() {
        use afkconvoy_core::account::{Record, SCHEMA_VERSION, Worker, WorkerGoal, render};
        let goal = |status: &str, after: u64| WorkerGoal {
            set: true,
            objective: Some("Read /b.md".into()),
            goal_turns: Some(4),
            goal_turns_after_verdict: Some(after),
            status: Some(status.into()),
            tokens_used: Some(1_000_300),
            token_budget: Some(1_000_000),
            session: Some("/kept/rollout-t.jsonl".into()),
        };
        let codex = |claim: &str, goal: WorkerGoal| Worker {
            harness: Some(Harness::Codex.into()),
            claim: Some(claim.into()),
            start: Some(100),
            goal: Some(goal),
            ..Worker::default()
        };
        let record = Record {
            schema_version: SCHEMA_VERSION,
            issue: 9,
            repo: None,
            tracker: None,
            afkconvoy_version: None,
            host: Default::default(),
            start: None,
            end: None,
            wall_seconds: None,
            outcome: None,
            workers: vec![
                codex("good", goal("complete", 0)),
                codex("spent", goal("budget_limited", 0)),
                codex("ran-on", goal("cleared", 2)),
                Worker { harness: Some(Harness::ClaudeCode.into()), ..Worker::default() },
            ],
            gates: vec![],
            legacy_evidence: None,
        };
        let comment = |body: String| Comment {
            id: 1,
            body,
            url: "https://github.com/acme/t/issues/9#issuecomment-1".into(),
            at: Some(200),
        };
        let goals = misbehaved_goals(9, &[comment("no account".into()), comment(render(&record))]);
        assert_eq!(goals.len(), 2, "{goals:?}");
        let found = goal_findings(&json!({"goals": goals}));
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|finding| finding.kind == Kind::GoalMisbehaved));
        assert_eq!(found[0].what, "#9: a `codex` worker's goal misbehaved");
        assert!(found[0].evidence.contains("issuecomment-1"), "{}", found[0].evidence);
        assert!(found[0].evidence.contains("`budget_limited`, 1000300 of 1000000"));
        assert!(found[1].evidence.contains("2 goal turns after its verdict"));
        assert!(found[1].evidence.contains("/kept/rollout-t.jsonl"));
        assert_eq!(found[0].fix, "afkconvoy show --issue 9");
        assert_ne!(found[0].watch_id, found[1].watch_id);
        assert_eq!(goal_findings(&json!({"goals": []})), []);
    }

    fn entry(at: u64, instance: &str, what: What) -> Entry {
        Entry { at, instance: instance.into(), host: "h1".into(), what }
    }

    fn tick(at: u64, line: &str, answer: Value) -> Entry {
        entry(
            at,
            "alpha",
            What::Tick(Tick {
                count: at,
                sleep: 90,
                line: line.into(),
                bound: Some(3),
                answer: Some(answer),
                error: None,
            }),
        )
    }

    fn command(at: u64, on: Option<&str>, outcome: &str) -> Entry {
        entry(
            at,
            "",
            What::Command {
                command: "gate".into(),
                on: on.map(str::to_string),
                by: "worker".into(),
                outcome: outcome.into(),
                failed: false,
            },
        )
    }

    #[test]
    fn adjacent_looks_partition_every_same_second_step_and_keep_the_last_step_before_since() {
        let entries = [
            command(9, Some("#7"), "started"),
            command(10, Some("#7"), "queued (6 ahead)"),
            command(20, Some("#7"), "runs (waited 633s)"),
            command(20, Some("#7"), "exit 1 in 122s"),
            command(20, Some("#7"), "red"),
        ];
        let active = || BTreeMap::from([(7, Issue { pr: Some(70), ..Issue::default() })]);
        let first = interval(&entries, "h1", 10, 20, active(), vec![]);
        let second = interval(&entries, "h1", 20, 30, active(), vec![]);
        let whole = interval(&entries, "h1", 10, 30, active(), vec![]);
        let steps = |v: &Value| v["issues"][0]["steps"].as_array().unwrap().clone();
        assert_eq!([steps(&first), steps(&second)].concat(), steps(&whole));
        assert_eq!(steps(&second).len(), 3);
        assert_eq!(
            second["issues"][0]["last_step"]["detail"]["entry"]["command"]["outcome"],
            "red"
        );
        let quiet = interval(&entries, "h1", 30, 40, active(), vec![]);
        assert_eq!(quiet["issues"][0]["steps"], json!([]));
        assert_eq!(quiet["issues"][0]["last_step"]["at"], 20);
        assert_eq!(quiet["issues"][0]["last_step"]["ago"], 20);
    }

    #[test]
    fn admission_repeats_fold_but_each_failure_and_each_instance_stays_visible() {
        let held = json!({"did": [], "errors": [], "held_back_by": ["load"]});
        let mut failed = tick(15, "FAILED: git ls-remote SSL_ERROR_SYSCALL", json!({}));
        let What::Tick(t) = &mut failed.what else { unreachable!() };
        t.error = Some("GitHub not reached".into());
        let mut peer = tick(17, "admission holding back", held.clone());
        peer.host = "h2".into();
        let entries = [
            tick(10, "admission holding back", held.clone()),
            tick(11, "admission holding back", held.clone()),
            tick(12, "admission holding back", held.clone()),
            tick(13, "FAILED: git ls-remote SSL_ERROR_SYSCALL", json!({})),
            tick(14, "FAILED: git ls-remote SSL_ERROR_SYSCALL", json!({})),
            failed,
            tick(
                16,
                "dispatch failed",
                json!({"did": [], "errors": [
                {"do": "dispatch", "issue": 7, "error": "orca failed"}]}),
            ),
            peer,
            entry(
                18,
                "beta",
                What::Start { worker_command: "codex".into(), harness_commands: [].into() },
            ),
            entry(19, "beta", What::Stop { line: "idle".into() }),
            command(20, None, "unattributed gate"),
        ];
        let report = interval(&entries, "h1", 10, 30, BTreeMap::new(), vec![]);
        assert_eq!(report["ticks"].as_array().unwrap().len(), 5, "{report}");
        assert_eq!(report["ticks"][0]["count"], 3);
        assert_eq!(report["ticks"][0]["last"], 12);
        assert!(report["ticks"].as_array().unwrap()[1..].iter().all(|t| t["failed"] == true));
        assert_eq!(report["issues"][0]["issue"], 7);
        assert_eq!(report["issues"][0]["steps"][0]["detail"]["failed"], true);
        assert_eq!(report["events"].as_array().unwrap().len(), 3);
        assert!(report["events"][2]["line"].as_str().unwrap().contains("unattributed gate"));
    }

    #[test]
    fn a_tick_held_only_by_a_failing_cut_need_is_no_failed_tick() {
        let need = "not cutting here: asc-api-key failed · holding 0/3 · frontier 0";
        let held = json!({"did": [], "errors": []});
        let mut errored = tick(12, need, held.clone());
        let What::Tick(t) = &mut errored.what else { unreachable!() };
        t.error = Some("GitHub not reached".into());
        let entries = [
            tick(10, need, held.clone()),
            tick(
                11,
                &format!("FAILED dispatch #7: orca failed · {need}"),
                json!({"did": [],
                "errors": [{"do": "dispatch", "issue": 7, "error": "orca failed"}]}),
            ),
            errored,
        ];
        let quiet = interval(&entries[..1], "h1", 10, 30, BTreeMap::new(), vec![]);
        assert_eq!(tick_findings(&quiet, &[]), []);
        let report = interval(&entries, "h1", 10, 30, BTreeMap::new(), vec![]);
        let found = tick_findings(&report, &[]);
        assert_eq!(found.len(), 2, "{report}");
        assert!(found.iter().all(|f| f.what.contains(need)), "{found:?}");
    }

    #[test]
    fn boundaries_accept_epoch_or_valid_utc_and_reject_invalid_civil_dates() {
        assert_eq!(since("1970-01-01T00:00:00Z"), Ok(0));
        assert_eq!(since("2026-10-25T11:42:00Z"), Ok(1_792_928_520));
        for at in [0, 951_782_400, 1_792_928_520, 253_402_300_799] {
            assert_eq!(since(&log::stamp(at)), Ok(at));
            assert_eq!(since(&at.to_string()), Ok(at));
        }
        for raw in [
            "",
            "-1",
            "tomorrow",
            "2026-02-29T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-10-25T24:00:00Z",
            "2026-10-25T11:42:60Z",
            "2026-10-25T11:42:00+00:00",
            "18446744073709551615",
        ] {
            assert!(since(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn maintenance_and_waiting_do_not_hide_a_quiet_prs_queued_gate() {
        let entries = [
            command(10, Some("#7"), "queued behind 4"),
            tick(
                20,
                "holding #7",
                json!({"did": [
                {"do": "watch", "issue": 7, "outcome": "waiting"},
                {"do": "board", "issue": 7, "outcome": "done"},
                {"do": "heartbeat", "outcome": "done"},
                {"do": "tend_train", "outcome": "nothing"}], "errors": []}),
            ),
        ];
        let active = BTreeMap::from([(7, Issue { pr: Some(70), ..Issue::default() })]);
        let report = interval(&entries, "h1", 15, 30, active, vec![]);
        assert_eq!(report["ticks"], json!([]));
        assert_eq!(report["issues"][0]["steps"], json!([]));
        assert_eq!(report["issues"][0]["last_step"]["at"], 10);
    }
}

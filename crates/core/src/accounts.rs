//! Read work accounts without treating their overlapping rows as a partition.
//! Only gate rows are summed, within their own issue, never into wall clock.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{account, record};

/// An account as answered to another system. Its URL is delivery context,
/// not a second copy stored inside the comment.
#[derive(Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub record: account::Record,
    pub url: String,
}

/// Read one window for both consumers. Incomplete measurements remain rows;
/// only the summary requires every measure it uses to be known.
pub fn rows(issues: Vec<Issue>, start: Option<u64>, end: u64) -> (Vec<Row>, Vec<Unreadable>) {
    let mut rows = vec![];
    let mut unreadable = vec![];
    for issue in issues {
        let read = match &issue.account {
            Ok(None) => continue,
            Ok(Some(body)) => {
                let read = account::read(body, issue.number);
                let at = match &read {
                    Ok(row) => row.end,
                    Err(_) if !body.contains("```json\n") => {
                        record::read_marker(account::ACCOUNT, body)
                            .and_then(|fields| fields.number("end"))
                    }
                    Err(_) => None,
                };
                if at.is_some_and(|at| at > end || start.is_some_and(|start| at < start)) {
                    continue;
                }
                read
            }
            Err(reason) => Err(reason.clone()),
        };
        match read {
            Ok(record) => rows.push(Row { record, url: issue.url }),
            Err(reason) => unreadable.push(Unreadable { number: issue.number, reason }),
        }
    }
    rows.sort_by_key(|row| row.record.issue);
    unreadable.sort_by_key(|row| row.number);
    (rows, unreadable)
}

/// An issue's account, read from its comments. An absent account is no sample;
/// an account that cannot supply every measure is reported, never partly counted.
pub struct Issue {
    pub number: u64,
    /// Whether the issue has closed.
    pub closed: bool,
    pub url: String,
    pub account: Result<Option<String>, String>,
}

struct Sample {
    number: u64,
    url: String,
    wall: u64,
    gates: u64,
    workers: usize,
    harness: String,
}

#[derive(Serialize)]
pub struct Report {
    window: Window,
    count: usize,
    wall_seconds: Distribution,
    by_harness: BTreeMap<String, Harness>,
    multiple_workers: usize,
    gate_seconds: GateSeconds,
    slowest: Vec<Slowest>,
    unreadable: Vec<Unreadable>,
}

#[derive(Serialize)]
struct Window {
    start: Option<u64>,
    end: u64,
}

#[derive(Serialize)]
struct Distribution {
    p50: Option<u64>,
    p90: Option<u64>,
    p95: Option<u64>,
    max: Option<u64>,
}

#[derive(Serialize)]
struct Harness {
    count: usize,
    wall_seconds: Distribution,
}

#[derive(Serialize)]
struct GateSeconds {
    p50: Option<u64>,
    p95: Option<u64>,
}

#[derive(Serialize)]
struct Slowest {
    number: u64,
    url: String,
    wall_seconds: u64,
}

#[derive(Serialize)]
pub struct Unreadable {
    number: u64,
    reason: String,
}

/// Summarize accounts ending in the inclusive epoch-second window. `None`
/// starts at the beginning of the record. Unknown ends remain unreadable even
/// in a bounded window: they cannot safely be excluded from it.
pub fn summarize(issues: Vec<Issue>, start: Option<u64>, end: u64) -> Report {
    let mut samples = vec![];
    let (rows, mut unreadable) = rows(issues, start, end);
    for row in rows {
        match sample(&row) {
            Ok(sample) => samples.push(sample),
            Err(reason) => unreadable.push(Unreadable { number: row.record.issue, reason }),
        }
    }
    let walls: Vec<u64> = samples.iter().map(|sample| sample.wall).collect();
    let gates: Vec<u64> = samples.iter().map(|sample| sample.gates).collect();
    let mut harnesses: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for sample in &samples {
        harnesses.entry(sample.harness.clone()).or_default().push(sample.wall);
    }
    let harnesses = harnesses
        .into_iter()
        .map(|(harness, walls)| {
            (harness, Harness { count: walls.len(), wall_seconds: distribution(&walls) })
        })
        .collect();
    let multiple_workers = samples.iter().filter(|sample| sample.workers > 1).count();
    samples.sort_by_key(|sample| (std::cmp::Reverse(sample.wall), sample.number));
    let slowest = samples
        .iter()
        .take(5)
        .map(|sample| Slowest {
            number: sample.number,
            url: sample.url.clone(),
            wall_seconds: sample.wall,
        })
        .collect();
    unreadable.sort_by_key(|row| row.number);
    let gates = distribution(&gates);
    Report {
        window: Window { start, end },
        count: samples.len(),
        wall_seconds: distribution(&walls),
        by_harness: harnesses,
        multiple_workers,
        gate_seconds: GateSeconds { p50: gates.p50, p95: gates.p95 },
        slowest,
        unreadable,
    }
}

/// Completed stage performances in closed issues' work accounts. Versioned
/// records supply the duration and verdict; unknown or inconsistent facts
/// do not become samples. Legacy records without these facts remain unknown.
pub fn stage_durations(issues: &[Issue]) -> BTreeMap<String, Vec<u64>> {
    let mut stages: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for issue in issues.iter().filter(|issue| issue.closed) {
        let Ok(Some(body)) = &issue.account else { continue };
        let Ok(account) = account::read(body, issue.number) else { continue };
        for worker in account.workers {
            if worker.verdict.as_deref() != Some("pass") {
                continue;
            }
            let (Some(stage), Some(start), Some(end), Some(seconds)) =
                (worker.stage, worker.start, worker.end, worker.seconds)
            else {
                continue;
            };
            if seconds >= 60 && end.checked_sub(start) == Some(seconds) {
                stages.entry(stage).or_default().push(seconds);
            }
        }
    }
    stages
}

/// A p95 needs at least twenty completed performances: otherwise its nearest
/// rank is just the maximum and a single outlier sets the baseline.
pub fn stage_baselines(issues: &[Issue]) -> BTreeMap<String, (u64, usize)> {
    stage_durations(issues)
        .into_iter()
        .filter_map(|(stage, mut durations)| {
            if durations.len() < 20 {
                return None;
            }
            durations.sort_unstable();
            Some((stage, (percentile(&durations, 95)?, durations.len())))
        })
        .collect()
}

fn sample(row: &Row) -> Result<Sample, String> {
    let account = &row.record;
    let start = account.start.ok_or("account start is unknown or invalid")?;
    let end = account.end.ok_or("account end is unknown or invalid")?;
    let wall = end.checked_sub(start).ok_or("account end precedes its start")?;
    if account.wall_seconds != Some(wall) {
        return Err("account wall_seconds does not match start and end".into());
    }
    if account.workers.iter().any(|worker| worker.harness.is_none() || worker.start.is_none()) {
        return Err("account worker rows cannot be read".into());
    }
    let harness =
        account.workers.first().and_then(|worker| worker.harness.as_deref()).unwrap_or("unknown");
    let mut seconds = 0u64;
    for gate in &account.gates {
        let time = gate.seconds.ok_or("gate seconds were not recorded or are invalid")?;
        seconds = seconds.checked_add(time).ok_or("gate seconds overflow")?;
    }
    Ok(Sample {
        number: account.issue,
        url: row.url.clone(),
        wall,
        gates: seconds,
        workers: account.workers.len(),
        harness: harness.to_string(),
    })
}

fn distribution(values: &[u64]) -> Distribution {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    Distribution {
        p50: percentile(&sorted, 50),
        p90: percentile(&sorted, 90),
        p95: percentile(&sorted, 95),
        max: sorted.last().copied(),
    }
}

/// Nearest rank: ceil(p * n / 100), one based. Splitting the quotient keeps
/// the calculation in range even when the slice's length approaches usize::MAX.
fn percentile(sorted: &[u64], p: usize) -> Option<u64> {
    let n = sorted.len();
    let rank =
        (n / 100).saturating_mul(p).saturating_add((n % 100).saturating_mul(p).div_ceil(100));
    sorted.get(rank.checked_sub(1)?).copied()
}

#[cfg(test)]
mod stage_tests {
    use super::*;

    fn issue(number: u64, closed: bool, seconds: u64, verdict: &str) -> Issue {
        let end = 100 + seconds;
        let body = format!(
            "<!--convoy:account -->\n```json\n{}\n```",
            serde_json::json!({
                "schema_version": 1, "issue": number, "repo": null, "tracker": null,
                "afkconvoy_version": null, "host": {"name": null, "cpu": null, "logical_cpus": null, "memory_bytes": null},
                "start": 100, "end": end, "wall_seconds": seconds, "outcome": null,
                "workers": [{"harness": "codex", "model": null, "role": null, "stage": "build", "claim": "c", "start": 100, "end": end, "seconds": seconds, "verdict": verdict}], "gates": []
            })
        );
        Issue { number, closed, url: String::new(), account: Ok(Some(body)) }
    }

    #[test]
    fn baselines_use_completed_passes_of_closed_issues_and_nearest_rank() {
        let mut issues: Vec<_> = (0..19).map(|n| issue(n, true, 3600, "pass")).collect();
        assert!(stage_baselines(&issues).is_empty());
        issues.push(issue(20, true, 7200, "pass"));
        issues.push(issue(21, false, 9000, "pass"));
        issues.push(issue(22, true, 100, "escalate"));
        issues.push(issue(23, true, 30, "pass"));
        assert_eq!(stage_baselines(&issues).get("build"), Some(&(3600, 20)));
    }
}

//! Read work accounts without treating their overlapping rows as a partition.
//! Only gate rows are summed, within their own issue, never into wall clock.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{account, record};

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
struct Unreadable {
    number: u64,
    reason: String,
}

/// Summarize accounts ending in the inclusive epoch-second window. `None`
/// starts at the beginning of the record. Unknown ends remain unreadable even
/// in a bounded window: they cannot safely be excluded from it.
pub fn summarize(issues: Vec<Issue>, start: Option<u64>, end: u64) -> Report {
    let mut samples = vec![];
    let mut unreadable = vec![];
    for issue in issues {
        let read = match &issue.account {
            Ok(None) => continue,
            Ok(Some(body)) => {
                let fields = record::read_marker(account::ACCOUNT, body);
                if let Some(at) = fields.as_ref().and_then(|fields| fields.number("end"))
                    && (at > end || start.is_some_and(|start| at < start))
                {
                    continue;
                }
                sample(&issue, body)
            }
            Err(reason) => Err(reason.clone()),
        };
        match read {
            Ok(sample) => samples.push(sample),
            Err(reason) => unreadable.push(Unreadable { number: issue.number, reason }),
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

/// Completed stage performances from closed issues' work accounts. Only a
/// passed stage is a completed performance; escalations and failures are not
/// representative of how long the work takes. Old accounts retain the start
/// marker and timed row, so no new record format is needed.
pub fn stage_durations(issues: &[Issue]) -> BTreeMap<String, Vec<u64>> {
    let mut stages: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for issue in issues.iter().filter(|issue| issue.closed) {
        let Ok(Some(body)) = &issue.account else { continue };
        if record::read_marker(account::ACCOUNT, body).is_none() {
            continue;
        }
        let Some((_, rows)) = body.split_once("\nWorkers:\n") else { continue };
        let Some((workers, _)) = rows.split_once("\nGates:\n") else { continue };
        for line in workers.lines().filter(|line| line.starts_with("- ")) {
            if !line.contains(", verdict `pass`.") {
                continue;
            }
            let Some(start) = record::markers(account::WORKER, line).into_iter().next() else {
                continue;
            };
            if start.get("consult").is_some() {
                continue;
            }
            let (Some(stage), Some(at)) = (start.get("stage"), start.number("at")) else {
                continue;
            };
            let Some((_, timed)) = line.split_once("s from ") else { continue };
            let Some((from, rest)) = timed.split_once(" to ") else { continue };
            let Some((until, _)) = rest.split_once(',') else { continue };
            let (Ok(from), Ok(until)) = (from.parse::<u64>(), until.parse::<u64>()) else {
                continue;
            };
            if from != at {
                continue;
            }
            let Some(seconds) = until.checked_sub(from) else { continue };
            if seconds >= 60 {
                stages.entry(stage.to_string()).or_default().push(seconds);
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

fn sample(issue: &Issue, body: &str) -> Result<Sample, String> {
    let fields = record::read_marker(account::ACCOUNT, body).ok_or("invalid account marker")?;
    let start = fields.number("start").ok_or("account start is unknown or invalid")?;
    let end = fields.number("end").ok_or("account end is unknown or invalid")?;
    let wall = end.checked_sub(start).ok_or("account end precedes its start")?;
    let (_, rows) = body.split_once("\nWorkers:\n").ok_or("account has no worker rows")?;
    let (workers, gates) = rows.split_once("\nGates:\n").ok_or("account has no gate rows")?;
    let starts = record::markers(account::WORKER, workers);
    let worker_rows = workers.lines().filter(|line| line.starts_with("- ")).count();
    if starts.len() != worker_rows
        || (starts.is_empty() && workers.trim() != "No worker was started.")
        || starts
            .iter()
            .any(|fields| fields.get("harness").is_none() || fields.number("at").is_none())
    {
        return Err("account worker rows cannot be read".into());
    }
    let harness = starts.first().and_then(|fields| fields.get("harness")).unwrap_or("unknown");
    let mut seconds = 0u64;
    let mut timed = false;
    for line in gates.lines().filter(|line| line.starts_with("- ")) {
        let (_, rest) = line.split_once(", ").ok_or("invalid gate row")?;
        let (time, _) = rest.split_once(", ").ok_or("invalid gate seconds")?;
        let time: u64 = time
            .strip_suffix('s')
            .and_then(|time| time.parse().ok())
            .ok_or("gate seconds were not recorded or are invalid")?;
        seconds = seconds.checked_add(time).ok_or("gate seconds overflow")?;
        timed = true;
    }
    if !timed && gates.trim() != "No gate was timed on this issue's tree." {
        return Err("account gate rows cannot be read".into());
    }
    Ok(Sample {
        number: issue.number,
        url: issue.url.clone(),
        wall,
        gates: seconds,
        workers: starts.len(),
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
            "<!--convoy:account start=100 end={end}-->\nWorkers:\n- harness `codex`, the default model, {seconds}s from 100 to {end}, stage `build`, verdict `{verdict}`. Stage record: `<!--convoy:stage stage=build-->`. Start: `<!--convoy:worker at=100 claim=c harness=codex stage=build-->`.\n\nGates:\nNo gate was timed on this issue's tree."
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

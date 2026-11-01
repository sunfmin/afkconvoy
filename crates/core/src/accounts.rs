//! Read work accounts without treating their overlapping rows as a partition.
//! Only gate rows are summed, within their own issue, never into wall clock.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{account, record};

/// An issue's account, read from its comments. An absent account is no sample;
/// an account that cannot supply every measure is reported, never partly counted.
pub struct Issue {
    pub number: u64,
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

/// Summarize accounts ending in the inclusive epoch-second window. `None`
/// starts at the beginning of the record. Unknown ends remain unreadable even
/// in a bounded window: they cannot safely be excluded from it.
pub fn summarize(issues: Vec<Issue>, start: Option<u64>, end: u64) -> Value {
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
            Err(reason) => unreadable.push(json!({"number": issue.number, "reason": reason})),
        }
    }
    let walls: Vec<u64> = samples.iter().map(|sample| sample.wall).collect();
    let gates: Vec<u64> = samples.iter().map(|sample| sample.gates).collect();
    let mut harnesses: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for sample in &samples {
        harnesses.entry(sample.harness.clone()).or_default().push(sample.wall);
    }
    let harnesses: BTreeMap<String, Value> = harnesses
        .into_iter()
        .map(|(harness, walls)| {
            (harness, json!({"count": walls.len(), "wall_seconds": distribution(&walls)}))
        })
        .collect();
    let multiple_workers = samples.iter().filter(|sample| sample.workers > 1).count();
    samples.sort_by_key(|sample| (std::cmp::Reverse(sample.wall), sample.number));
    let slowest: Vec<Value> = samples
        .iter()
        .take(5)
        .map(|sample| json!({"number": sample.number, "url": sample.url, "wall_seconds": sample.wall}))
        .collect();
    unreadable.sort_by_key(|row| row["number"].as_u64());
    let gates = distribution(&gates);
    json!({
        "window": {"start": start, "end": end}, "count": samples.len(),
        "wall_seconds": distribution(&walls), "by_harness": harnesses,
        "multiple_workers": multiple_workers,
        "gate_seconds": {"p50": gates["p50"], "p95": gates["p95"]},
        "slowest": slowest, "unreadable": unreadable,
    })
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

fn distribution(values: &[u64]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    json!({"p50": percentile(&sorted, 50), "p90": percentile(&sorted, 90),
        "p95": percentile(&sorted, 95), "max": sorted.last()})
}

/// Nearest rank: ceil(p * n / 100), one based. Splitting the quotient keeps
/// the calculation in range even when the slice's length approaches usize::MAX.
fn percentile(sorted: &[u64], p: usize) -> Option<u64> {
    let n = sorted.len();
    let rank =
        (n / 100).saturating_mul(p).saturating_add((n % 100).saturating_mul(p).div_ceil(100));
    sorted.get(rank.checked_sub(1)?).copied()
}

//! Conversion of old work-account comments.

use super::{Gate, Host, Record, SCHEMA_VERSION, Worker, data::gate_of};
use crate::{account, gate, record};

pub(super) fn legacy(body: &str, issue: u64) -> Result<Record, String> {
    let fields = record::read_marker(account::ACCOUNT, body).ok_or("invalid account marker")?;
    let start = fields.number("start");
    let end = fields.number("end");
    let (_, rows) = body.split_once("\nWorkers:\n").ok_or("account has no worker rows")?;
    let (workers, gates) = rows.split_once("\nGates:\n").ok_or("account has no gate rows")?;
    let starts = record::markers(account::WORKER, workers);
    let worker_rows = workers.lines().filter(|line| line.starts_with("- ")).count();
    if starts.len() != worker_rows
        || (starts.is_empty() && workers.trim() != "No worker was started.")
    {
        return Err("account worker rows cannot be read".into());
    }
    let workers: Vec<Worker> = starts
        .into_iter()
        .map(|fields| Worker {
            harness: fields.get("harness").map(str::to_string),
            model: fields.get("model").map(str::to_string),
            role: fields.get("role").map(str::to_string),
            stage: fields.get("stage").map(str::to_string),
            claim: fields.get("claim").map(str::to_string),
            start: fields.number("at"),
            ..Worker::default()
        })
        .collect();
    let reds: Vec<Gate> = record::markers(account::KEPT_RED, gates)
        .iter()
        .map(|fields| gate_of(fields, "red"))
        .collect();
    let mut runs = vec![];
    let mut red_index = 0usize;
    for line in gates.lines().filter(|line| line.starts_with("- ")) {
        let run = if let Some((_, rest)) = line.split_once("Recorded gate run: `") {
            rest.strip_suffix("`.")
                .and_then(|raw| record::read(gate::GATE, raw))
                .map(|fields| gate_of(&fields, "green"))
        } else if line.starts_with("- red,") {
            let run = reds.get(red_index).cloned();
            red_index = red_index.saturating_add(1);
            run
        } else {
            None
        };
        // Earlier comments may have only the seconds in prose. Preserve that
        // summary measure without inventing metadata those comments lack.
        let run = run.unwrap_or_else(|| Gate {
            seconds: line
                .split_once(", ")
                .and_then(|(_, rest)| rest.split_once(", "))
                .and_then(|(time, _)| time.strip_suffix('s'))
                .and_then(|n| n.parse().ok()),
            ..Gate::default()
        });
        runs.push(run);
    }
    if runs.is_empty() && gates.trim() != "No gate was timed on this issue's tree." {
        return Err("account gate rows cannot be read".into());
    }
    Ok(Record {
        schema_version: SCHEMA_VERSION,
        issue,
        repo: None,
        tracker: None,
        afkconvoy_version: None,
        host: Host::default(),
        start,
        end,
        wall_seconds: start.zip(end).and_then(|(start, end)| end.checked_sub(start)),
        outcome: None,
        workers,
        gates: runs,
    })
}

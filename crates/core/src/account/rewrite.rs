//! Conversion of old work-account comments.

use super::{Gate, Host, Record, SCHEMA_VERSION, Worker, data::gate_of};
use crate::{account, gate, record};

pub fn legacy(body: &str, issue: u64) -> Result<Record, String> {
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
        legacy_evidence: None,
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

/// Convert only the fleet's complete, recognizable prose layout. The original
/// evidence is retained in the record because old stage markers contain facts
/// (such as role commits and individual claim starts) outside the v1 fields.
pub fn convert(body: &str, issue: u64) -> Result<Record, String> {
    let mut row = legacy(body, issue)?;
    let (header, rest) = body.split_once("\nWorkers:\n").ok_or("missing Workers heading")?;
    let (worker_text, gate_text) = rest.split_once("\nGates:\n").ok_or("missing Gates heading")?;
    let lines: Vec<&str> = header.lines().collect();
    if lines.len() != 6
        || !lines[0].starts_with("<!--convoy:account ")
        || lines[1] != "**afkconvoy** — work account."
        || !lines[2].is_empty()
        || !lines[4].is_empty()
    {
        return Err("unrecognized account header or extra text".into());
    }
    let expected_wall = match (row.start, row.end, row.wall_seconds) {
        (Some(start), Some(end), Some(seconds)) => {
            format!("Wall clock: {seconds}s, from {start} to {end}.")
        }
        (None, Some(end), _) => {
            format!("The start is unknown. No wall clock is stated. The end is {end}.")
        }
        _ => return Err("invalid account start or end".into()),
    };
    if lines[3] != expected_wall {
        return Err("wall clock disagrees with marker".into());
    }
    let machine = lines[5]
        .strip_prefix("Machine: ")
        .and_then(|s| s.strip_suffix('.'))
        .ok_or("invalid machine row")?;
    let mut parts = machine.rsplitn(3, ", ");
    let memory = parts.next().ok_or("invalid memory")?;
    let cpus = parts.next().ok_or("invalid CPUs")?;
    let cpu = parts.next().ok_or("invalid CPU")?;
    row.host.cpu = (cpu != "chip unknown").then(|| cpu.to_string());
    row.host.logical_cpus = if cpus == "logical CPUs unknown" {
        None
    } else {
        Some(
            cpus.strip_suffix(" logical CPUs")
                .and_then(|n| n.parse().ok())
                .ok_or("invalid CPUs")?,
        )
    };
    row.host.memory_bytes = if memory == "memory unknown" {
        None
    } else {
        Some(memory.strip_suffix(" bytes").and_then(|n| n.parse().ok()).ok_or("invalid memory")?)
    };
    if row.workers.is_empty() {
        if worker_text.trim() != "No worker was started." {
            return Err("unrecognized worker text".into());
        }
    } else {
        for (worker, line) in row.workers.iter_mut().zip(worker_text.lines()) {
            let (human, marker) =
                line.split_once(". Start: `").ok_or("worker has no start marker")?;
            if !marker.ends_with("`.") || record::read_marker(account::WORKER, marker).is_none() {
                return Err("invalid worker marker".into());
            }
            let (facts, citation) = human.split_once(". ").ok_or("worker has no stage citation")?;
            if citation != "No stage record was left"
                && !(citation.starts_with("Stage record: `<!--convoy:stage ")
                    && citation.ends_with("-->`"))
            {
                return Err("unrecognized worker stage citation".into());
            }
            let (harness, rest) = facts
                .strip_prefix("- harness `")
                .and_then(|s| s.split_once("`, "))
                .ok_or("invalid worker harness")?;
            let (model, rest) = rest.split_once(", ").ok_or("invalid worker model")?;
            worker.harness = Some(harness.to_string());
            worker.model = if model == "the default model" {
                None
            } else {
                Some(
                    model
                        .strip_prefix("model `")
                        .and_then(|s| s.strip_suffix('`'))
                        .ok_or("invalid worker model")?
                        .to_string(),
                )
            };
            let (timing, extra) = rest.split_once(", ").unwrap_or((rest, ""));
            let (seconds, span) = timing.split_once("s from ").ok_or("invalid worker duration")?;
            let (start, end) = span.split_once(" to ").ok_or("invalid worker times")?;
            let seconds: u64 = seconds.parse().map_err(|_| "invalid worker seconds")?;
            let start: u64 = start.parse().map_err(|_| "invalid worker start")?;
            let end: u64 = end.parse().map_err(|_| "invalid worker end")?;
            if worker.start != Some(start) || end.checked_sub(start) != Some(seconds) {
                return Err("worker timing disagrees with marker".into());
            }
            worker.end = Some(end);
            worker.seconds = Some(seconds);
            worker.stage = None;
            for part in extra.split(", ").filter(|p| !p.is_empty()) {
                if let Some(stage) = part.strip_prefix("stage `").and_then(|s| s.strip_suffix('`'))
                {
                    worker.stage = Some(stage.to_string());
                } else if let Some(verdict) =
                    part.strip_prefix("verdict `").and_then(|s| s.strip_suffix('`'))
                {
                    worker.verdict = Some(verdict.to_string());
                } else {
                    return Err("unrecognized worker fact".into());
                }
            }
        }
    }
    let mut gate_lines = gate_text.lines().filter(|line| line.starts_with("- "));
    for run in &row.gates {
        let line = gate_lines.next().ok_or("missing gate row")?;
        if !(line.starts_with("- green, ") || line.starts_with("- red, "))
            || !line.contains(", command `")
            || !line.contains(", tree `")
            || !line.ends_with('.')
        {
            return Err("unrecognized gate row".into());
        }
        if run.seconds.is_none() && !line.contains("seconds not recorded") {
            return Err("gate seconds cannot be recovered".into());
        }
    }
    for line in gate_text.lines() {
        if !line.is_empty()
            && !line.starts_with("- ")
            && !line.starts_with("<!--convoy:account-red ")
            && line != "No gate was timed on this issue's tree."
        {
            return Err("unrecognized text after gate rows".into());
        }
    }
    row.legacy_evidence = Some(body.to_string());
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT_421: &str = include_str!("../../../cli/tests/fixtures/account-421.md");

    #[test]
    fn real_account_retains_evidence_and_recovers_rows() {
        let row = convert(ACCOUNT_421, 421).unwrap();
        assert_eq!(row.start, Some(1793539449));
        assert_eq!(row.end, Some(1793541788));
        assert_eq!(row.host.cpu.as_deref(), Some("Apple M4 Pro"));
        assert_eq!(row.host.logical_cpus, Some(14));
        assert_eq!(row.workers[0].seconds, Some(2339));
        assert_eq!(row.workers[0].end, Some(1793541788));
        assert_eq!(row.gates.iter().map(|gate| gate.seconds.unwrap()).sum::<u64>(), 966);
        let rendered = account::render(&row);
        assert_eq!(account::read(&rendered, 421).unwrap(), row);
        assert!(convert(&rendered, 421).is_err());
    }

    #[test]
    fn unknown_facts_refuse_conversion() {
        let body =
            ACCOUNT_421.replace("Machine: Apple", "Additional fact: important\nMachine: Apple");
        assert!(convert(&body, 421).is_err());
    }
}

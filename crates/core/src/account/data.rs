//! The versioned facts of a work account. Only legacy conversion reads prose.

use serde::{Deserialize, Serialize};

use crate::{account, gate, record};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub schema_version: u32,
    pub issue: u64,
    pub repo: Option<String>,
    pub tracker: Option<String>,
    pub afkconvoy_version: Option<String>,
    pub host: Host,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub wall_seconds: Option<u64>,
    pub outcome: Option<String>,
    pub workers: Vec<Worker>,
    pub gates: Vec<Gate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub name: Option<String>,
    pub cpu: Option<String>,
    pub logical_cpus: Option<u64>,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worker {
    pub harness: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub stage: Option<String>,
    pub claim: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub seconds: Option<u64>,
    pub verdict: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub tree: Option<String>,
    pub command: Option<String>,
    pub lane: Option<String>,
    pub result: Option<String>,
    pub at: Option<u64>,
    pub seconds: Option<u64>,
    pub host: Option<String>,
}

/// Read the authoritative JSON block, or convert an old comment once. Never
/// fall back to prose when a JSON block is damaged or has a future version.
pub fn read(body: &str, issue: u64) -> Result<Record, String> {
    if let Some((_, rest)) = body.rsplit_once("```json\n") {
        let (json, _) = rest.split_once("\n```").ok_or("unterminated account JSON block")?;
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| format!("invalid account JSON: {error}"))?;
        let version = value.get("schema_version").and_then(serde_json::Value::as_u64);
        if version != Some(u64::from(SCHEMA_VERSION)) {
            return Err(format!("unsupported account schema version: {}", value["schema_version"]));
        }
        let row: Record = serde_json::from_value(value)
            .map_err(|error| format!("invalid account record: {error}"))?;
        if row.issue != issue {
            return Err("account issue does not match its issue".into());
        }
        return Ok(row);
    }
    legacy(body, issue)
}

/// Render only from the record. Escape HTML and fence characters in the JSON
/// so even a command containing a closing details tag or fence stays data.
pub fn render(row: &Record) -> String {
    let wall = match (row.start, row.end, row.wall_seconds) {
        (Some(start), Some(end), Some(seconds)) => {
            format!("Wall clock: {seconds}s, from {start} to {end}.")
        }
        _ => "The start is unknown. No wall clock is stated.".to_string(),
    };
    let cpu = row.host.cpu.as_deref().unwrap_or("chip unknown");
    let cpus = row
        .host
        .logical_cpus
        .map(|n| format!("{n} logical CPUs"))
        .unwrap_or_else(|| "logical CPUs unknown".into());
    let bytes = row
        .host
        .memory_bytes
        .map(|n| format!("{n} bytes"))
        .unwrap_or_else(|| "memory unknown".into());
    let mut workers = vec![];
    for worker in &row.workers {
        let harness = worker.harness.as_deref().unwrap_or("unknown");
        let model = worker
            .model
            .as_ref()
            .map(|model| format!("model `{model}`"))
            .unwrap_or_else(|| "the default model".into());
        let timing = match (worker.start, worker.end, worker.seconds) {
            (Some(start), Some(end), Some(seconds)) => format!("{seconds}s from {start} to {end}"),
            _ => "duration unknown".into(),
        };
        let mut line = format!("- harness `{harness}`, {model}, {timing}");
        if let Some(stage) = &worker.stage {
            line.push_str(&format!(", stage `{stage}`"));
        }
        if let Some(verdict) = &worker.verdict {
            line.push_str(&format!(", verdict `{verdict}`"));
        }
        workers.push(format!("{line}."));
    }
    let mut gates = vec![];
    for gate in &row.gates {
        let result = gate.result.as_deref().unwrap_or("unknown");
        let seconds =
            gate.seconds.map(|n| format!("{n}s")).unwrap_or_else(|| "seconds not recorded".into());
        let command = gate.command.as_deref().unwrap_or("unknown");
        let tree = gate.tree.as_deref().unwrap_or("unknown");
        let mut line = format!("- {result}, {seconds}, command `{command}`, tree `{tree}`");
        if let Some(at) = gate.at {
            line.push_str(&format!(", at {at}"));
        }
        if gate.host != row.host.name
            && let Some(host) = &gate.host
        {
            line.push_str(&format!(", on host `{host}`"));
        }
        gates.push(format!("{line}."));
    }
    let workers =
        if workers.is_empty() { "No worker was started.".into() } else { workers.join("\n") };
    let gates = if gates.is_empty() {
        "No gate was timed on this issue's tree.".into()
    } else {
        gates.join("\n")
    };
    // Serializing these types is infallible: all keys are strings and no floats exist.
    let json = serde_json::to_string_pretty(row)
        .unwrap_or_default()
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('`', "\\u0060");
    format!(
        "<!--convoy:account -->\n**afkconvoy** — work account.\n\n{wall}\n\nMachine: {cpu}, {cpus}, {bytes}.\n\nWorkers:\n{workers}\n\nGates:\n{gates}\n\n<details>\n<summary>Work account JSON</summary>\n\n```json\n{json}\n```\n\n</details>\n"
    )
}

pub(super) fn gate_of(fields: &record::Fields, result: &str) -> Gate {
    Gate {
        tree: fields.get("tree").map(str::to_string),
        command: fields.get("command").map(str::to_string),
        lane: fields.get("lane").map(str::to_string),
        result: Some(result.to_string()),
        at: fields.number("at"),
        seconds: fields.number("seconds"),
        host: fields.get("host").map(str::to_string),
    }
}

fn legacy(body: &str, issue: u64) -> Result<Record, String> {
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

//! The versioned facts of a work account. Only legacy conversion reads prose.

use serde::{Deserialize, Serialize};

use crate::harness::Harness;
use crate::record;

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
    /// Original evidence retained only when converting a legacy account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_evidence: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub name: Option<String>,
    pub cpu: Option<String>,
    pub logical_cpus: Option<u64>,
    pub memory_bytes: Option<u64>,
}

/// The harness a work account names: one this binary knows, or a name it
/// does not — one a later afkconvoy wrote — kept as it was written, so the
/// account still reads and writes back as it was (sunfmin/control#630).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedHarness(Recorded);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Recorded {
    Known(Harness),
    Unknown(String),
}

impl RecordedHarness {
    /// The harness written as `key`.
    pub fn read(key: &str) -> RecordedHarness {
        RecordedHarness(match Harness::read(key) {
            Some(harness) => Recorded::Known(harness),
            None => Recorded::Unknown(key.to_string()),
        })
    }

    /// The harness, when this binary knows it.
    pub fn known(&self) -> Option<Harness> {
        match &self.0 {
            Recorded::Known(harness) => Some(*harness),
            Recorded::Unknown(_) => None,
        }
    }

    /// What it is written as.
    pub fn key(&self) -> &str {
        match &self.0 {
            Recorded::Known(harness) => harness.key(),
            Recorded::Unknown(key) => key,
        }
    }

    /// What an account and its summary call `harness`: its key, or
    /// `unknown` where the record names none.
    pub fn named(harness: Option<&RecordedHarness>) -> &str {
        harness.map_or("unknown", RecordedHarness::key)
    }
}

impl From<Harness> for RecordedHarness {
    fn from(harness: Harness) -> RecordedHarness {
        RecordedHarness(Recorded::Known(harness))
    }
}

impl Serialize for RecordedHarness {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

impl<'de> Deserialize<'de> for RecordedHarness {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(RecordedHarness::read(&String::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worker {
    pub harness: Option<RecordedHarness>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub stage: Option<String>,
    pub claim: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub seconds: Option<u64>,
    pub verdict: Option<String>,
    #[serde(default)]
    pub tokens: Option<super::Tokens>,
    /// How its goal went, where its harness keeps one: absent for any other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<super::WorkerGoal>,
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

/// Read the authoritative JSON block only. Damaged blocks and unknown schema
/// versions are unreadable; prose conversion belongs to the rewrite.
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
    Err("account has no JSON block".into())
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
        let harness = RecordedHarness::named(worker.harness.as_ref());
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
        if let Some(tokens) = &worker.tokens {
            for (model, count) in tokens {
                let model = if model.is_empty() { "unknown" } else { model };
                line.push_str(&format!(
                    ", tokens ({model}): input {}, cache read {}, cache write {}, output {}",
                    count.input, count.cache_read, count.cache_write, count.output
                ));
                if let Some(reasoning) = count.reasoning {
                    line.push_str(&format!(", reasoning {reasoning}"));
                }
            }
        } else {
            line.push_str(", tokens unknown");
        }
        if let Some(goal) = &worker.goal {
            line.push_str(&format!(", {}", goal.words()));
        }
        workers.push(format!("{line}."));
    }
    let mut total = super::Counts::default();
    let mut unknown = 0usize;
    for worker in &row.workers {
        match &worker.tokens {
            Some(tokens) => {
                for count in tokens.values() {
                    total.add(count);
                }
            }
            None => unknown = unknown.saturating_add(1),
        }
    }
    let token_total = format!(
        "Tokens: input {}, cache read {}, cache write {}, output {}; {unknown} workers unknown.",
        total.input, total.cache_read, total.cache_write, total.output
    );
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
        "<!--convoy:account -->\n**afkconvoy** — work account.\n\n{wall}\n\nMachine: {cpu}, {cpus}, {bytes}.\n\nWorkers:\n{workers}\n\n{token_total}\n\nGates:\n{gates}\n\n<details>\n<summary>Work account JSON</summary>\n\n```json\n{json}\n```\n\n</details>\n"
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Accounts of schema version 1 as the fleet wrote them: one with every
    /// fact it records, a harness of every kind among them, and one with
    /// none.
    const V1: &str = include_str!("recorded/v1.md");
    const V1_EMPTY: &str = include_str!("recorded/v1-empty.md");

    #[test]
    fn a_recorded_account_of_each_schema_version_reads_and_writes_back_byte_for_byte() {
        assert_eq!(SCHEMA_VERSION, 1, "a new version brings a recorded account of its own");
        for (issue, body) in [(628, V1), (7, V1_EMPTY)] {
            let row = read(body, issue).unwrap();
            assert_eq!(row.schema_version, 1);
            assert_eq!(render(&row), body, "issue {issue}");
        }
    }

    #[test]
    fn a_harness_name_this_binary_does_not_know_reads_as_it_was_written() {
        let row = read(V1, 628).unwrap();
        let named: Vec<Option<String>> = row
            .workers
            .iter()
            .map(|worker| serde_json::to_value(&worker.harness).unwrap().as_str().map(String::from))
            .collect();
        assert_eq!(
            named,
            [Some("codex"), Some("claude-code"), Some("kimi"), Some("opencode"), None]
                .map(|name| name.map(String::from))
        );
        let known: Vec<Option<Harness>> = row
            .workers
            .iter()
            .map(|worker| worker.harness.as_ref().and_then(RecordedHarness::known))
            .collect();
        assert_eq!(
            known,
            [Some(Harness::Codex), Some(Harness::ClaudeCode), Some(Harness::Kimi), None, None]
        );
        assert!(render(&row).contains("- harness `opencode`, model"), "{}", render(&row));
        assert!(render(&row).contains("- harness `unknown`, the default model"));
    }
}

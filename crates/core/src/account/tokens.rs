//! Token usage as the harness recorded it. Input excludes cache hits and writes;
//! reasoning, when stated, is included in output and is not added to the total.

use crate::harness::Harness;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: Option<u64>,
}

impl Counts {
    pub fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = match (self.reasoning, other.reasoning) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        };
    }
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
            .saturating_add(self.output)
    }
    fn delta(&self, previous: &Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            cache_read: self.cache_read.saturating_sub(previous.cache_read),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            output: self.output.saturating_sub(previous.output),
            reasoning: self.reasoning.map(|n| n.saturating_sub(previous.reasoning.unwrap_or(0))),
        }
    }
}

/// Usage by model. An unknown model has the empty key; unknown usage is `None`.
pub type Tokens = BTreeMap<String, Counts>;

fn number(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}
fn normalized(
    v: &Value,
    input: &str,
    read: &str,
    write: &str,
    output: &str,
    reasoning: &str,
    inclusive: bool,
) -> Counts {
    let cache_read = number(v, read);
    let cache_write = number(v, write);
    let raw = number(v, input);
    Counts {
        input: if inclusive {
            raw.saturating_sub(cache_read).saturating_sub(cache_write)
        } else {
            raw
        },
        cache_read,
        cache_write,
        output: number(v, output),
        reasoning: v[reasoning].as_u64(),
    }
}

/// Read one complete session, including continuations. `None` means no usage
/// evidence, not zero. Repeated assistant messages are keyed by message id;
/// cumulative Codex and Grok counters are read only once per snapshot.
pub fn parse(harness: Harness, text: &str) -> Option<Tokens> {
    let mut result = Tokens::new();
    let mut messages: BTreeMap<String, (String, Counts)> = BTreeMap::new();
    let mut model = String::new();
    let mut previous = Counts::default();
    let mut found = false;
    for (line_no, line) in text.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match harness {
            Harness::Codex => {
                if v["type"] == "turn_context"
                    && let Some(m) = v["payload"]["model"].as_str()
                {
                    model = m.to_string();
                }
                if v["type"] != "event_msg" || v["payload"]["type"] != "token_count" {
                    continue;
                }
                let usage = &v["payload"]["info"]["total_token_usage"];
                if !usage["input_tokens"].is_number() {
                    continue;
                }
                let current = normalized(
                    usage,
                    "input_tokens",
                    "cached_input_tokens",
                    "cache_write_input_tokens",
                    "output_tokens",
                    "reasoning_output_tokens",
                    true,
                );
                let delta = current.delta(&previous);
                result.entry(model.clone()).or_default().add(&delta);
                previous = current;
                found = true;
            }
            Harness::ClaudeCode | Harness::Pi => {
                if v["type"] != if harness == Harness::ClaudeCode { "assistant" } else { "message" }
                {
                    continue;
                }
                let msg = &v["message"];
                if msg["role"] != "assistant" {
                    continue;
                }
                let usage = &msg["usage"];
                let (input, read, write, output, reasoning) = if harness == Harness::ClaudeCode {
                    (
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                        "output_tokens",
                        "reasoning_tokens",
                    )
                } else {
                    ("input", "cacheRead", "cacheWrite", "output", "reasoning")
                };
                if !usage[input].is_number() {
                    continue;
                }
                let id = msg["id"]
                    .as_str()
                    .or_else(|| v["id"].as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("line:{line_no}"));
                let mut counts = normalized(usage, input, read, write, output, reasoning, false);
                if harness == Harness::ClaudeCode {
                    counts.reasoning = usage["output_tokens_details"]["thinking_tokens"]
                        .as_u64()
                        .or(counts.reasoning);
                }
                messages.insert(id, (msg["model"].as_str().unwrap_or("").to_string(), counts));
                found = true;
            }
            Harness::Kimi => {
                if v["type"] != "usage.record" || !v["usage"]["inputOther"].is_number() {
                    continue;
                }
                let usage = &v["usage"];
                let key = format!("{}:{}", v["time"], v["agentId"]);
                messages.insert(
                    key,
                    (
                        v["model"].as_str().unwrap_or("").to_string(),
                        normalized(
                            usage,
                            "inputOther",
                            "inputCacheRead",
                            "inputCacheCreation",
                            "output",
                            "reasoning",
                            false,
                        ),
                    ),
                );
                found = true;
            }
            Harness::Grok => {
                let usage = &v["params"]["update"]["usage"];
                let Some(models) = usage["modelUsage"].as_object() else { continue };
                let mut snapshot = Tokens::new();
                for (name, usage) in models {
                    if usage["inputTokens"].is_number() {
                        snapshot.insert(
                            name.clone(),
                            normalized(
                                usage,
                                "inputTokens",
                                "cachedReadTokens",
                                "cacheCreationTokens",
                                "outputTokens",
                                "reasoningTokens",
                                true,
                            ),
                        );
                    }
                }
                if !snapshot.is_empty() {
                    result = snapshot;
                    found = true;
                }
            }
            Harness::Dsh => {}
        }
    }
    for (_, (model, counts)) in messages {
        result.entry(model).or_default().add(&counts);
    }
    found.then_some(result)
}

/// Usage recorded from `start` through `end`, inclusive. Cumulative counters
/// establish their baseline before the window, so a resident session's earlier
/// rounds and standby are not charged to this one. A usage record without a
/// readable timestamp makes the window unknown.
pub fn parse_between(harness: Harness, text: &str, start: u64, end: u64) -> Option<Tokens> {
    let mut result = Tokens::new();
    let mut messages: BTreeMap<String, (u64, String, Counts)> = BTreeMap::new();
    let mut model = String::new();
    let mut previous = Counts::default();
    let mut previous_grok = Tokens::new();
    let mut latest_grok = Tokens::new();
    let mut found = false;
    for (line_no, line) in text.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if harness == Harness::Codex
            && v["type"] == "turn_context"
            && let Some(m) = v["payload"]["model"].as_str()
        {
            model = m.to_string();
        }
        let usage = match harness {
            Harness::Codex if v["type"] == "event_msg" && v["payload"]["type"] == "token_count" => {
                &v["payload"]["info"]["total_token_usage"]
            }
            Harness::ClaudeCode | Harness::Pi
                if v["type"]
                    == if harness == Harness::ClaudeCode { "assistant" } else { "message" } =>
            {
                &v["message"]["usage"]
            }
            Harness::Kimi if v["type"] == "usage.record" => &v["usage"],
            Harness::Grok => &v["params"]["update"]["usage"],
            _ => continue,
        };
        let Some(at) = usage_at(&v) else {
            if usage.is_object() {
                return None;
            }
            continue;
        };
        match harness {
            Harness::Codex => {
                if !usage["input_tokens"].is_number() {
                    continue;
                }
                let current = normalized(
                    usage,
                    "input_tokens",
                    "cached_input_tokens",
                    "cache_write_input_tokens",
                    "output_tokens",
                    "reasoning_output_tokens",
                    true,
                );
                if at < start {
                    previous = current;
                    continue;
                }
                if at > end {
                    continue;
                }
                let delta = current.delta(&previous);
                result.entry(model.clone()).or_default().add(&delta);
                previous = current;
                found = true;
            }
            Harness::ClaudeCode | Harness::Pi | Harness::Kimi => {
                let msg = &v["message"];
                let (input, read, write, output, reasoning) = match harness {
                    Harness::ClaudeCode => (
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                        "output_tokens",
                        "reasoning_tokens",
                    ),
                    Harness::Pi => ("input", "cacheRead", "cacheWrite", "output", "reasoning"),
                    _ => (
                        "inputOther",
                        "inputCacheRead",
                        "inputCacheCreation",
                        "output",
                        "reasoning",
                    ),
                };
                if !usage[input].is_number() {
                    continue;
                }
                let id = if harness == Harness::Kimi {
                    format!("{}:{}", v["time"], v["agentId"])
                } else {
                    msg["id"]
                        .as_str()
                        .or_else(|| v["id"].as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("line:{line_no}"))
                };
                let mut counts = normalized(usage, input, read, write, output, reasoning, false);
                if harness == Harness::ClaudeCode {
                    counts.reasoning = usage["output_tokens_details"]["thinking_tokens"]
                        .as_u64()
                        .or(counts.reasoning);
                }
                let model = if harness == Harness::Kimi {
                    v["model"].as_str().unwrap_or("")
                } else {
                    msg["model"].as_str().unwrap_or("")
                };
                messages.entry(id).or_insert_with(|| (at, model.to_string(), counts));
            }
            Harness::Grok => {
                let Some(models) = usage["modelUsage"].as_object() else { continue };
                let mut snapshot = Tokens::new();
                for (name, usage) in models {
                    if usage["inputTokens"].is_number() {
                        snapshot.insert(
                            name.clone(),
                            normalized(
                                usage,
                                "inputTokens",
                                "cachedReadTokens",
                                "cacheCreationTokens",
                                "outputTokens",
                                "reasoningTokens",
                                true,
                            ),
                        );
                    }
                }
                if snapshot.is_empty() {
                    continue;
                }
                if at < start {
                    previous_grok = snapshot;
                } else if at <= end {
                    latest_grok = snapshot;
                    found = true;
                }
            }
            Harness::Dsh => {}
        }
    }
    if matches!(harness, Harness::ClaudeCode | Harness::Pi | Harness::Kimi) {
        for (at, model, counts) in messages.into_values() {
            if at >= start && at <= end {
                result.entry(model).or_default().add(&counts);
                found = true;
            }
        }
    }
    if harness == Harness::Grok {
        for (model, counts) in latest_grok {
            let delta = counts.delta(previous_grok.get(&model).unwrap_or(&Counts::default()));
            result.entry(model).or_default().add(&delta);
        }
    }
    found.then_some(result)
}

fn usage_at(value: &Value) -> Option<u64> {
    for key in ["timestamp", "time", "ts"] {
        if let Some(n) = value[key].as_u64() {
            return Some(crate::harness::epoch_number(n));
        }
        if let Some(raw) = value[key].as_str() {
            return crate::harness::parse_iso(raw);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_records_count_each_message_and_continuation_once() {
        let cases = [
            (
                Harness::Codex,
                r#"{"type":"turn_context","payload":{"model":"a"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":30,"output_tokens":10,"reasoning_output_tokens":4}}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":30,"output_tokens":10,"reasoning_output_tokens":4}}}}
{"type":"turn_context","payload":{"model":"b"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":50,"output_tokens":15,"reasoning_output_tokens":6}}}}"#,
                (100, 50, 0, 15),
            ),
            (
                Harness::ClaudeCode,
                r#"{"type":"assistant","message":{"id":"1","role":"assistant","model":"a","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":3,"output_tokens":5}}}
{"type":"assistant","message":{"id":"1","role":"assistant","model":"a","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":3,"output_tokens":5}}}
{"type":"assistant","message":{"id":"2","role":"assistant","model":"b","usage":{"input_tokens":6,"cache_read_input_tokens":7,"cache_creation_input_tokens":1,"output_tokens":2}}}"#,
                (16, 27, 4, 7),
            ),
            (
                Harness::Kimi,
                r#"{"type":"usage.record","time":1,"model":"a","usage":{"inputOther":10,"inputCacheRead":20,"inputCacheCreation":3,"output":5}}
{"type":"context.append_loop_event","event":{"usage":{"inputOther":10,"inputCacheRead":20,"inputCacheCreation":3,"output":5}}}
{"type":"usage.record","time":2,"model":"b","usage":{"inputOther":6,"inputCacheRead":7,"inputCacheCreation":1,"output":2}}"#,
                (16, 27, 4, 7),
            ),
            (
                Harness::Pi,
                r#"{"type":"message","id":"1","message":{"role":"assistant","model":"a","usage":{"input":10,"cacheRead":20,"cacheWrite":3,"output":5}}}
{"type":"message","id":"1","message":{"role":"assistant","model":"a","usage":{"input":10,"cacheRead":20,"cacheWrite":3,"output":5}}}
{"type":"message","id":"2","message":{"role":"assistant","model":"b","usage":{"input":6,"cacheRead":7,"cacheWrite":1,"output":2}}}"#,
                (16, 27, 4, 7),
            ),
            (
                Harness::Grok,
                r#"{"params":{"update":{"usage":{"modelUsage":{"a":{"inputTokens":33,"cachedReadTokens":20,"cacheCreationTokens":3,"outputTokens":5}}}}}}
{"params":{"update":{"usage":{"modelUsage":{"a":{"inputTokens":33,"cachedReadTokens":20,"cacheCreationTokens":3,"outputTokens":5},"b":{"inputTokens":14,"cachedReadTokens":7,"cacheCreationTokens":1,"outputTokens":2}}}}}}"#,
                (16, 27, 4, 7),
            ),
        ];
        for (harness, text, expected) in cases {
            let tokens = parse(harness, text).unwrap();
            assert_eq!(tokens.len(), 2, "{harness:?}");
            let mut total = Counts::default();
            for counts in tokens.values() {
                total.add(counts);
            }
            assert_eq!(
                (total.input, total.cache_read, total.cache_write, total.output),
                expected,
                "{harness:?}"
            );
        }
        assert_eq!(parse(Harness::Codex, "{}"), None);
    }

    #[test]
    fn a_round_window_excludes_standby_and_subtracts_cumulative_baselines() {
        let codex = r#"{"timestamp":90,"type":"turn_context","payload":{"model":"codex"}}
{"timestamp":95,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":30,"output_tokens":10,"reasoning_output_tokens":4}}}}
{"timestamp":105,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":160,"cached_input_tokens":45,"output_tokens":18,"reasoning_output_tokens":7}}}}
{"timestamp":115,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":55,"output_tokens":25,"reasoning_output_tokens":9}}}}"#;
        let tokens = parse_between(Harness::Codex, codex, 100, 110).unwrap();
        assert_eq!(
            tokens["codex"],
            Counts { input: 45, cache_read: 15, cache_write: 0, output: 8, reasoning: Some(3) }
        );

        let claude = r#"{"timestamp":"1970-01-01T00:01:35Z","type":"assistant","message":{"id":"before","role":"assistant","model":"claude","usage":{"input_tokens":10,"cache_read_input_tokens":2,"output_tokens":3}}}
{"timestamp":"1970-01-01T00:01:45Z","type":"assistant","message":{"id":"round","role":"assistant","model":"claude","usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":1,"output_tokens":4}}}
{"timestamp":"1970-01-01T00:01:55Z","type":"assistant","message":{"id":"after","role":"assistant","model":"claude","usage":{"input_tokens":20,"cache_read_input_tokens":8,"output_tokens":9}}}"#;
        let tokens = parse_between(Harness::ClaudeCode, claude, 100, 110).unwrap();
        assert_eq!(
            tokens["claude"],
            Counts { input: 7, cache_read: 5, cache_write: 1, output: 4, reasoning: None }
        );

        let grok = r#"{"ts":95,"params":{"update":{"usage":{"modelUsage":{"grok":{"inputTokens":130,"cachedReadTokens":30,"outputTokens":10}}}}}}
{"ts":105,"params":{"update":{"usage":{"modelUsage":{"grok":{"inputTokens":190,"cachedReadTokens":45,"outputTokens":18}}}}}}
{"ts":115,"params":{"update":{"usage":{"modelUsage":{"grok":{"inputTokens":255,"cachedReadTokens":55,"outputTokens":25}}}}}}"#;
        let tokens = parse_between(Harness::Grok, grok, 100, 110).unwrap();
        assert_eq!(
            tokens["grok"],
            Counts { input: 45, cache_read: 15, cache_write: 0, output: 8, reasoning: None }
        );
        assert_eq!(parse_between(Harness::Codex, codex, 200, 210), None);
        assert_eq!(
            parse_between(
                Harness::Codex,
                &codex.replace("\"timestamp\":105", "\"at\":105"),
                100,
                110
            ),
            None
        );
    }
}

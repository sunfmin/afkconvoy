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
                if v["type"] == "turn_context" {
                    if let Some(m) = v["payload"]["model"].as_str() {
                        model = m.to_string();
                    }
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
}

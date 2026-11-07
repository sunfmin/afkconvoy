//! Token usage as the harness recorded it. Input excludes cache hits and writes;
//! reasoning, when stated, is included in output and is not added to the total.
//!
//! Which session lines record usage, and what they count, is each harness's
//! (`Harness::usage`). This module only adds the records up.

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

    /// The counts one usage object `v` states under the harness's `keys`, or
    /// `None` when it states no input. An `inclusive` input counts the cache
    /// reads and writes too, which are taken out of it.
    pub(crate) fn read(v: &Value, keys: &Keys, inclusive: bool) -> Option<Self> {
        let raw = v[keys.input].as_u64()?;
        let cache_read = number(v, keys.cache_read);
        let cache_write = number(v, keys.cache_write);
        Some(Counts {
            input: if inclusive {
                raw.saturating_sub(cache_read).saturating_sub(cache_write)
            } else {
                raw
            },
            cache_read,
            cache_write,
            output: number(v, keys.output),
            reasoning: v[keys.reasoning].as_u64(),
        })
    }
}

/// The names one harness gives the counts of a usage object.
pub(crate) struct Keys {
    pub input: &'static str,
    pub cache_read: &'static str,
    pub cache_write: &'static str,
    pub output: &'static str,
    pub reasoning: &'static str,
}

/// Usage by model. An unknown model has the empty key; unknown usage is `None`.
pub type Tokens = BTreeMap<String, Counts>;

fn number(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

/// What one session line says of usage, as its harness reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Usage {
    /// The model the running counters after it count.
    Model(String),
    /// The session's running counter so far, of the latest model.
    Running(Counts),
    /// One message's usage. A message repeated under the same `key` counts once.
    Message { key: String, model: String, counts: Counts },
    /// Every model's running usage so far.
    Snapshot(Tokens),
    /// A usage record that states no counts. In a window it still needs a time.
    Uncounted,
}

impl Usage {
    /// Whether this line is a usage record, which a window must place in time.
    fn timed(&self) -> bool {
        !matches!(self, Usage::Model(_))
    }
}

/// The usage `text` records: the whole session, including continuations, or
/// from `window.0` through `window.1` inclusive. `usage` is the harness's
/// reading of one line and its number. `None` means no usage evidence, not
/// zero.
///
/// A repeated message is counted once. Running counters and snapshots are
/// read as the difference from the last one before the window, so a resident
/// session's earlier rounds and standby are not charged to this one. In a
/// window, a usage record without a readable timestamp makes the window
/// unknown.
pub fn fold(
    text: &str,
    window: Option<(u64, u64)>,
    usage: impl Fn(&Value, usize) -> Option<Usage>,
) -> Option<Tokens> {
    let mut result = Tokens::new();
    let mut messages: BTreeMap<String, (u64, String, Counts)> = BTreeMap::new();
    let mut model = String::new();
    let mut previous = Counts::default();
    let mut previous_snapshot = Tokens::new();
    let mut latest_snapshot = Tokens::new();
    let mut found = false;
    for (line_no, line) in text.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(record) = usage(&v, line_no) else { continue };
        let at = match window {
            None => 0,
            Some(_) if !record.timed() => 0,
            Some(_) => usage_at(&v)?,
        };
        // Before the window, after it, or (with no window) inside it.
        let place = window.map_or(std::cmp::Ordering::Equal, |(start, end)| {
            if at < start {
                std::cmp::Ordering::Less
            } else if at > end {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        match record {
            Usage::Model(name) => model = name,
            Usage::Running(current) => match place {
                std::cmp::Ordering::Less => previous = current,
                std::cmp::Ordering::Greater => {}
                std::cmp::Ordering::Equal => {
                    result.entry(model.clone()).or_default().add(&current.delta(&previous));
                    previous = current;
                    found = true;
                }
            },
            Usage::Message { key, model, counts } => {
                if window.is_none() {
                    messages.insert(key, (at, model, counts));
                } else {
                    messages.entry(key).or_insert((at, model, counts));
                }
            }
            Usage::Snapshot(snapshot) => {
                if snapshot.is_empty() {
                    continue;
                }
                match place {
                    std::cmp::Ordering::Less => previous_snapshot = snapshot,
                    std::cmp::Ordering::Greater => {}
                    std::cmp::Ordering::Equal => {
                        latest_snapshot = snapshot;
                        found = true;
                    }
                }
            }
            Usage::Uncounted => {}
        }
    }
    for (at, model, counts) in messages.into_values() {
        if window.is_none_or(|(start, end)| at >= start && at <= end) {
            result.entry(model).or_default().add(&counts);
            found = true;
        }
    }
    for (model, counts) in latest_snapshot {
        let delta = counts.delta(previous_snapshot.get(&model).unwrap_or(&Counts::default()));
        result.entry(model).or_default().add(&delta);
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

    fn counts(input: u64) -> Counts {
        Counts { input, ..Counts::default() }
    }

    /// A line `{"n": …}` is a running counter, `{"m": …}` a model.
    fn running(v: &Value, _: usize) -> Option<Usage> {
        if let Some(m) = v["m"].as_str() {
            return Some(Usage::Model(m.into()));
        }
        Some(v["n"].as_u64().map_or(Usage::Uncounted, |n| Usage::Running(counts(n))))
    }

    #[test]
    fn running_counters_are_read_as_their_increase() {
        let text = "{\"m\":\"a\"}\n{\"n\":10}\n{\"n\":10}\n{\"m\":\"b\"}\n{\"n\":25}";
        let tokens = fold(text, None, running).unwrap();
        assert_eq!((tokens["a"].input, tokens["b"].input), (10, 15));
        assert_eq!(fold("{}", None, |_, _| None), None);
    }

    #[test]
    fn a_window_needs_every_usage_record_placed_in_time() {
        let text = "{\"m\":\"a\"}\n{\"ts\":5,\"n\":10}\n{\"ts\":15,\"n\":30}\n{\"ts\":25,\"n\":70}";
        assert_eq!(fold(text, Some((10, 20)), running).unwrap()["a"].input, 20);
        assert_eq!(fold(text, Some((30, 40)), running), None);
        let untimed = format!("{text}\n{{}}");
        assert_eq!(fold(&untimed, Some((10, 20)), running), None);
    }
}

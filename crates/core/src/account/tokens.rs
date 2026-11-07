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
    let mut sum = Sum { window, ..Sum::default() };
    for (line_no, line) in text.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(record) = usage(&v, line_no) else { continue };
        let at = if window.is_some() && record.timed() { usage_at(&v)? } else { 0 };
        sum.take(record, at);
    }
    sum.total()
}

/// Where a record falls against the window. With no window, every record is in it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Before,
    In,
    After,
}

/// The records of one session read so far.
#[derive(Default)]
struct Sum {
    window: Option<(u64, u64)>,
    result: Tokens,
    messages: BTreeMap<String, (u64, String, Counts)>,
    model: String,
    previous: Counts,
    previous_snapshot: Tokens,
    latest_snapshot: Tokens,
    found: bool,
}

impl Sum {
    fn place(&self, at: u64) -> Place {
        match self.window {
            Some((start, _)) if at < start => Place::Before,
            Some((_, end)) if at > end => Place::After,
            _ => Place::In,
        }
    }

    fn take(&mut self, record: Usage, at: u64) {
        let place = self.place(at);
        match record {
            Usage::Model(name) => self.model = name,
            Usage::Running(current) => self.running(current, place),
            Usage::Message { key, model, counts } => {
                if self.window.is_none() {
                    self.messages.insert(key, (at, model, counts));
                } else {
                    self.messages.entry(key).or_insert((at, model, counts));
                }
            }
            Usage::Snapshot(snapshot) => self.snapshot(snapshot, place),
            Usage::Uncounted => {}
        }
    }

    fn running(&mut self, current: Counts, place: Place) {
        match place {
            Place::Before => self.previous = current,
            Place::After => {}
            Place::In => {
                let delta = current.delta(&self.previous);
                self.result.entry(self.model.clone()).or_default().add(&delta);
                self.previous = current;
                self.found = true;
            }
        }
    }

    fn snapshot(&mut self, snapshot: Tokens, place: Place) {
        if snapshot.is_empty() {
            return;
        }
        match place {
            Place::Before => self.previous_snapshot = snapshot,
            Place::After => {}
            Place::In => {
                self.latest_snapshot = snapshot;
                self.found = true;
            }
        }
    }

    fn total(mut self) -> Option<Tokens> {
        for (at, model, counts) in std::mem::take(&mut self.messages).into_values() {
            if self.place(at) == Place::In {
                self.result.entry(model).or_default().add(&counts);
                self.found = true;
            }
        }
        for (model, counts) in std::mem::take(&mut self.latest_snapshot) {
            let zero = Counts::default();
            let delta = counts.delta(self.previous_snapshot.get(&model).unwrap_or(&zero));
            self.result.entry(model).or_default().add(&delta);
        }
        self.found.then_some(self.result)
    }
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

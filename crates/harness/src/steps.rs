//! A session's steps: each tool call it made, with what came back and
//! whether the harness marked that an error (sunfmin/control#747).
//!
//! Which session lines are a call or its result, and how a harness marks an
//! error, is each harness's (`Harness::steps`). This module only pairs the
//! calls with their results, and puts what each says on one line.

use serde_json::Value;

/// One tool call of a session and what came back of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The tool and what it was called with, as one line.
    pub action: String,
    /// What came back, as one line (enough to tell two results apart).
    pub result: String,
    /// Whether the harness marks what came back as an error.
    pub error: bool,
    /// When the result was recorded, seconds since the epoch, where the line says.
    pub at: Option<u64>,
}

/// What one session line says of a tool call, as its harness reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Said {
    /// A call, by the id its result names it by.
    Call { id: String, action: String },
    /// What came back of the call `id`.
    Result { id: String, result: String, error: bool, at: Option<u64> },
    /// The whole file is a child session's: none of its steps are the
    /// session's own.
    Child,
}

/// The steps `text` records, in the order their calls were made. `said` is
/// the harness's reading of one line. A call with no result yet is no step,
/// and a result whose call is not in the text is none either. A result said
/// again of the same call counts once, the first.
pub(crate) fn fold(text: &str, said: impl Fn(&Value) -> Vec<Said>) -> Vec<Step> {
    let mut calls: Vec<(String, String)> = Vec::new();
    let mut results: Vec<(String, Step)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        for one in said(&value) {
            match one {
                Said::Call { id, action } => {
                    if !calls.iter().any(|(known, _)| *known == id) {
                        calls.push((id, action));
                    }
                }
                Said::Result { id, result, error, at } => {
                    let called = calls.iter().find(|(known, _)| *known == id);
                    let answered = results.iter().any(|(known, _)| *known == id);
                    if let Some((_, action)) = called
                        && !answered
                    {
                        let step = Step { action: action.clone(), result, error, at };
                        results.push((id, step));
                    }
                }
                Said::Child => return Vec::new(),
            }
        }
    }
    calls
        .iter()
        .filter_map(|(id, _)| {
            results.iter().find(|(known, _)| known == id).map(|(_, step)| step.clone())
        })
        .collect()
}

/// `text` on one line: every run of white space one space, none at either end.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A tool's name and what it was called with, on one line. Arguments given
/// as a string of JSON, as a model writes them, are that JSON.
pub(crate) fn action(name: &str, input: &Value) -> String {
    let input = match input {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed) if parsed.is_object() || parsed.is_array() => parsed.to_string(),
            _ => text.clone(),
        },
        Value::Null => String::new(),
        other => other.to_string(),
    };
    one_line(&format!("{name} {input}"))
}

/// What came back, on one line: a string as it is, a list of content parts
/// as their texts, and anything else as its JSON.
pub(crate) fn content(value: &Value) -> String {
    match value {
        Value::String(text) => one_line(text),
        Value::Array(parts) => one_line(
            &parts
                .iter()
                .map(|part| match part {
                    Value::String(text) => text.clone(),
                    part => match part["text"].as_str() {
                        Some(text) => text.to_string(),
                        None => part.to_string(),
                    },
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Value::Null => String::new(),
        other => one_line(&other.to_string()),
    }
}

/// The exit code `text` says right after the first `before`, when that is a
/// number: `Some(1)` for `"Exit code: 1"` read after `"Exit code: "`.
pub(crate) fn exit_code(text: &str, before: &str) -> Option<i64> {
    let (_, rest) = text.split_once(before)?;
    let (sign, rest) = match rest.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", rest),
    };
    let end = rest.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(rest.len());
    let digits = rest.get(..end)?;
    format!("{sign}{digits}").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(value: &Value) -> Vec<Said> {
        let id = value["id"].as_str().unwrap_or("").to_string();
        match value["kind"].as_str() {
            Some("call") => vec![Said::Call { id, action: action("run", &value["input"]) }],
            Some("result") => vec![Said::Result {
                id,
                result: content(&value["output"]),
                error: value["failed"] == true,
                at: value["at"].as_u64(),
            }],
            _ => Vec::new(),
        }
    }

    #[test]
    fn an_exit_code_is_the_number_after_its_words() {
        assert_eq!(exit_code("Exit code: 1\nWall time", "Exit code: "), Some(1));
        assert_eq!(exit_code("x\nProcess exited with code -9\n", "exited with code "), Some(-9));
        assert_eq!(exit_code("Exit code: none", "Exit code: "), None);
        assert_eq!(exit_code("nothing", "Exit code: "), None);
    }

    #[test]
    fn a_child_sessions_file_has_no_steps() {
        let text = format!(
            "{}\n{}\n{}",
            json!({"kind": "call", "id": "a", "input": {}}),
            json!({"kind": "result", "id": "a", "output": "ok"}),
            json!({"kind": "child"})
        );
        let read = |value: &Value| match value["kind"].as_str() {
            Some("child") => vec![Said::Child],
            _ => read(value),
        };
        assert_eq!(fold(&text, read), Vec::new());
    }

    #[test]
    fn a_call_is_a_step_once_its_result_is_recorded_in_the_order_the_calls_were_made() {
        let text = [
            json!({"kind": "call", "id": "a", "input": "{\"cmd\":\n \"ls\"}"}),
            json!({"kind": "call", "id": "b", "input": {"cmd": "pwd"}}),
            json!({"kind": "call", "id": "c", "input": {"cmd": "never answered"}}),
            json!({"kind": "result", "id": "b", "output": "/wt\n", "at": 9}),
            json!({"kind": "result", "id": "a", "output": [{"type": "text", "text": "a\n  b"}],
                "failed": true}),
            json!({"kind": "result", "id": "a", "output": "again"}),
            json!({"kind": "result", "id": "z", "output": "no call"}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let steps = fold(&format!("{text}\nnot json\n"), read);
        assert_eq!(
            steps,
            vec![
                Step {
                    action: r#"run {"cmd":"ls"}"#.into(),
                    result: "a b".into(),
                    error: true,
                    at: None
                },
                Step {
                    action: r#"run {"cmd":"pwd"}"#.into(),
                    result: "/wt".into(),
                    error: false,
                    at: Some(9)
                },
            ]
        );
    }
}

//! The agent setup's guards (sunfmin/control#529): small programs of the
//! operator's, each run before the tool calls it applies to, that can refuse
//! one. The operator declares them once, in a guard list the host setting
//! `--agent-guards` names; afkconvoy owns where each harness runs its user's
//! hooks (`Harness::user_hooks`) and wires each guard there.
//!
//! Wiring merges and owns nothing else: other programs write the same file
//! (orca its hooks, the operator their settings), so only the entries whose
//! command opens with `MARK` are the kit's. Those are replaced by the
//! declared ones, appended after every other at the event, so no other
//! entry moves — Codex keys its review of a hook by its place. Every other
//! key and entry is written back as it was read, byte for byte, and a file
//! already wired as declared is not written at all.
//!
//! What a guard is given is what its harness sends a hook before a call, on
//! standard input, as JSON; both send `tool_name`, `tool_input` and `cwd`.
//! A shell call is `tool_name` `Bash` with the line in `tool_input.command`
//! on both. An edit is Claude Code's `Edit`, `Write`, `MultiEdit` or
//! `NotebookEdit` with `tool_input.file_path` (or `notebook_path`), and
//! Codex's `apply_patch` with the patch in `tool_input.command`, whose
//! `*** Add File: `, `*** Update File: ` and `*** Delete File: ` lines name
//! the files. `AFKCONVOY_HARNESS` names the harness that asks. A guard
//! refuses by exiting 2 with why on standard error, or by printing
//! `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"<why>"}}`:
//! both harnesses take either.

use std::path::Path;

use afkconvoy_core::harness::{Harness, UserHooks};
use afkconvoy_core::role::HARNESS_ENV;
use anyhow::{Context, Result};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use serde_json::value::RawValue;

/// What opens the command of every hook entry the kit wires: a shell line
/// that does nothing, by which the kit knows its own entries.
pub const MARK: &str = ": afkconvoy-agent-guard;";
/// How long a harness waits on one guard, in seconds: a guard asks `gh`.
const TIMEOUT: u64 = 20;

/// The tool calls one guard applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Calls {
    /// Those that run a shell command.
    Shell,
    /// Those that edit a file.
    Edits,
}

/// One line of a guard list: the calls it applies to and the shell command
/// that runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub calls: Calls,
    pub command: String,
}

/// Reads a guard list: one line per guard, `shell <command>` or `edits
/// <command>`, the command the rest of the line; blank lines and `#` lines
/// say nothing.
pub fn parse(text: &str) -> Result<Vec<Guard>, String> {
    let mut guards = vec![];
    for (at, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (calls, command) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let calls = match calls {
            "shell" => Calls::Shell,
            "edits" => Calls::Edits,
            other => {
                return Err(format!(
                    "line {}: `{other}` is no calls a guard applies to: `shell` or `edits`",
                    at + 1
                ));
            }
        };
        let command = command.trim();
        if command.is_empty() {
            return Err(format!("line {}: the guard names no command", at + 1));
        }
        guards.push(Guard { calls, command: command.to_string() });
    }
    Ok(guards)
}

/// One group of hooks at an event, as both harnesses write it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Group {
    matcher: String,
    hooks: Vec<Handler>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Handler {
    #[serde(rename = "type")]
    kind: String,
    command: String,
    timeout: u64,
}

/// The group that runs `guard` for `harness`, whose hooks are `hooks`.
fn group(harness: Harness, hooks: UserHooks, guard: &Guard) -> Group {
    let matcher = match guard.calls {
        Calls::Shell => hooks.shell,
        Calls::Edits => hooks.edits,
    };
    let command = format!("{MARK} export {HARNESS_ENV}={}; {}", harness.key(), guard.command);
    Group {
        matcher: matcher.to_string(),
        hooks: vec![Handler { kind: "command".into(), command, timeout: TIMEOUT }],
    }
}

/// Whether one handler of a group, as JSON, is the kit's.
fn ours(handler: &Value) -> bool {
    handler["command"].as_str().is_some_and(|command| command.starts_with(MARK))
}

/// A JSON object read with its keys in the order written, each value as it
/// was written.
struct Ordered<V>(Vec<(String, V)>);

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Ordered<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Keys<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for Keys<V> {
            type Value = Ordered<V>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = vec![];
                while let Some((key, value)) = map.next_entry()? {
                    entries.push((key, value));
                }
                Ok(Ordered(entries))
            }
        }
        deserializer.deserialize_map(Keys(std::marker::PhantomData))
    }
}

impl<V: Serialize> Serialize for Ordered<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// What is written back: what was read, as it was, or what the kit made.
#[derive(Serialize)]
#[serde(untagged)]
enum Node {
    Raw(Box<RawValue>),
    Value(Value),
    Map(Ordered<Node>),
    List(Vec<Node>),
    Group(Group),
}

/// `entries` with `key` set to `node`: in its place, or after the rest.
fn put(entries: &mut Vec<(String, Node)>, key: &str, node: Node) {
    match entries.iter_mut().find(|(at, _)| at == key) {
        Some((_, value)) => *value = node,
        None => entries.push((key.to_string(), node)),
    }
}

/// `text`, a harness's hook settings (`None`: there is no file yet), with
/// `guards` wired for `harness` → the text to write; `None` where it holds
/// them as declared already. An error is a file that is not a JSON object
/// of hooks, which is left as it is.
fn wired_text(text: Option<&str>, harness: Harness, guards: &[Guard]) -> Result<Option<String>> {
    let hooks = harness.user_hooks().context("the harness has no hook settings")?;
    let text = text.filter(|text| !text.trim().is_empty()).unwrap_or("{}");
    let top: Ordered<Box<RawValue>> =
        serde_json::from_str(text).context("it is not a JSON object")?;
    let mut top: Vec<(String, Node)> =
        top.0.into_iter().map(|(key, raw)| (key, Node::Raw(raw))).collect();
    let table = match top.iter().find(|(key, _)| key == hooks.table) {
        Some((_, Node::Raw(raw))) => {
            serde_json::from_str::<Ordered<Box<RawValue>>>(raw.get())
                .with_context(|| format!("its `{}` is not a JSON object", hooks.table))?
                .0
        }
        _ => vec![],
    };
    let mut table: Vec<(String, Node)> =
        table.into_iter().map(|(key, raw)| (key, Node::Raw(raw))).collect();
    let groups: Vec<Box<RawValue>> = match table.iter().find(|(key, _)| key == hooks.event) {
        Some((_, Node::Raw(raw))) => serde_json::from_str(raw.get())
            .with_context(|| format!("its `{}` is not a list", hooks.event))?,
        _ => vec![],
    };
    let mut was = vec![];
    let mut kept = vec![];
    let mut now = vec![];
    for raw in groups {
        let value: Value = serde_json::from_str(raw.get())?;
        was.push(value.clone());
        let handlers = value["hooks"].as_array().cloned().unwrap_or_default();
        if !handlers.iter().any(ours) {
            now.push(value);
            kept.push(Node::Raw(raw));
            continue;
        }
        let theirs: Vec<Value> = handlers.into_iter().filter(|handler| !ours(handler)).collect();
        if theirs.is_empty() {
            continue;
        }
        let mut value = value;
        value["hooks"] = Value::Array(theirs);
        now.push(value.clone());
        kept.push(Node::Value(value));
    }
    for guard in guards {
        let group = group(harness, hooks, guard);
        now.push(serde_json::to_value(&group)?);
        kept.push(Node::Group(group));
    }
    if was == now {
        return Ok(None);
    }
    put(&mut table, hooks.event, Node::List(kept));
    put(&mut top, hooks.table, Node::Map(Ordered(table)));
    let mut written = serde_json::to_string_pretty(&Ordered(top))?;
    written.push('\n');
    Ok(Some(written))
}

/// Wires `guards` into `harness`'s hook settings under `home` → whether it
/// changed the file. A harness with no hook settings is given none.
pub fn wire(home: &Path, harness: Harness, guards: &[Guard]) -> Result<bool> {
    let Some(hooks) = harness.user_hooks() else { return Ok(false) };
    let file = home.join(hooks.settings);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", file.display())),
    };
    let Some(written) = wired_text(text.as_deref(), harness, guards)
        .with_context(|| format!("cannot wire the guards into {}", file.display()))?
    else {
        return Ok(false);
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot make {}", parent.display()))?;
    }
    // Written through, not replaced: the file may be a link of the operator's.
    std::fs::write(&file, written).with_context(|| format!("cannot write {}", file.display()))?;
    Ok(true)
}

/// How `harness`'s hook settings under `home` differ from `guards`: each
/// declared guard not wired there, and each wired one the list does not
/// name. Nothing for a harness with no hook settings.
pub fn unwired(home: &Path, harness: Harness, guards: &[Guard]) -> Vec<String> {
    let Some(hooks) = harness.user_hooks() else { return vec![] };
    let shown = format!("~/{}", hooks.settings);
    let value: Value = match std::fs::read_to_string(home.join(hooks.settings)) {
        Err(_) => Value::Null,
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => return vec![format!("{shown} cannot be read as JSON: {error}")],
        },
    };
    let groups = value[hooks.table][hooks.event].as_array().cloned().unwrap_or_default();
    let wired: Vec<(String, String)> = groups
        .iter()
        .flat_map(|group| {
            let matcher = group["matcher"].as_str().unwrap_or_default().to_string();
            let handlers = group["hooks"].as_array().cloned().unwrap_or_default();
            handlers.into_iter().filter(ours).map(move |handler| {
                (matcher.clone(), handler["command"].as_str().unwrap_or_default().to_string())
            })
        })
        .collect();
    let declared: Vec<(&Guard, (String, String))> = guards
        .iter()
        .map(|guard| {
            let group = group(harness, hooks, guard);
            (guard, (group.matcher, group.hooks[0].command.clone()))
        })
        .collect();
    let mut said = vec![];
    let missing: Vec<&str> = declared
        .iter()
        .filter(|(_, pair)| !wired.contains(pair))
        .map(|(guard, _)| guard.command.as_str())
        .collect();
    if !missing.is_empty() {
        said.push(format!("{shown} is missing guards: {}", missing.join(", ")));
    }
    let extra: Vec<&str> = wired
        .iter()
        .filter(|pair| !declared.iter().any(|(_, declared)| declared == *pair))
        .map(|(_, command)| command.as_str())
        .collect();
    if !extra.is_empty() {
        said.push(format!("{shown} holds guards the list does not name: {}", extra.join(", ")));
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(command: &str) -> Guard {
        Guard { calls: Calls::Shell, command: command.into() }
    }

    #[test]
    fn a_guard_list_names_the_calls_and_the_command() {
        let text = "# guards\n\nshell bash \"$HOME/guard-branch-delete.sh\"\nedits  ./no-edit.sh --strict\n";
        assert_eq!(
            parse(text).unwrap(),
            vec![
                shell("bash \"$HOME/guard-branch-delete.sh\""),
                Guard { calls: Calls::Edits, command: "./no-edit.sh --strict".into() },
            ]
        );
        assert!(parse("files x\n").unwrap_err().contains("line 1"));
        assert!(parse("shell\n").unwrap_err().contains("no command"));
    }

    #[test]
    fn wiring_appends_its_own_after_the_rest_and_a_second_wiring_changes_nothing() {
        let before = "{\n  \"zeta\": {\"b\": 1, \"a\": 2},\n  \"hooks\": {\n    \"Stop\": [{\"hooks\": []}],\n    \"PreToolUse\": [\n      {\"matcher\": \"*\", \"hooks\": [{\"type\": \"command\", \"command\": \"orca\"}]}\n    ]\n  },\n  \"alpha\": true\n}\n";
        let guards = [shell("guard.sh")];
        let after = wired_text(Some(before), Harness::Codex, &guards).unwrap().unwrap();
        // Every other key and entry is written as it was read, in its place.
        assert!(after.find("\"zeta\"").unwrap() < after.find("\"hooks\"").unwrap(), "{after}");
        assert!(after.contains("{\"b\": 1, \"a\": 2}"), "{after}");
        assert!(after.contains("[{\"hooks\": []}]"), "{after}");
        assert!(after.contains(
            "{\"matcher\": \"*\", \"hooks\": [{\"type\": \"command\", \"command\": \"orca\"}]}"
        ));
        let value: Value = serde_json::from_str(&after).unwrap();
        let groups = value["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1]["matcher"], "Bash");
        assert_eq!(
            groups[1]["hooks"][0]["command"],
            ": afkconvoy-agent-guard; export AFKCONVOY_HARNESS=codex; guard.sh"
        );
        assert_eq!(wired_text(Some(&after), Harness::Codex, &guards).unwrap(), None);
        // A guard dropped from the list is unwired; another's entry stays.
        let unwired = wired_text(Some(&after), Harness::Codex, &[]).unwrap().unwrap();
        let value: Value = serde_json::from_str(&unwired).unwrap();
        assert_eq!(value["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(value["alpha"], true);
    }

    #[test]
    fn a_handler_of_the_kits_in_anothers_group_is_taken_out_and_the_rest_kept() {
        let before = format!(
            r#"{{"hooks":{{"PreToolUse":[{{"matcher":"Bash","hooks":[{{"type":"command","command":"theirs"}},{{"type":"command","command":"{MARK} old"}}]}}]}}}}"#
        );
        let after = wired_text(Some(&before), Harness::ClaudeCode, &[]).unwrap().unwrap();
        let value: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            value["hooks"]["PreToolUse"],
            serde_json::json!([{"matcher": "Bash", "hooks": [{"type": "command", "command": "theirs"}]}])
        );
    }

    #[test]
    fn no_file_is_made_with_the_guards_and_a_file_not_of_json_is_left_alone() {
        let edits = Guard { calls: Calls::Edits, command: "no-edit".into() };
        let made = wired_text(None, Harness::ClaudeCode, &[edits]).unwrap().unwrap();
        let value: Value = serde_json::from_str(&made).unwrap();
        assert_eq!(value["hooks"]["PreToolUse"][0]["matcher"], "Edit|Write|MultiEdit|NotebookEdit");
        assert_eq!(wired_text(None, Harness::ClaudeCode, &[]).unwrap(), None, "nothing to wire");
        assert!(wired_text(Some("not json"), Harness::Codex, &[shell("x")]).is_err());
        assert!(wired_text(Some("[]"), Harness::Codex, &[shell("x")]).is_err());
    }

    #[test]
    fn unwired_names_a_guard_missing_and_one_the_list_does_not_name() {
        let home = tempfile::tempdir().unwrap();
        let guards = [shell("guard.sh")];
        let said = unwired(home.path(), Harness::Codex, &guards);
        assert_eq!(said, vec!["~/.codex/hooks.json is missing guards: guard.sh".to_string()]);
        assert!(wire(home.path(), Harness::Codex, &guards).unwrap());
        assert!(unwired(home.path(), Harness::Codex, &guards).is_empty());
        assert!(!wire(home.path(), Harness::Codex, &guards).unwrap(), "wired already");
        let said = unwired(home.path(), Harness::Codex, &[]);
        assert!(said[0].contains("does not name"), "{said:?}");
        assert!(unwired(home.path(), Harness::Pi, &guards).is_empty(), "pi has no hook settings");
    }
}

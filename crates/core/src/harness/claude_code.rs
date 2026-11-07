//! Claude Code, as a harness: everything of a worker's start, and of the
//! guard's asking, that is Claude Code's own. It loads a role's rules, skills
//! and hooks from the directory it starts in (docs/adr/0009), so what the
//! fleet adds is the rest: the directories beside the role's, the settings it
//! may load, and the hooks that ask the guard and tell the worker backend.
//! The host user's skills and rules (docs/adr/0017) are one more directory
//! beside the role's: one the host keeps, which holds nothing else of the
//! user's, so that leaving their settings out still leaves out their hooks.

use std::collections::BTreeMap;

use flux_rs::attrs::*;
use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::{Held, Kept, Link, SessionFile, SessionLayout};
use crate::account::{Counts, Keys, Usage};
use crate::guard::Call;
use crate::line::Line;
use crate::role::Start;
use crate::turn::{Marker, Note, Turn};

pub(super) const NAME: &str = "Claude Code";
pub(super) const KEY: &str = "claude-code";
pub(super) const PROGRAM: &str = "claude";
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".claude/CLAUDE.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("claude-code");
/// The setting that answers the warning Claude Code opens with when it is
/// started to work with nobody at its terminal: a worker has nobody to accept
/// it, and stops there. On a host whose user accepted it once, their settings
/// say the same. Written as a member of a JSON object.
macro_rules! warning_accepted {
    () => {
        "\"skipDangerousModePermissionPrompt\":true"
    };
}
/// Where Claude Code reads its user's hooks: their settings, which a
/// session they start by hand, and a worker outside a role, loads. Its tools
/// that edit a file are each named.
pub(super) const USER_HOOKS: super::UserHooks = super::UserHooks {
    settings: ".claude/settings.json",
    table: "hooks",
    event: "PreToolUse",
    shell: "Bash",
    edits: "Edit|Write|MultiEdit|NotebookEdit",
};
/// Claude Code with the flag that lets it work with nobody at its terminal,
/// and the warning it opens with under that flag answered. Each worker adds
/// native session trust as well. Of two `--settings` Claude Code takes
/// the last, whole: a worker in its role is started on the fleet's (`added`).
pub(super) const STOCK: &str =
    concat!("claude --dangerously-skip-permissions --settings '{", warning_accepted!(), "}'");
/// The variable Claude Code reads a custom provider's address from.
pub(super) const PROVIDER: &str = "ANTHROPIC_BASE_URL";
/// Claude Code's native session trust. Permission bypass and its warning
/// setting do not accept workspace trust, even when both settings are given.
/// This does not write a trust entry in the host user's config.
pub(super) const TRUST: &str = "CLAUDE_CODE_SANDBOXED=1";
/// Its worktree trust outside a role is the permission-bypass and settings
/// flags `in_worktree` adds: a launch checks the command takes `FLAGS`.
pub(super) const TRUST_IS_FLAGS: bool = true;
const BYPASS: &str = "--dangerously-skip-permissions";

/// Starts Claude Code with its worktree trusted and its unattended warning
/// answered, without replacing a setting the stock command already carries.
pub(super) fn in_worktree(command: &str) -> String {
    let mut command = command.to_string();
    if !command.contains(BYPASS) {
        command.push_str(&format!(" {BYPASS}"));
    }
    if !command.contains("skipDangerousModePermissionPrompt") {
        command.push_str(&format!(" --settings '{{{}}}'", warning_accepted!()));
    }
    if command.contains(TRUST) {
        command
    } else {
        Line::new().fixed("export ").fixed(TRUST).fixed(" && ").launch(&command).done()
    }
}

/// Starts one watch interactively, with the same unattended trust setup.
pub(super) fn watch(command: &str, model: Option<&str>) -> String {
    let line = Line::new().launch(&in_worktree(command));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Adds the fleet's hooks without changing project or user rule discovery.
pub(super) fn guarded(command: &str, guard: &str) -> String {
    Line::new()
        .launch(command)
        .fixed(" --settings ")
        .word(&added_settings(guard, REPORT, &[]))
        .done()
}

/// Claude Code's flag for a directory added to the one it was started in.
const ADD_DIR: &str = "--add-dir";
/// Claude Code's flag for which settings it loads.
const SETTING_SOURCES: &str = "--setting-sources";
/// Claude Code's flag for the model of the session.
pub(super) const MODEL: &str = "--model";
/// Claude Code's flag for settings added to those it loads.
const SETTINGS: &str = "--settings";
/// Every flag the fleet adds: what a worker launch command that starts
/// Claude Code must take.
pub(super) const FLAGS: [&str; 4] = [ADD_DIR, SETTING_SOURCES, SETTINGS, MODEL];
/// What Claude Code is asked for the flags it takes.
const HELP: &str = " --help";

/// The line that sends `model` the one-line `prompt` through `command`, a
/// worker launch command of Claude Code's, and prints its answer. Claude
/// Code keeps its logins with the host's user, as a worker finds them.
pub(super) fn prompt_question(command: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" --print ").word(prompt).done()
}

/// The control request Claude Code answers with, among the rest of what it
/// offers, the models its login runs: sent before any prompt, it is the only
/// thing sent, and no prompt follows.
const INITIALIZE: &str =
    r#"{"type":"control_request","request_id":"models","request":{"subtype":"initialize"}}"#;

/// How Claude Code is started to read control requests from its input and
/// answer them on its output, a JSON object a line.
const STREAMED: &str = " -p --input-format stream-json --output-format stream-json --verbose";

/// What comes before the list of models in Claude Code's answer to
/// `INITIALIZE`, with no whitespace: a `"` inside a JSON string is written
/// `\"`, so this is in the answer only where the key is.
const MODELS: &str = "\"models\":[";

/// What comes before each model's name in that list: the key of it.
const NAMED: &str = "\"value\":\"";

/// The line that asks `command`, a worker launch command of Claude Code's,
/// which models its login runs: `INITIALIZE`, on its input, and nothing else.
/// Claude Code keeps its logins with the host's user, as a worker finds them.
pub(super) fn models_question(command: &str) -> String {
    Line::new()
        .fixed("printf '%s\\n' ")
        .word(INITIALIZE)
        .fixed(" | ")
        .launch(command)
        .fixed(STREAMED)
        .done()
}

/// The models Claude Code lists in `answer`, what it printed for
/// `models_question`, each by the name its `MODEL` flag takes, in the order
/// listed: the `value` of each model there, whatever a shell printed around
/// it or wherever a terminal broke its lines. `None` when it lists none: it
/// said something else.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let dense: String = answer.split_whitespace().collect();
    let (_, list) = dense.split_once(MODELS)?;
    // The models are the objects one level into the list, which ends at the
    // `]` that closes it; a `[`, `]`, `{` or `}` inside a string is none.
    let mut listed = vec![];
    let (mut depth, mut quoted, mut escaped) = (1_usize, false, false);
    for (at, character) in list.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => {
                if !quoted && depth == 2 {
                    let named =
                        list[at..].strip_prefix(NAMED).and_then(|after| after.split_once('"'));
                    listed.extend(named.map(|(name, _)| name.to_string()));
                }
                quoted = !quoted;
            }
            _ if quoted => {}
            '[' | '{' => depth = depth.saturating_add(1),
            ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    listed.retain(|name| !name.is_empty());
    if listed.is_empty() { None } else { Some(listed) }
}

/// What Claude Code's probe adds: its answer as one JSON object, whose
/// `modelUsage` is keyed by the model that answered.
const AS_JSON: &str = " --output-format json";

/// What comes before the first model's id in that answer, with no
/// whitespace.
const USAGE: &str = "\"modelUsage\":{\"";

/// The line that probes `model` with `prompt` through `command`, as
/// `prompt_question` sends it, answering in JSON (`answered_as`).
pub(super) fn probe_question(command: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" --print ").word(prompt).fixed(AS_JSON).done()
}

/// The model that answered `probe_question`, by `answer`: the first key of
/// its `modelUsage`, whatever a terminal broke its lines at.
pub(super) fn answered_as(answer: &str) -> Option<String> {
    let dense: String = answer.split_whitespace().collect();
    let (_, after) = dense.split_once(USAGE)?;
    let (id, _) = after.split_once('"')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// What comes before a model's display name in Claude Code's answer to
/// `INITIALIZE`.
const SHOWN: &str = "\"displayName\":\"";

/// The display name of each model Claude Code lists in `answer` (`models`),
/// by its name: what its menu shows it as. A terminal's line breaks are
/// taken out, and the spaces a display name holds kept.
pub(super) fn shown(answer: &str) -> Vec<(String, String)> {
    let joined: String = answer.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    let Some((_, list)) = joined.split_once("\"models\":") else { return vec![] };
    // Each model is an object one level into the list, whichever order its
    // keys are in; a bracket inside a string is none.
    let mut shown = vec![];
    let (mut depth, mut quoted, mut escaped, mut opened) = (0_usize, false, false, 0);
    for (at, character) in list.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if quoted => {}
            '[' | '{' => {
                depth = depth.saturating_add(1);
                if depth == 2 {
                    opened = at;
                }
            }
            ']' | '}' => {
                if depth == 2 {
                    let model = &list[opened..at];
                    let value = |key: &str| {
                        let (_, after) = model.split_once(key)?;
                        after.split_once('"').map(|(value, _)| value.to_string())
                    };
                    if let (Some(name), Some(display)) = (value(NAMED), value(SHOWN))
                        && !name.is_empty()
                        && !display.is_empty()
                    {
                        shown.push((name, display));
                    }
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    shown
}

/// The settings a worker loads: those of the directory it starts in — its
/// role's — and nothing of the host user's: no hook, no MCP server, no
/// permission of theirs.
const SOURCES: &str = "project";

/// Claude Code also walks `.claude` directories above its role as project
/// rules: `--setting-sources project` alone does not exclude the host user's
/// rules when the role's copy is under their home (sunfmin/control#107).
pub(super) fn host_rule_excludes(home: &str) -> Vec<String> {
    let home = home.trim_end_matches('/');
    vec![format!("{home}/.claude/CLAUDE.md"), format!("{home}/.claude/rules/**")]
}
/// The event of Claude Code before each tool call: where the guard is asked.
const BEFORE_A_CALL: &str = "PreToolUse";
/// The events of Claude Code at which a worker's state changes: what the
/// worker backend hears of, to report it (`REPORT`). With the user's
/// settings left out, nothing else tells it.
const STATE_EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "SubagentStart",
    "SubagentStop",
    "TeammateIdle",
    "PostCompact",
    "Stop",
    "StopFailure",
    "SessionEnd",
];
/// How long Claude Code waits on one report, in seconds: a worker backend
/// that does not answer holds no tool call up.
const REPORT_TIMEOUT: u64 = 10;
/// Has Claude Code load the rules of the directories added to it: the repo's
/// own, in the worktree, and the team rules, in the team's copy. Their hooks
/// are not loaded: what a repo must enforce belongs in its gate.
const ADDED_RULES: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";
/// Keeps Claude Code from carrying what one worker learned to the next on
/// this host: a role is what its repo says, the same on every host.
const NO_MEMORY: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";
/// The variables a worker's environment sets for Claude Code.
pub(super) const ENVIRONMENT: [&str; 3] = [ADDED_RULES, NO_MEMORY, "CLAUDE_CODE_SANDBOXED"];

/// What a host keeps for Claude Code: the host user's skills and rules, as
/// a directory added to a worker's holds them, and nothing else. Claude Code
/// loads the skills and the rules of an added directory, and not its
/// settings; a skill of the role's wins over one of the same name there.
pub(super) const KEPT: Kept = Kept {
    links: &[],
    host_user: &[
        Link { home: ".claude/skills", kept: ".claude/skills" },
        Link { home: ".claude/CLAUDE.md", kept: ".claude/CLAUDE.md" },
    ],
    files: &[],
};

/// The shell line that tells orca of a worker's state: the hook orca installs
/// on its host for Claude Code, which reads the event on standard input and
/// knows its terminal from the environment orca gave it. orca puts the same
/// line in the user's settings, which a worker in its role does not load. On
/// a host where orca installed none, nothing is told.
const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/claude-hook.sh\"; if [ -x \"$hook\" ]; \
     then ORCA_HOOK_AGENT_PID=\"$PPID\" /bin/sh \"$hook\"; else cat >/dev/null; fi";

/// The tool Claude Code runs shell lines with.
const SHELL_TOOL: &str = "Bash";
/// The exit code that tells Claude Code a hook refuses the call.
pub(super) const REFUSED: u8 = 2;

/// Where a role's limits for Claude Code are written, in its directory: the
/// settings Claude Code loads from the directory it starts in.
pub(super) const LIMITS: &str = ".claude/settings.json";

/// A role's settings, as far as they say whether they limit a worker.
#[derive(Deserialize)]
struct Settings {
    /// Each event of Claude Code's, with the hooks it runs at it.
    #[serde(default)]
    hooks: BTreeMap<String, Vec<IgnoredAny>>,
    #[serde(default)]
    permissions: Permissions,
}

#[derive(Default, Deserialize)]
struct Permissions {
    /// What a session may never do, whatever it is started with.
    #[serde(default)]
    deny: Vec<IgnoredAny>,
}

/// Whether what a role's directory holds at `LIMITS` limits a worker: a file
/// of settings that names a hook at some event, or denies a permission. One
/// that names neither — a skeleton's — holds no limit.
pub(super) fn limited<'de, D: Deserializer<'de>>(held: Held<D>) -> Result<bool, D::Error> {
    match held {
        Held::Nothing => Ok(false),
        Held::Entries(_) => Err(D::Error::custom("it is a directory, not a file of settings")),
        Held::File(settings) => {
            let settings = Settings::deserialize(settings)?;
            let hooked = settings.hooks.values().any(|hooks| !hooks.is_empty());
            Ok(hooked || !settings.permissions.deny.is_empty())
        }
    }
}

/// `text` as a JSON string, quotes and all.
fn json_text(text: &str) -> String {
    let mut json = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                json.push('\\');
                json.push(c);
            }
            // A shell line holds no control character worth carrying.
            c if c.is_control() => json.push(' '),
            c => json.push(c),
        }
    }
    json.push('"');
    json
}

/// The settings added to a worker's: the guard, a hook Claude Code runs
/// before every tool call with the line `guard`, the report, a hook it
/// runs with the line `report` wherever the worker's state changes, and the
/// answer to the warning it would stop at, whatever command starts it. They are
/// added to the role's own, so a role's hooks run beside them and no role is
/// without the guard (docs/adr/0012) — or unheard of by the worker backend,
/// which would read a worker it has no report of off its terminal alone.
/// `excluded` keeps host-user rule paths out of native ancestor discovery
/// when the role has turned host-user rules off.
fn added_settings(guard: &str, report: &str, excluded: &[String]) -> String {
    let guard = format!("{{\"type\":\"command\",\"command\":{}}}", json_text(guard));
    let report = format!(
        "{{\"type\":\"command\",\"command\":{},\"timeout\":{REPORT_TIMEOUT}}}",
        json_text(report)
    );
    let mut settings =
        format!("{{\"hooks\":{{\"{BEFORE_A_CALL}\":[{{\"hooks\":[{guard},{report}]}}]");
    for event in STATE_EVENTS {
        settings.push_str(&format!(",\"{event}\":[{{\"hooks\":[{report}]}}]"));
    }
    settings.push_str(concat!("},", warning_accepted!()));
    if !excluded.is_empty() {
        let paths = excluded.iter().map(|path| json_text(path)).collect::<Vec<_>>().join(",");
        settings.push_str(&format!(",\"claudeMdExcludes\":[{paths}]"));
    }
    settings.push('}');
    settings
}

/// `line`, which starts the worker launch command, with what Claude Code is
/// given to put a worker in its role: the worktree and the team's copy added,
/// and the host user's skills and rules where it is given them; the user's
/// settings left out, the guard and the report added, and the stage's model.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    let excluded = if start.host_user { &[][..] } else { start.host_rules };
    let line = line
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.team_dir);
    let line = if start.host_user {
        line.fixed(" ").fixed(ADD_DIR).fixed(" ").word(start.kept)
    } else {
        line
    };
    let line = line
        .fixed(" ")
        .fixed(SETTING_SOURCES)
        .fixed(" ")
        .fixed(SOURCES)
        .fixed(" ")
        .fixed(SETTINGS)
        .fixed(" ")
        .word(&added_settings(start.guard, REPORT, excluded));
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

/// `line`, which starts the worker launch command, asking it which flags it
/// takes instead of starting a session.
#[spec(fn(Line[@l]) -> Line[l])]
pub(super) fn asked_its_flags(line: Line) -> Line {
    line.fixed(HELP)
}

/// One tool call, as Claude Code describes it to a hook before making it.
#[derive(Deserialize)]
struct Described {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    /// The shell line, when the tool runs one.
    #[serde(default)]
    command: Option<String>,
}

/// The tool call `described` to a hook, as the guard judges one.
pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.tool_input.command {
        Some(line) if described.tool_name == SHELL_TOOL => Call::Shell(line),
        _ => Call::Other,
    })
}

/// The line Claude Code waits for the next prompt at: its footer is below it.
const PROMPT: char = '❯';
/// What Claude Code's footer counts of the work a session runs in the
/// background, as `<n> <what>` between its `·`s: `1 shell, 1 monitor`.
const BACKGROUND: [&str; 6] =
    ["shell", "shells", "monitor", "monitors", "background task", "background tasks"];

/// Whether the end of a Claude Code worker's screen shows work it started
/// itself still running: its footer — the lines below the last prompt, or
/// the last line where no prompt shows — counts a shell or a monitor. What
/// a turn's end said ran, above the prompt, is not read: that work may have
/// ended since.
pub(super) fn shows_background(screen: &[String]) -> bool {
    let mut below = screen.iter();
    match screen.iter().rposition(|line| line.trim_start().starts_with(PROMPT)) {
        Some(prompt) => {
            below.nth(prompt);
        }
        None => {
            let last = screen.iter().rposition(|line| !line.trim().is_empty());
            if let Some(last) = last.and_then(|last| last.checked_sub(1)) {
                below.nth(last);
            }
        }
    }
    below.any(|line| {
        line.split(['·', ',']).any(|part| match part.trim().split_once(' ') {
            Some((count, what)) => {
                !count.is_empty()
                    && count.bytes().all(|b| b.is_ascii_digit())
                    && count != "0"
                    && BACKGROUND.contains(&what.trim())
            }
            None => false,
        })
    })
}

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".claude";

/// Sessions are filed under the directory Claude Code starts in, which is
/// the role copy for a bound worker. The session identifies its worktree.
pub(super) fn session_layout(_cwd: &str) -> SessionLayout {
    SessionLayout::Tree { dir: "projects".into(), extension: "jsonl" }
}

/// Notes typed at an unbound worker's prompt stay in its project directory.
pub(super) fn note_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Files { dir: format!("projects/{}", dir_name(cwd)), extension: "jsonl" }
}

/// `/Users/sunfmin/Developments/chessmirror` → `-Users-sunfmin-Developments-chessmirror`.
fn dir_name(cwd: &str) -> String {
    session::stored_name(cwd.replace('/', "-"), cwd)
}

/// The turn for `cwd`, or for the worker its initial brief pointer names
/// when Claude Code starts in a role copy. A sidechain is a child session.
pub(super) fn recorded_turn(
    cwd: &str,
    text: &str,
    not_before: Option<u64>,
    brief: Option<&str>,
) -> Option<Turn> {
    let named = brief.is_some_and(|brief| names_brief(text, brief));
    let here = json_lines(text).any(|value| {
        value["isSidechain"] != true
            && value["cwd"].as_str().is_some_and(|recorded| session::same_cwd(recorded, cwd))
    });
    if !here && !named {
        return None;
    }
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(cwd, &value, named)).collect();
    session::turn_of(&notes, not_before)
}

/// The first primary user message must be this worker's brief pointer.
/// Tool results, queued input, replies and later mentions cannot associate
/// another worker's session. The pointer match is shared with Codex.
pub(super) fn names_brief(text: &str, brief: &str) -> bool {
    for value in json_lines(text) {
        if value["isSidechain"] == true {
            continue;
        }
        match value["type"].as_str() {
            Some("user") => return session::brief_match(&value, brief).unwrap_or(false),
            Some("assistant") => return false,
            _ => {}
        }
    }
    false
}

fn note(cwd: &str, value: &serde_json::Value, named: bool) -> Option<Note> {
    if !named && value["cwd"].as_str().is_some_and(|recorded| !session::same_cwd(recorded, cwd)) {
        return None;
    }
    let child = value["isSidechain"] == true;
    let at = session::parse_iso(value["timestamp"].as_str()?)?;
    match value["type"].as_str()? {
        // A compaction's summary is written as the user's, but nobody asked
        // anything: compacted while idle, the session stays at its prompt.
        "user" if value["isCompactSummary"] == true => None,
        "user" => Some(Note { marker: Marker::Start, at, child }),
        "assistant" if value["message"]["stop_reason"] == "end_turn" => {
            Some(Note { marker: Marker::End, at, child })
        }
        _ => None,
    }
}

/// The line a stand-in writes for `line`, typed while the worker worked: a
/// `queued_command` attachment once its model has it, else only the
/// `queue-operation` that holds it in the input queue.
pub(super) fn note_files(cwd: &str, line: &str, taken: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let record = if taken {
        session::object(vec![
            ("type", "attachment".into()),
            ("timestamp", iso.clone().into()),
            ("cwd", cwd.into()),
            ("isSidechain", false.into()),
            (
                "attachment",
                session::object(vec![
                    ("type", "queued_command".into()),
                    ("prompt", line.into()),
                    ("commandMode", "prompt".into()),
                    ("timestamp", iso.into()),
                ]),
            ),
        ])
    } else {
        session::object(vec![
            ("type", "queue-operation".into()),
            ("operation", "enqueue".into()),
            ("timestamp", iso.into()),
            ("content", line.into()),
        ])
    };
    vec![SessionFile {
        path: format!("projects/{}/afkconvoy-note.jsonl", dir_name(cwd)),
        body: format!("{record}\n"),
        compressed: false,
        replaces: None,
    }]
}

/// One session file a stand-in writes for this turn.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let user = session::object(vec![
        ("type", "user".into()),
        ("timestamp", iso.clone().into()),
        ("cwd", cwd.into()),
        ("isSidechain", false.into()),
        ("message", session::object(vec![("role", "user".into()), ("content", "brief".into())])),
    ]);
    let mut body = format!("{user}\n");
    if !open {
        let end = session::object(vec![
            ("type", "assistant".into()),
            ("timestamp", iso.into()),
            ("cwd", cwd.into()),
            ("isSidechain", false.into()),
            (
                "message",
                session::object(vec![
                    ("role", "assistant".into()),
                    ("stop_reason", "end_turn".into()),
                ]),
            ),
        ]);
        body.push_str(&format!("{end}\n"));
    }
    vec![SessionFile {
        path: format!("projects/{}/afkconvoy-turn.jsonl", dir_name(cwd)),
        body,
        compressed: false,
        replaces: None,
    }]
}

/// The names Claude Code gives the counts of a message's usage.
const USAGE_KEYS: Keys = Keys {
    input: "input_tokens",
    cache_read: "cache_read_input_tokens",
    cache_write: "cache_creation_input_tokens",
    output: "output_tokens",
    reasoning: "reasoning_tokens",
};

/// What one Claude Code session line says of usage: each assistant message,
/// keyed by its id, as a streamed message repeats it. Thinking, where stated,
/// is its reasoning.
pub(super) fn usage(value: &serde_json::Value, line: usize) -> Option<Usage> {
    let message = &value["message"];
    if value["type"] != "assistant" || message["role"] != "assistant" {
        return None;
    }
    let usage = &message["usage"];
    if !usage.is_object() {
        return None;
    }
    let Some(mut counts) = Counts::read(usage, &USAGE_KEYS, false) else {
        return Some(Usage::Uncounted);
    };
    counts.reasoning =
        usage["output_tokens_details"]["thinking_tokens"].as_u64().or(counts.reasoning);
    Some(Usage::Message {
        key: message_key(value, line),
        model: message["model"].as_str().unwrap_or("").to_string(),
        counts,
    })
}

/// The message's id, else the line's, else its line number.
pub(super) fn message_key(value: &serde_json::Value, line: usize) -> String {
    value["message"]["id"]
        .as_str()
        .or_else(|| value["id"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("line:{line}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::line::quoted;
    use crate::role::{Facts, On, Told};

    #[test]
    fn only_the_first_primary_prompt_can_name_a_role_workers_brief() {
        let harness = Harness::ClaudeCode;
        let brief = "/repo/.git/worktrees/issue-7/convoy-worker-prompt.md";
        let pointer = crate::prompt::brief_pointer(brief);
        let user = |content: serde_json::Value| {
            serde_json::json!({
            "type": "user", "isSidechain": false, "message": {"role": "user", "content": content}
        }).to_string()
        };
        let prompt = user(pointer.clone().into());
        assert!(harness.names_brief(&prompt, brief));
        let parts = user(serde_json::json!([{"type": "text", "text": pointer}]));
        assert!(harness.names_brief(&parts, brief));
        for other in [
            "/repo/.git/worktrees/issue-70/convoy-worker-prompt.md",
            "/elsewhere/repo/.git/worktrees/issue-7/convoy-worker-prompt.md",
        ] {
            let other = user(crate::prompt::brief_pointer(other).into());
            assert!(!harness.names_brief(&format!("{other}\n{prompt}"), brief));
        }
        let tool = user(serde_json::json!([{"type": "tool_result", "content": pointer}]));
        let assistant = serde_json::json!({"type": "assistant", "message": {
            "role": "assistant", "content": pointer}})
        .to_string();
        let queued = serde_json::json!({"type": "queue-operation", "content": pointer}).to_string();
        let child = prompt.replace("\"isSidechain\":false", "\"isSidechain\":true");
        for non_prompt in [tool, assistant, queued, child] {
            assert!(!harness.names_brief(&non_prompt, brief), "{non_prompt}");
        }
        let earlier = user("an earlier task".into());
        assert!(!harness.names_brief(&format!("{earlier}\n{prompt}"), brief));
        let unstamped = serde_json::json!({"type": "user", "message": {
            "role": "user", "content": pointer}})
        .to_string();
        assert_eq!(
            harness.recorded_turn("/worktree", &unstamped, Some(1), Some(brief)),
            None,
            "a matching pointer without a timestamp is no turn"
        );
    }

    #[test]
    fn folder_trust_is_session_configuration_beside_permission_bypass_and_both_settings() {
        let outside = Harness::ClaudeCode.in_worktree(STOCK, "/new/worktree", "");
        assert!(outside.starts_with("export CLAUDE_CODE_SANDBOXED=1 && "), "{outside}");
        assert_eq!(in_worktree(&outside), outside, "trust setup is idempotent");
        for host_user in [false, true] {
            let start = Start { host_user, ..start(None) };
            let role = in_role(STOCK, &start);
            assert!(role.contains(" CLAUDE_CODE_SANDBOXED=1 "), "{role}");
            assert_eq!(role.matches("--settings ").count(), 2, "{role}");
            assert!(role.contains("--setting-sources project"), "{role}");
            assert!(role.contains("/bin/afkconvoy guard"), "{role}");
        }
    }

    fn in_role(command: &str, start: &Start) -> String {
        Harness::ClaudeCode.in_role(command, start)
    }

    fn flags_question(command: &str) -> String {
        Harness::ClaudeCode.flags_question(command)
    }

    fn takes_flags(command: &str, answer: Option<&str>) -> Result<(), String> {
        Harness::ClaudeCode.takes(command, answer)
    }

    /// The settings that add the guard `/bin/afkconvoy guard` and the report.
    fn added() -> String {
        let report =
            format!(r#"{{"type":"command","command":{},"timeout":10}}"#, json_text(REPORT));
        let mut added = format!(
            r#"{{"hooks":{{"PreToolUse":[{{"hooks":[{{"type":"command","command":"/bin/afkconvoy guard"}},{report}]}}]"#
        );
        for event in STATE_EVENTS {
            added.push_str(&format!(r#","{event}":[{{"hooks":[{report}]}}]"#));
        }
        added + r#"},"skipDangerousModePermissionPrompt":true}"#
    }

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model,
            guard: "/bin/afkconvoy guard",
            kept: "/cache/harness/claude-code",
            host_user: false,
            dsh_profile: crate::team::DshProfile::Fleet,
            host_rules: &[],
            shadowed: &[],
            told: Told::default(),
            facts: Facts {
                on: On::Stage { issue: 7, flow: "feature", stage: "build" },
                role: "dev",
                skill: "implement",
                writes: false,
                consult: false,
                base_branch: "main",
                target_branch: None,
                escalate_label: "ready-for-human",
                lock_label: "fleet-lock",
                role_commit: "abc",
                team_commit: "def",
            },
        }
    }

    #[test]
    fn the_line_changes_to_the_role_first_and_adds_a_fixed_set() {
        let line = in_role("kim --yolo", &start(Some("opus")));
        assert_eq!(
            line,
            "cd '/copies/acme/dev/abc' && export CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 \
             CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 CLAUDE_CODE_SANDBOXED=1 \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' \
             AFKCONVOY_HARNESS='claude-code' AFKCONVOY_MODEL='opus' && \
             kim --yolo --add-dir '/wt/issue 7' --add-dir '/copies/acme/team/def' \
             --setting-sources project --settings '"
                .to_string()
                + &added()
                + "' --model 'opus'"
        );
    }

    #[test]
    fn the_line_has_claude_code_load_the_team_rules() {
        // They are the `CLAUDE.md` of the team's copy: the directory is
        // added, and the rules of added directories are on — for every
        // worker, whatever it is on.
        for on in [On::Stage { issue: 7, flow: "feature", stage: "land" }, On::Patrol("sweep")] {
            let mut start = start(None);
            start.facts.on = on;
            let line = in_role("claude", &start);
            let (environment, started) = line.split_once(" && claude ").unwrap();
            assert!(environment.contains(&format!(" export {ADDED_RULES}=1 ")), "{line}");
            assert!(started.contains(&format!("{ADD_DIR} '{}' ", start.team_dir)), "{line}");
        }
    }

    #[test]
    fn a_stage_with_no_model_names_none() {
        let line = in_role("claude", &start(None));
        assert!(
            line.ends_with(&format!("--setting-sources project --settings '{}'", added())),
            "{line}"
        );
        assert!(!line.contains("--model"), "{line}");
    }

    #[test]
    fn nothing_a_path_or_a_name_holds_is_read_by_the_shell() {
        let odd = Start { role_dir: "/it's here/$HOME", ..start(Some("a b;c")) };
        let line = in_role("claude", &odd);
        assert!(line.starts_with("cd '/it'\\''s here/$HOME' && "), "{line}");
        assert!(line.ends_with("--model 'a b;c'"), "{line}");
        // Nor by Claude Code, of the line that runs the guard.
        let odd = Start { guard: "/it's \"here\"/afk\\convoy guard", ..start(None) };
        let settings = added_settings(odd.guard, r#"tell "$HOME""#, &[]);
        assert!(settings.contains(r#""command":"/it's \"here\"/afk\\convoy guard""#), "{settings}");
        assert!(settings.contains(r#""command":"tell \"$HOME\"","timeout":10"#), "{settings}");
        let settings = added_settings(odd.guard, REPORT, &[]);
        assert!(in_role("claude", &odd).ends_with(&format!("{SETTINGS} {}", quoted(&settings))));
    }

    const HELP: &str = "Usage: claude [options]\n  --add-dir <directories...>\n  --model <model>\n  \
                        --setting-sources <sources>\n  --settings <file-or-json>\n";

    #[test]
    fn settings_limit_a_worker_where_they_name_a_hook_or_deny_a_permission() {
        let limited = |settings: &str| {
            Harness::ClaudeCode.limited(Held::File(serde_yaml::Deserializer::from_str(settings)))
        };
        for none in ["{}", r#"{"hooks": {}}"#, r#"{"hooks": {"PreToolUse": []}, "model": "opus"}"#]
        {
            assert!(!limited(none).unwrap(), "{none}");
        }
        for some in [
            r#"{"hooks": {"Stop": [], "PreToolUse": [{"matcher": "Bash", "hooks": []}]}}"#,
            r#"{"permissions": {"deny": ["Bash(git push --force:*)"]}}"#,
        ] {
            assert!(limited(some).unwrap(), "{some}");
        }
        assert!(limited(r#"{"hooks": ["PreToolUse"]}"#).is_err());
        let held = |held: Held<serde_yaml::Deserializer>| Harness::ClaudeCode.limited(held);
        assert!(!held(Held::Nothing).unwrap());
        assert!(held(Held::Entries(1)).unwrap_err().to_string().contains("a directory"));
    }

    #[test]
    fn a_command_that_takes_the_flags_is_not_refused() {
        assert_eq!(flags_question("kim --yolo"), "kim --yolo --help");
        assert_eq!(takes_flags("kim --yolo", Some(HELP)), Ok(()));
    }

    #[test]
    fn a_command_that_does_not_take_the_flags_is_refused_naming_the_reason() {
        let old = HELP.replace("--setting-sources", "--settings").replace("--add-dir", "--dir");
        let why = takes_flags("agent", Some(&old)).unwrap_err();
        assert!(why.contains("`agent --help` names no --add-dir, no --setting-sources."), "{why}");
        assert!(
            why.contains("cannot take the fleet's launch flags") && why.contains("\"agent\""),
            "{why}"
        );
        for silent in [None, Some(""), Some(" \n")] {
            let why = takes_flags("agent ; true", silent).unwrap_err();
            assert!(why.contains("`agent ; true --help` failed in a login shell"), "{why}");
        }
    }

    #[test]
    fn its_footer_says_whether_background_work_of_its_own_runs() {
        let screen = |lines: &[&str]| lines.iter().map(|line| line.to_string()).collect::<Vec<_>>();
        let waiting = screen(&[
            "⏺ Monitor(gate --land finishing for #512)",
            "✻ Worked for 14s · done 11:03 PM · 1 shell, 1 monitor still running",
            "─────",
            "❯ ",
            "─────",
            "  ⏵⏵ bypass permissions on · 1 shell, 1 monitor · ← 4 agents · ↓ to manage     130593 tokens",
        ]);
        assert!(shows_background(&waiting));
        let two = screen(&["❯", "  ⏵⏵ bypass permissions on · 2 shells"]);
        assert!(shows_background(&two));
        // what a turn's end said, with the footer counting none now
        let ended = screen(&[
            "✻ Worked for 14s · done 11:03 PM · 1 shell, 1 monitor still running",
            "❯ ",
            "  ⏵⏵ bypass permissions on · ← 4 agents",
        ]);
        assert!(!shows_background(&ended));
        assert!(!shows_background(&screen(&["❯", "  ⏵⏵ bypass permissions on (shift+tab)"])));
        assert!(!shows_background(&screen(&[])));
        // no prompt in sight: the last line is the footer
        assert!(shows_background(&screen(&["output", "1 monitor", ""])));
        assert!(!shows_background(&screen(&["1 monitor", "output"])));
    }

    /// What Claude Code printed for `models_question` on a host, its
    /// commands cut to one, after its hooks had begun.
    const INITIALIZED: &str = include_str!("claude_code/initialize.jsonl");

    #[test]
    fn claude_code_is_asked_its_models_with_initialize_and_no_prompt() {
        let question = Harness::ClaudeCode.models_question("kim  --verbose", "/kept");
        assert_eq!(
            question,
            "printf '%s\\n' '{\"type\":\"control_request\",\"request_id\":\"models\",\
             \"request\":{\"subtype\":\"initialize\"}}' | kim  --verbose -p \
             --input-format stream-json --output-format stream-json --verbose"
        );
        // Nothing is sent but the one request: no prompt.
        assert_eq!(question.matches("control_request").count(), 1);
        assert!(!question.contains("\"user\""), "{question}");
    }

    #[test]
    fn its_models_are_the_values_listed_in_its_answer_in_order() {
        let listed = [
            "default",
            "opus",
            "fable",
            "sonnet",
            "haiku",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-fable-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-mythos-5-1",
        ];
        assert_eq!(Harness::ClaudeCode.models(INITIALIZED).unwrap(), listed);
        // As a terminal shows it: an rc file's line first, broken every 80
        // characters, with a prompt after.
        let shown: Vec<String> = INITIALIZED
            .chars()
            .collect::<Vec<_>>()
            .chunks(80)
            .map(|row| row.iter().collect::<String>())
            .collect();
        let shown = format!("an rc file said so\n{}\n% ", shown.join("\n"));
        assert_eq!(Harness::ClaudeCode.models(&shown).unwrap(), listed);
        for nothing in [
            "",
            "Not logged in · Please run /login\n",
            r#"{"type":"control_response","response":{"response":{"models":[]}}}"#,
            r#"{"models":[{"name":"x","efforts":[{"value":"low"}]}]}"#,
        ] {
            assert_eq!(Harness::ClaudeCode.models(nothing), None, "{nothing}");
        }
        // A `value` of a model's own, or in a string, is no model.
        let nested = r#"{"models":[{"value":"a","x":{"value":"no"},"d":"\"value\":\"no\""},
            {"d":"]}","value":"b"}],"after":{"value":"no"}}"#;
        assert_eq!(Harness::ClaudeCode.models(nested).unwrap(), ["a", "b"]);
    }

    #[test]
    fn the_names_claude_code_shows_its_models_by_are_read_whatever_their_keys_order() {
        let shown = Harness::ClaudeCode.shown(INITIALIZED);
        assert_eq!(shown[1], ("opus".to_string(), "Opus 5.5".to_string()));
        assert_eq!(shown.len(), Harness::ClaudeCode.models(INITIALIZED).unwrap().len());
        let reordered = r#"{"models":[{"displayName":"Opus 5.5","value":"opus"},{"value":"x"}]}"#;
        assert_eq!(Harness::ClaudeCode.shown(reordered), [("opus".into(), "Opus 5.5".into())]);
    }

    #[test]
    fn claude_code_knows_a_model_it_lists_by_its_whole_name_and_no_other() {
        let listed = Harness::ClaudeCode.models(INITIALIZED).unwrap();
        for known in ["opus", " sonnet ", "claude-opus-4-8"] {
            assert!(Harness::ClaudeCode.knows(&listed, known), "{known}");
            assert!(Harness::ClaudeCode.names(&listed, known), "{known}");
        }
        for unknown in ["Opus", "claude-opus", "gpt-5.5", ""] {
            assert!(!Harness::ClaudeCode.knows(&listed, unknown), "{unknown}");
            assert!(!Harness::ClaudeCode.names(&listed, unknown), "{unknown}");
        }
    }
}

#[cfg(test)]
mod usage_tests {
    use crate::account::Counts;
    use crate::harness::Harness;

    #[test]
    fn a_streamed_message_is_counted_once() {
        let text = r#"{"type":"assistant","message":{"id":"1","role":"assistant","model":"a","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":3,"output_tokens":5}}}
{"type":"assistant","message":{"id":"1","role":"assistant","model":"a","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":3,"output_tokens":5}}}
{"type":"assistant","message":{"id":"2","role":"assistant","model":"b","usage":{"input_tokens":6,"cache_read_input_tokens":7,"cache_creation_input_tokens":1,"output_tokens":2}}}"#;
        let tokens = Harness::ClaudeCode.tokens(text, None).unwrap();
        assert_eq!(
            tokens["a"],
            Counts { input: 10, cache_read: 20, cache_write: 3, output: 5, reasoning: None }
        );
        assert_eq!(
            tokens["b"],
            Counts { input: 6, cache_read: 7, cache_write: 1, output: 2, reasoning: None }
        );
    }

    #[test]
    fn a_round_window_counts_only_its_messages() {
        let claude = r#"{"timestamp":"1970-01-01T00:01:35Z","type":"assistant","message":{"id":"before","role":"assistant","model":"claude","usage":{"input_tokens":10,"cache_read_input_tokens":2,"output_tokens":3}}}
{"timestamp":"1970-01-01T00:01:45Z","type":"assistant","message":{"id":"round","role":"assistant","model":"claude","usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":1,"output_tokens":4}}}
{"timestamp":"1970-01-01T00:01:55Z","type":"assistant","message":{"id":"after","role":"assistant","model":"claude","usage":{"input_tokens":20,"cache_read_input_tokens":8,"output_tokens":9}}}"#;
        let tokens = Harness::ClaudeCode.tokens(claude, Some((100, 110))).unwrap();
        assert_eq!(
            tokens["claude"],
            Counts { input: 7, cache_read: 5, cache_write: 1, output: 4, reasoning: None }
        );
    }
}

//! Grok, as a harness. A role's denied permissions are Grok's own project
//! file; its hooks are not loaded from that file, so each role gets a
//! generated home whose config holds the role's hooks and denies, the fleet's
//! guard and the report. A session works only in its cwd, so the worker
//! starts in the worktree (ADR-0043). The role's rules are told in that home,
//! and its skills are named there. The role's files stay where every harness
//! writes them.

use std::collections::BTreeMap;

use flux_rs::attrs::*;
use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};
use toml::Value as Toml;

use super::session::{self, json_lines};
use super::{Held, Kept, Keystroke, Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::RULES;
use crate::start::{Rules, Start, Told};
use crate::tokens::{Counts, Keys, Usage};
use crate::turn::{Marker, Note, Turn};

pub(super) const NAME: &str = "Grok";
pub(super) const KEY: &str = "grok";
pub(super) const PROGRAM: &str = "grok";
/// The line that installs it.
pub(super) const INSTALL: &str = "curl -fsSL https://x.ai/cli/install.sh | bash";
/// A session of it outside a role gets a home of its own.
pub(super) const OWN_HOME_OUTSIDE_ROLE: bool = true;
/// How it is asked whether it is logged in, and logged in.
pub(super) const LOGIN: Option<super::Login> = Some(super::Login {
    question: "grok models",
    logged_out: "is not logged in or listed no model",
    line: "grok login",
});

/// Whether the answer to its login question says it is logged in: it does
/// not say it is not, and names a model.
pub(super) fn logged_in(succeeded: bool, printed: &str) -> bool {
    succeeded && !not_authenticated(printed) && models(printed).is_some()
}
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".grok/rules/AGENTS.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("grok");
/// Lets an unattended interactive worker run. Deny rules and hooks still apply.
pub(super) const STOCK: &str = "grok --always-approve";

pub(super) const MODEL: &str = "--model";
/// On the launch line. Grok 1.0.70 accepts it and `--help` does not name it,
/// so it is not one of `FLAGS`: the flag check does not require `--help` to.
pub(super) const TRUST: &str = "--trust";
/// `TRUST` is a flag `--help` does not name, and not one of `FLAGS`: outside
/// a role a launch adds no flag it checks the command takes.
pub(super) const TRUST_IS_FLAGS: bool = false;
/// `--add-dir` is not one of these. Grok 1.0.60 and later reject
/// `additionalDirectories` on session create, so the fleet never passes it
/// (ADR-0043). A command must take `--model`.
pub(super) const FLAGS: [&str; 1] = [MODEL];
const HELP: &str = " --help";
const MODELS: &str = " models";
const PROMPT: &str = " -p";
const AS_JSON: &str = " --output-format json";

const USER_DIR: &str = "GROK_HOME";
/// Nothing here is set to `=1`. The disables are `=0`, written in `environment`.
pub(super) const ENVIRONMENT: [&str; 0] = [];

pub(super) const KEPT: Kept = Kept {
    links: &[Link { home: ".grok/auth.json", kept: "auth.json" }],
    host_user: &[
        Link { home: ".grok/skills", kept: "skills" },
        Link { home: ".grok/commands", kept: "commands" },
    ],
    files: &[],
};

pub(super) const LIMITS: &str = ".grok/config.toml";
pub(super) const REPO_RULES: [&str; 2] = ["AGENTS.md", RULES];
/// The role's rules. The session cwd is the worktree, so this file is how
/// they reach the worker (ADR-0043). The role module reads the name.
pub(super) const ROLE_FILE: &str = RULES;
/// Where the generated home writes the repo and team rules. Not the user's
/// rules directory: that one is named, when the worker is given it, and this
/// file has to stay the fleet's.
pub(super) const SYSTEM_FILE: &str = "rules/told.md";
pub(super) const REFUSED: u8 = 2;

const HOOK_TIMEOUT: i64 = 10;

/// Events the report rides, besides `PreToolUse` which also asks the guard.
/// Documented for Grok; a denied call may not reach the report on that one
/// event, and busy and stopped still come from the others.
const STATE_EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "StopFailure",
    "PreCompact",
    "PostCompact",
    "Notification",
    "SessionEnd",
];

/// Home skill and command directories a worker is not given when `host_user`
/// is false. `~` is the host user's home, which Grok expands in its own config.
const HOME_SKILLS: [&str; 7] = [
    "~/.claude/skills",
    "~/.claude/commands",
    "~/.agents/skills",
    "~/.agents/commands",
    "~/.cursor/skills",
    "~/.grok/skills",
    "~/.grok/commands",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<IgnoredAny>>,
    #[serde(default)]
    permission: Permissions,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Permissions {
    #[serde(default)]
    deny: Vec<String>,
    /// Accepted so a role can name them beside a deny. Neither is a limit.
    #[serde(default, rename = "allow")]
    _allow: Vec<IgnoredAny>,
    #[serde(default, rename = "ask")]
    _ask: Vec<IgnoredAny>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    #[serde(default)]
    decision: String,
}

pub(super) fn limited<'de, D: Deserializer<'de>>(held: Held<D>) -> Result<bool, D::Error> {
    match held {
        Held::Nothing => Ok(false),
        Held::Entries(_) => Err(D::Error::custom("it is a directory, not a TOML file")),
        Held::File(limits) => {
            let limits = Limits::deserialize(limits)?;
            Ok(!limits.permission.deny.is_empty()
                || limits.permission.rules.iter().any(|rule| rule.decision == "deny")
                || limits.hooks.values().any(|hooks| !hooks.is_empty()))
        }
    }
}

/// Reads a role's TOML limits through the same decision as the other harnesses.
pub(super) fn limited_file(text: &str) -> Result<bool, String> {
    let deserializer = toml::de::Deserializer::new(text);
    limited(Held::File(deserializer)).map_err(|error| error.to_string())
}

/// The configuration a worker or a shared home gets. The host user's Grok
/// config is not read: its hooks, MCP servers, plugins, permission mode and
/// memories stay out. A worker also gets its role's limits and the fleet's
/// hooks; a listing or a probe gets the compatibility switches and neither.
pub(super) fn configuration(
    role: Option<&str>,
    guard: Option<&str>,
    host_user: bool,
    role_skills: Option<&str>,
    role_skill_names: &[String],
) -> Result<String, String> {
    let mut table = toml::map::Map::new();
    table.insert("compat".into(), compat(host_user));
    if host_user {
        let mut paths = toml::map::Map::new();
        paths.insert("extra_rule_dirs".into(), Toml::Array(vec![Toml::from("~/.grok/rules")]));
        table.insert("paths".into(), Toml::Table(paths));
    }
    if let Some(skills) = skills_table(host_user, role_skills, role_skill_names) {
        table.insert("skills".into(), skills);
    }
    if let Some(role) = role.filter(|text| !text.trim().is_empty()) {
        let role =
            role.parse::<Toml>().map_err(|error| format!("the role's Grok limits: {error}"))?;
        let role = role.as_table().ok_or("the role's Grok limits are not a table")?;
        for (key, value) in role {
            if !matches!(key.as_str(), "hooks" | "permission") {
                return Err(format!("the role's Grok limits hold unsupported key `{key}`"));
            }
            if !value.is_table() {
                return Err(format!("the role's Grok {key} is not a table"));
            }
            table.insert(key.clone(), value.clone());
        }
    }
    if let Some(guard) = guard {
        let hooks = table.entry("hooks").or_insert_with(|| Toml::Table(Default::default()));
        let hooks = hooks.as_table_mut().ok_or("the role's Grok hooks are not a table")?;
        let fleet = group(vec![command_hook(&guard_asked(guard)), command_hook(REPORT)]);
        prepend(hooks, "PreToolUse", fleet)?;
        for event in STATE_EVENTS {
            prepend(hooks, event, group(vec![command_hook(REPORT)]))?;
        }
    }
    toml::to_string_pretty(&Toml::Table(table)).map_err(|error| error.to_string())
}

/// Role skills, and the host skills a role's own name stands in place of.
/// With `host_user` false the whole host skill directories are ignored, as
/// before. With it true, only a directory of the same name as one of the
/// role's is ignored, so the rest of the host user's skills stay and the
/// role's copy of a shared name is the one Grok loads.
fn skills_table(host_user: bool, role_skills: Option<&str>, names: &[String]) -> Option<Toml> {
    let mut skills = toml::map::Map::new();
    let role_skills = role_skills.filter(|dir| !dir.is_empty());
    if !host_user {
        let ignore = HOME_SKILLS.into_iter().map(Toml::from).collect();
        skills.insert("ignore".into(), Toml::Array(ignore));
    } else if role_skills.is_some() {
        let mut ignore = Vec::new();
        for name in names {
            if name.is_empty() || name.contains('/') || name.contains('\\') {
                continue;
            }
            for root in HOME_SKILLS {
                ignore.push(Toml::from(format!("{root}/{name}")));
            }
        }
        if !ignore.is_empty() {
            skills.insert("ignore".into(), Toml::Array(ignore));
        }
    }
    if let Some(dir) = role_skills {
        skills.insert("paths".into(), Toml::Array(vec![Toml::from(dir)]));
    }
    if skills.is_empty() { None } else { Some(Toml::Table(skills)) }
}

fn compat(host_user: bool) -> Toml {
    let mut claude = toml::map::Map::new();
    claude.insert("hooks".into(), Toml::from(false));
    claude.insert("mcps".into(), Toml::from(false));
    let mut cursor = toml::map::Map::new();
    cursor.insert("hooks".into(), Toml::from(false));
    cursor.insert("mcps".into(), Toml::from(false));
    if !host_user {
        for cell in ["rules", "agents"] {
            claude.insert(cell.into(), Toml::from(false));
        }
        for cell in ["skills", "rules", "agents"] {
            cursor.insert(cell.into(), Toml::from(false));
        }
    }
    let mut compat = toml::map::Map::new();
    compat.insert("claude".into(), Toml::Table(claude));
    compat.insert("cursor".into(), Toml::Table(cursor));
    Toml::Table(compat)
}

fn command_hook(command: &str) -> Toml {
    let mut hook = toml::map::Map::new();
    hook.insert("type".into(), Toml::from("command"));
    hook.insert("command".into(), Toml::from(command));
    hook.insert("timeout".into(), Toml::from(HOOK_TIMEOUT));
    Toml::Table(hook)
}

fn group(hooks: Vec<Toml>) -> Toml {
    let mut group = toml::map::Map::new();
    group.insert("hooks".into(), Toml::Array(hooks));
    Toml::Table(group)
}

fn prepend(
    hooks: &mut toml::map::Map<String, Toml>,
    event: &str,
    group: Toml,
) -> Result<(), String> {
    let entry = hooks.entry(event).or_insert_with(|| Toml::Array(vec![]));
    let array = entry
        .as_array_mut()
        .ok_or_else(|| format!("the role's Grok hooks.{event} is not an array"))?;
    array.insert(0, group);
    Ok(())
}

/// Grok's hooks fail open on an unreadable or failed guard. Turn every such
/// outcome into the explicit refusal code, with the guard's reason on stderr.
fn guard_asked(guard: &str) -> String {
    format!(
        "said=$({guard} 2>&1 >/dev/null) && exit 0; \
         printf '%s\\n' \"${{said:-afkconvoy guard: it could not be asked}}\" >&2; exit 2"
    )
}

const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/grok-hook.sh\"; \
     if [ -x \"$hook\" ]; then ORCA_HOOK_AGENT_PID=\"$PPID\" /bin/sh \"$hook\"; \
     else cat >/dev/null; fi; exit 0";

/// Repo, team and role rules, as markdown Grok loads from its home. The
/// worker starts in the worktree, so the role's `CLAUDE.md` is told here
/// rather than loaded as the start directory's own rules.
pub(super) fn system(told: &Told) -> Option<String> {
    if told.own.is_none() && told.repo.is_none() && told.added.is_none() {
        return None;
    }
    let mut text = String::new();
    if let Some(rules) = told.own {
        told_rules(&mut text, "The role's own rules", rules);
    }
    if let Some(rules) = told.repo {
        told_rules(&mut text, "The repo's own rules", rules);
    }
    if let Some(rules) = told.added {
        told_rules(&mut text, "The team rules", rules);
    }
    Some(text)
}

fn told_rules(text: &mut String, whose: &str, rules: Rules) {
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(&format!("# {whose}: {}\n\n{}\n", rules.file, rules.text.trim_end()));
}

/// Appends `--trust` when the command does not already carry that word.
/// A path is not that word.
pub(super) fn in_worktree(command: &str) -> String {
    if command.split_whitespace().any(|word| word == TRUST) {
        command.to_string()
    } else {
        format!("{command} {TRUST}")
    }
}

/// Starts one watch interactively: the stock command, trust, and the model.
pub(super) fn watch(command: &str, model: Option<&str>) -> String {
    let line = Line::new().launch(&in_worktree(command));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Uses the generated home containing the fleet's hooks.
pub(super) fn guarded(command: &str, kept: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command)
        .done()
}

#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(start.kept)
        .fixed(" GROK_MEMORY=0")
        .fixed(" GROK_CLAUDE_HOOKS_ENABLED=0")
        .fixed(" GROK_CLAUDE_MCPS_ENABLED=0")
        .fixed(" GROK_CURSOR_HOOKS_ENABLED=0")
        .fixed(" GROK_CURSOR_MCPS_ENABLED=0");
    if start.host_user {
        line
    } else {
        line.fixed(" GROK_CLAUDE_RULES_ENABLED=0")
            .fixed(" GROK_CLAUDE_AGENTS_ENABLED=0")
            .fixed(" GROK_CURSOR_RULES_ENABLED=0")
            .fixed(" GROK_CURSOR_AGENTS_ENABLED=0")
            .fixed(" GROK_CURSOR_SKILLS_ENABLED=0")
    }
}

#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    // No `--add-dir`. Grok 1.0.60+ refuses `additionalDirectories` and the
    // session then never starts. The worktree is the cwd (`in_role`). Repo
    // and team rules are in the kept home, and so are the role's.
    let line = line.fixed(" ").fixed(TRUST);
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

#[spec(fn(Line[@l]) -> Line[l])]
pub(super) fn asked_its_flags(line: Line) -> Line {
    line.fixed(HELP)
}

pub(super) fn models_question(command: &str, kept: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command)
        .fixed(MODELS)
        .done()
}

pub(super) fn prompt_question(
    command: &str,
    kept: &str,
    model: Option<&str>,
    prompt: &str,
) -> String {
    asked(command, kept, model, prompt, false)
}

pub(super) fn probe_question(
    command: &str,
    kept: &str,
    model: Option<&str>,
    prompt: &str,
) -> String {
    asked(command, kept, model, prompt, true)
}

fn asked(command: &str, kept: &str, model: Option<&str>, prompt: &str, json: bool) -> String {
    let line = Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(PROMPT).fixed(" ").word(prompt).fixed(if json { AS_JSON } else { "" }).done()
}

/// What comes before the first model's id in a headless JSON answer.
const USAGE: &str = "\"modelUsage\":{\"";

pub(super) fn answered_as(answer: &str) -> Option<String> {
    let dense: String = answer.split_whitespace().collect();
    let (_, after) = dense.split_once(USAGE)?;
    let (id, _) = after.split_once('"')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// The ids under `Available models:`. The one marked `*` or `(default)` is
/// first: it is the model workers start on until research places them.
/// A line that is not an id ends the list. No section, or no id, is none.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let (_, rest) = answer.split_once("Available models:")?;
    let mut listed = vec![];
    let mut default = None;
    // The colon ends the header line, so the first line of what follows is empty.
    for line in rest.lines().skip_while(|line| line.trim().is_empty()) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let marked = trimmed.starts_with('*') || trimmed.contains("(default)");
        let id = trimmed
            .trim_start_matches(|c: char| c == '*' || c == '-' || c.is_whitespace())
            .trim_end_matches("(default)")
            .trim();
        if id.is_empty() || id.split_whitespace().nth(1).is_some() {
            break;
        }
        if marked && default.is_none() {
            default = Some(id.to_string());
        } else if default.as_deref() != Some(id) {
            listed.push(id.to_string());
        }
    }
    let mut models = Vec::new();
    if let Some(default) = default {
        models.push(default);
    }
    models.extend(listed);
    if models.is_empty() { None } else { Some(models) }
}

pub(super) fn shown(_answer: &str) -> Vec<(String, String)> {
    vec![]
}

/// Whether a listing says the login is missing. A listing that is not logged
/// in still names models, so the models alone do not say it is logged in.
fn not_authenticated(answer: &str) -> bool {
    answer.to_ascii_lowercase().contains("not authenticated")
}

/// One tool call, as Grok hands it to a hook. The event names the tool in
/// camelCase and again in Claude's snake_case, and the input the same way
/// (`toolInput` and `tool_input`). Either spelling alone is the call. Both
/// spellings of one fact have to agree: an alias would reject the event as a
/// duplicate field, and judging one spelling while the tool runs the other
/// would let a command past the guard.
#[derive(Deserialize)]
struct Described {
    #[serde(rename = "toolName")]
    camel_name: Option<String>,
    #[serde(rename = "tool_name")]
    snake_name: Option<String>,
    #[serde(default, rename = "toolInput")]
    camel_input: Option<Input>,
    #[serde(default, rename = "tool_input")]
    snake_input: Option<Input>,
}

#[derive(Default, Deserialize)]
struct Input {
    #[serde(default)]
    command: Option<String>,
}

/// The one value the two spellings give. A spelling that is absent is the
/// other. Both present and different is not a value.
fn one_spelling<T: PartialEq, E: Error>(
    what: &str,
    camel: Option<T>,
    snake: Option<T>,
) -> Result<Option<T>, E> {
    match (camel, snake) {
        (Some(camel), Some(snake)) if camel != snake => {
            Err(E::custom(format!("`{what}` and its other spelling disagree")))
        }
        (Some(camel), _) => Ok(Some(camel)),
        (None, snake) => Ok(snake),
    }
}

pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    let name = one_spelling("toolName", described.camel_name, described.snake_name)?
        .ok_or_else(|| D::Error::custom("a Grok tool call names no tool"))?;
    let command = one_spelling(
        "toolInput",
        described.camel_input.map(|input| input.command),
        described.snake_input.map(|input| input.command),
    )?
    .flatten();
    Ok(match command {
        Some(line) if name == "run_terminal_command" => Call::Shell(line),
        _ => Call::Other,
    })
}

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".grok";

const EVENTS: &str = "events.jsonl";

/// Where Grok keeps the session of `cwd`: one events file in each session
/// directory under the worktree's directory, its usage in the updates
/// beside them.
pub(super) fn session_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Named {
        dir: format!("sessions/{}", dir_name(cwd)),
        name: EVENTS,
        compressed: false,
        usage: Some(UPDATES),
    }
}

/// What was said in each session: the user's lines and the model's, beside
/// its events.
const CHAT: &str = "chat_history.jsonl";

/// Where Grok keeps what was said in the session of `cwd`.
pub(super) fn note_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Named {
        dir: format!("sessions/{}", dir_name(cwd)),
        name: CHAT,
        compressed: false,
        usage: None,
    }
}

/// What a busy Grok prints when a typed line waits in its input queue, and
/// the key that hint names. Measured 2026-11-01 (sunfmin/control#387): the
/// line stays queued until Enter, which the screen calls send now. A line
/// still only queued is not a message the model was given.
const QUEUE: &str = "Queued";
const SEND_NOW: &str = "Enter:send now";
/// The shortest visible piece of the typed line that counts as that line on
/// a truncated queue row. Shorter, and a header could match.
const SHOWN_MIN: usize = 8;

/// The key that hands `line` to the model now, when `screen` shows it
/// waiting in the input queue. `None` when the screen does not show that
/// queue, or does not show this line on it.
pub(super) fn send_queued(screen: &[String], line: &str) -> Option<Keystroke> {
    let line = squash(line);
    if line.chars().count() < SHOWN_MIN {
        return None;
    }
    let flat = squash(&screen.join(" "));
    if !flat.contains(QUEUE) || !flat.contains(SEND_NOW) {
        return None;
    }
    screen.iter().any(|raw| shows_item(raw, &line)).then_some(Keystroke { text: None, enter: true })
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `raw` is a queue row whose visible text is `line`, or the head of
/// it where the row is cut short.
fn shows_item(raw: &str, line: &str) -> bool {
    let shown = visible_item(raw);
    let n = shown.chars().count();
    n >= SHOWN_MIN && (line.starts_with(&shown) || shown.starts_with(line))
}

/// A queue row with its index and a trailing ellipsis taken off.
fn visible_item(raw: &str) -> String {
    let shown = squash(raw);
    let shown = match shown.strip_prefix('#') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => {
            rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start()
        }
        _ => shown.as_str(),
    };
    shown.trim_end_matches(['.', '…']).trim().to_string()
}

/// The chat entry a stand-in writes for `line`, typed while the worker
/// worked, once Grok gave it to its model: as Grok words it. A line still in
/// its input queue is in no file.
pub(super) fn note_files(cwd: &str, line: &str, taken: bool, _at: u64) -> Vec<SessionFile> {
    if !taken {
        return Vec::new();
    }
    let said =
        format!("The user sent a message while you were working: <user_query>{line}</user_query>");
    let entry = session::object(vec![("role", "user".into()), ("content", said.into())]);
    vec![SessionFile {
        path: format!("sessions/{}/afkconvoy-turn/{CHAT}", dir_name(cwd)),
        body: format!("{entry}\n"),
        compressed: false,
        replaces: None,
    }]
}

/// The same path → `%2FUsers%2Fsunfmin%2FDevelopments%2Fchessmirror`.
fn dir_name(cwd: &str) -> String {
    let mut out = String::new();
    for byte in cwd.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    session::stored_name(out, cwd)
}

/// The turn `text` records. The worktree is the directory the file was in.
/// A session that is not the primary one is a child.
pub(super) fn recorded_turn(_cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
    session::turn_of(&notes, not_before)
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::parse_iso(value["ts"].as_str()?)?;
    let child = value["session_relationship"].as_str().is_some_and(|kind| kind != "primary");
    match value["type"].as_str()? {
        "turn_started" => Some(Note { marker: Marker::Start, at, child }),
        "turn_ended" => Some(Note { marker: Marker::End, at, child }),
        _ => None,
    }
}

/// The events file a stand-in writes for this turn, and the summary beside
/// it. The summary is not the turn.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let dir = format!("sessions/{}/afkconvoy-turn", dir_name(cwd));
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("ts", iso.clone().into()),
            ("type", "turn_started".into()),
            ("turn_number", 1_u64.into()),
            ("session_relationship", "primary".into()),
        ])
    );
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("ts", iso.clone().into()),
                ("type", "turn_ended".into()),
                ("outcome", "completed".into()),
            ])
        ));
    }
    vec![
        SessionFile { path: format!("{dir}/{EVENTS}"), body, compressed: false, replaces: None },
        SessionFile {
            path: format!("{dir}/summary.json"),
            body: session::object(vec![(
                "info",
                session::object(vec![("cwd", cwd.into()), ("created_at", iso.into())]),
            )])
            .to_string(),
            compressed: false,
            replaces: None,
        },
    ]
}

/// The file beside a session's events whose updates state its usage.
const UPDATES: &str = "updates.jsonl";

/// The names Grok gives the counts of one model's usage. Its input includes
/// the cached input.
const USAGE_KEYS: Keys = Keys {
    input: "inputTokens",
    cache_read: "cachedReadTokens",
    cache_write: "cacheCreationTokens",
    output: "outputTokens",
    reasoning: "reasoningTokens",
};

/// What one line of a Grok session's updates says of usage: every model's
/// running usage so far.
pub(super) fn usage(value: &serde_json::Value, _line: usize) -> Option<Usage> {
    let usage = &value["params"]["update"]["usage"];
    if !usage.is_object() {
        return None;
    }
    let Some(models) = usage["modelUsage"].as_object() else { return Some(Usage::Uncounted) };
    Some(Usage::Snapshot(
        models
            .iter()
            .filter_map(|(name, usage)| {
                Some((name.clone(), Counts::read(usage, &USAGE_KEYS, true)?))
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{Harness, Keystroke};

    fn start<'a>(model: Option<&'a str>, host_user: bool) -> Start<'a> {
        Start {
            dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            added: "/copies/acme/team/def",
            model,
            hook: "/bin/afkconvoy guard",
            kept: "/cache/harness/grok-role",
            host_user,
            dsh_profile: crate::start::DshProfile::Fleet,
            host_rules: &[],
            shadowed: &[],
            told: Told::default(),
            env: crate::harness::stage_env(crate::harness::Harness::Grok, model),
        }
    }

    #[test]
    fn a_worker_starts_in_its_role_with_trust_and_no_shell_expansion() {
        let line = Harness::Grok.in_role(STOCK, &start(Some("grok-4.7"), false));
        assert!(
            line.starts_with(
                "cd '/wt/issue 7' && export GROK_HOME='/cache/harness/grok-role' GROK_MEMORY=0"
            ),
            "{line}"
        );
        assert!(line.contains("GROK_CLAUDE_HOOKS_ENABLED=0"), "{line}");
        assert!(line.contains("GROK_CLAUDE_RULES_ENABLED=0"), "{line}");
        assert!(line.contains("GROK_CURSOR_SKILLS_ENABLED=0"), "{line}");
        assert!(!line.contains("GROK_CLAUDE_SKILLS_ENABLED=0"), "{line}");
        assert!(!line.contains("--add-dir"), "{line}");
        assert!(line.contains(" AFKCONVOY_HARNESS='grok' AFKCONVOY_MODEL='grok-4.7' && grok --always-approve --trust --model 'grok-4.7'"), "{line}");
        let odd = Start { kept: "/it's kept/$HOME", host_user: true, ..start(None, true) };
        let line = Harness::Grok.in_role("grok", &odd);
        assert!(line.contains("GROK_HOME='/it'\\''s kept/$HOME'"), "{line}");
        assert!(!line.contains("GROK_CLAUDE_RULES_ENABLED"), "{line}");
        assert!(!line.contains("--model"), "{line}");
        assert!(line.contains("--trust"), "{line}");
    }

    #[test]
    fn an_unbound_command_gains_trust_once() {
        assert_eq!(Harness::Grok.in_worktree(STOCK, "/wt", ""), "grok --always-approve --trust");
        assert_eq!(
            Harness::Grok.in_worktree("grok --trust --always-approve", "/wt", ""),
            "grok --trust --always-approve"
        );
        assert_eq!(
            Harness::Grok.watch(STOCK, "/wt", "/kept", Some("grok-4.7")),
            "grok --always-approve --trust --model 'grok-4.7'"
        );
    }

    #[test]
    fn configuration_keeps_the_role_and_not_the_user_and_refuses_other_keys() {
        let role = r#"
[permission]
deny = ["Bash(rm -rf *)"]
[[permission.rules]]
decision = "deny"
pattern = "Bash(git push*)"
[[hooks.PreToolUse]]
matcher = "Write"
hooks = [{ type = "command", command = "role hook", timeout = 5 }]
"#;
        let config =
            configuration(Some(role), Some("/bin/afkconvoy guard"), false, None, &[]).unwrap();
        let parsed: Toml = config.parse().unwrap();
        assert_eq!(parsed["compat"]["claude"]["hooks"].as_bool(), Some(false));
        assert_eq!(parsed["compat"]["claude"]["mcps"].as_bool(), Some(false));
        assert_eq!(parsed["compat"]["claude"]["rules"].as_bool(), Some(false));
        assert!(parsed["compat"]["claude"].get("skills").is_none());
        assert_eq!(parsed["compat"]["cursor"]["skills"].as_bool(), Some(false));
        assert!(parsed["skills"]["ignore"].as_array().unwrap().len() >= 6);
        assert!(parsed.get("paths").is_none());
        assert_eq!(parsed["permission"]["deny"][0].as_str(), Some("Bash(rm -rf *)"));
        let pre = parsed["hooks"]["PreToolUse"].as_array().unwrap();
        let fleet = pre[0]["hooks"].as_array().unwrap();
        assert!(fleet[0]["command"].as_str().unwrap().contains("/bin/afkconvoy guard"));
        assert!(fleet[0]["command"].as_str().unwrap().ends_with("exit 2"));
        assert!(fleet[1]["command"].as_str().unwrap().contains("grok-hook.sh"));
        assert!(fleet[1]["command"].as_str().unwrap().ends_with("exit 0"));
        assert_eq!(pre[1]["hooks"][0]["command"].as_str(), Some("role hook"));
        assert!(
            parsed["hooks"]["SessionStart"].as_array().unwrap()[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("grok-hook.sh")
        );
        assert!(configuration(Some("plugins = []\n"), None, true, None, &[]).is_err());
        let shared = configuration(None, None, true, None, &[]).unwrap().parse::<Toml>().unwrap();
        assert!(shared.get("skills").is_none());
        let named = configuration(
            None,
            None,
            true,
            Some("/copies/acme/dev/abc/.claude/skills"),
            &["implement".into()],
        )
        .unwrap();
        assert!(named.contains("/copies/acme/dev/abc/.claude/skills"), "{named}");
        assert!(named.contains("~/.grok/skills/implement"), "{named}");
        assert!(named.contains("~/.agents/skills/implement"), "{named}");
        assert!(!named.contains("~/.grok/skills\"]"), "{named}");
        assert!(shared.get("hooks").is_none());
        assert_eq!(shared["paths"]["extra_rule_dirs"][0].as_str(), Some("~/.grok/rules"));
        assert!(shared["compat"]["claude"].get("rules").is_none());
    }

    #[test]
    fn probes_use_the_isolated_home_and_name_no_interactive_prompt_wait() {
        assert_eq!(
            Harness::Grok.models_question(STOCK, "/it's kept"),
            "export GROK_HOME='/it'\\''s kept' && grok --always-approve models"
        );
        assert_eq!(
            Harness::Grok.probe_question(STOCK, "/k", Some("a b"), "Say OK."),
            "export GROK_HOME='/k' && grok --always-approve --model 'a b' -p 'Say OK.' --output-format json"
        );
        assert_eq!(
            Harness::Grok.prompt_question("mygrok", "/k", None, "Say OK."),
            "export GROK_HOME='/k' && mygrok -p 'Say OK.'"
        );
    }

    #[test]
    fn models_and_the_answered_model_are_read_from_grok() {
        let listed = "You are logged in with grok.com.\nDefault model: grok-4.7\nAvailable models:\n  * grok-4.7 (default)\n  - grok-4.5\n  - grok-4.6\n";
        assert_eq!(Harness::Grok.models(listed).unwrap(), ["grok-4.7", "grok-4.5", "grok-4.6"]);
        assert!(Harness::Grok.shown(listed).is_empty());
        assert!(!not_authenticated(listed));
        let out = "You are not authenticated.\nDefault model: grok-4.6\nAvailable models:\n  * grok-4.6 (default)\n  - grok-4.5\n";
        assert!(not_authenticated(out));
        assert_eq!(Harness::Grok.models(out).unwrap(), ["grok-4.6", "grok-4.5"]);
        assert_eq!(Harness::Grok.models("no models here"), None);
        assert_eq!(Harness::Grok.models("Available models:\n\n"), None);
        let answered = "{\"result\":\"OK\",\"modelUsage\":{\"grok-4.6\":{\"outputTokens\":1}}}";
        assert_eq!(
            Harness::Grok.answered_as(answered),
            super::super::AnsweredAs::Said("grok-4.6".into())
        );
        assert_eq!(Harness::Grok.answered_as("OK\n"), super::super::AnsweredAs::Unsaid);
    }

    #[test]
    fn role_limits_and_calls_are_read_in_grok_s_own_form() {
        assert!(!limited_file("# nothing yet\n").unwrap());
        assert!(!limited_file("[permission]\ndeny = []\n").unwrap());
        assert!(limited_file("[permission]\ndeny = ['Bash(rm *)']\n").unwrap());
        assert!(limited_file("[[permission.rules]]\ndecision = 'deny'\n").unwrap());
        assert!(!limited_file("[[permission.rules]]\ndecision = 'ask'\n").unwrap());
        assert!(limited_file("[[hooks.PreToolUse]]\nmatcher = 'Write'\n").unwrap());
        assert!(limited_file("plugins = []\n").is_err());
        let bash =
            r#"{"toolName":"run_terminal_command","toolInput":{"command":"git commit -m x"}}"#;
        let judged = call(&mut serde_json::Deserializer::from_str(bash)).unwrap();
        assert_eq!(judged, Call::Shell("git commit -m x".into()));
        let snake = r#"{"tool_name":"run_terminal_command","tool_input":{"command":"git status"}}"#;
        assert_eq!(
            call(&mut serde_json::Deserializer::from_str(snake)).unwrap(),
            Call::Shell("git status".into())
        );
        let other = r#"{"toolName":"Bash","toolInput":{"command":"git commit"}}"#;
        assert_eq!(call(&mut serde_json::Deserializer::from_str(other)).unwrap(), Call::Other);
        // Grok 1.0.70 sends both spellings of the same fact on one event.
        let both = r#"{"hookEventName":"pre_tool_use","hook_event_name":"PreToolUse","toolName":"run_terminal_command","tool_name":"run_terminal_command","toolInput":{"command":"ls"},"tool_input":{"command":"ls"},"cwd":"/wt"}"#;
        assert_eq!(
            call(&mut serde_json::Deserializer::from_str(both)).unwrap(),
            Call::Shell("ls".into())
        );
        let read = r#"{"toolName":"read_file","tool_name":"read_file","toolInput":{"target_file":"a"},"tool_input":{"target_file":"a"}}"#;
        assert_eq!(call(&mut serde_json::Deserializer::from_str(read)).unwrap(), Call::Other);
        let names = r#"{"toolName":"read_file","tool_name":"run_terminal_command","toolInput":{"command":"ls"},"tool_input":{"command":"ls"}}"#;
        assert!(
            call(&mut serde_json::Deserializer::from_str(names))
                .unwrap_err()
                .to_string()
                .contains("disagree")
        );
        let commands = r#"{"toolName":"run_terminal_command","tool_name":"run_terminal_command","toolInput":{"command":"ls"},"tool_input":{"command":"rm -rf /"}}"#;
        assert!(
            call(&mut serde_json::Deserializer::from_str(commands))
                .unwrap_err()
                .to_string()
                .contains("disagree")
        );
    }

    #[test]
    fn a_busy_grok_queues_a_line_until_enter_sends_it_now() {
        let line = "Operator's note (also on the issue: https://github.com/acme/widgets/issues/7#issuecomment-41): use the v2 endpoint";
        let screen = vec![
            "reading the repo".to_string(),
            "▾ Queued 1".to_string(),
            "#1 Operator's note …".to_string(),
            "Enter:send now".to_string(),
        ];
        assert_eq!(
            Harness::Grok.send_queued(&screen, line),
            Some(Keystroke { text: None, enter: true })
        );
        // The hint wraps, and the row holds the whole line.
        let wrapped =
            vec!["▾ Queued 1".into(), format!("#1 {line}"), "Enter:send".into(), "now".into()];
        assert_eq!(
            Harness::Grok.send_queued(&wrapped, line),
            Some(Keystroke { text: None, enter: true })
        );
        // Another line is queued. Enter would hand that one over, not this.
        let other = vec![
            "▾ Queued 1".into(),
            "#1 land after the other issue …".into(),
            "Enter:send now".into(),
        ];
        assert_eq!(Harness::Grok.send_queued(&other, line), None);
        // No queue, or no send-now hint: the line was not seen waiting.
        assert_eq!(Harness::Grok.send_queued(&["#1 Operator's note …".into()], line), None);
        assert_eq!(
            Harness::Grok.send_queued(&["▾ Queued 1".into(), "#1 Operator's note …".into()], line),
            None
        );
        assert_eq!(Harness::Grok.send_queued(&screen, "short"), None);
        for harness in Harness::ALL {
            if harness == Harness::Grok {
                continue;
            }
            assert_eq!(harness.send_queued(&screen, line), None, "{}", harness.key());
        }
    }

    #[test]
    fn told_rules_are_markdown_with_no_base_prompt() {
        let told = Told {
            own: Some(Rules { file: "CLAUDE.md", text: "role rules" }),
            repo: Some(Rules { file: "AGENTS.md", text: "repo rules" }),
            added: Some(Rules { file: "CLAUDE.md", text: "team rules" }),
        };
        let text = system(&told).unwrap();
        assert!(
            text.find("# The role's own rules:").unwrap()
                < text.find("# The repo's own rules:").unwrap()
        );
        assert!(text.contains("# The repo's own rules: AGENTS.md"));
        assert!(
            text.contains("role rules")
                && text.contains("repo rules")
                && text.contains("team rules")
        );
        assert!(!text.contains("base_prompt"));
        assert!(system(&Told::default()).is_none());
    }

    #[test]
    fn grok_is_logged_in_only_when_it_says_so_and_names_a_model() {
        let grok = Harness::Grok;
        let listed = "You are logged in with grok.com.\nAvailable models:\n  * grok-4.7 (default)\n  - grok-4.5\n";
        assert!(grok.logged_in(true, listed));
        assert!(!grok.logged_in(false, listed), "a failed question is not a login");
        let out =
            "You are not authenticated.\nAvailable models:\n  * grok-4.6 (default)\n  - grok-4.5\n";
        assert!(!grok.logged_in(true, out), "a listing that names models is still logged out");
        assert!(!grok.logged_in(true, "You are logged in with grok.com.\n"));
        let login = grok.login().unwrap();
        assert_eq!((login.question, login.line), ("grok models", "grok login"));
        assert_eq!(login.logged_out, "is not logged in or listed no model");
        assert_eq!(grok.install(), "curl -fsSL https://x.ai/cli/install.sh | bash");
        assert!(grok.own_home_outside_role());
    }
}

#[cfg(test)]
mod usage_tests {
    use crate::harness::Harness;
    use crate::tokens::Counts;

    #[test]
    fn the_last_usage_update_is_the_sessions() {
        let text = r#"{"params":{"update":{"usage":{"modelUsage":{"a":{"inputTokens":33,"cachedReadTokens":20,"cacheCreationTokens":3,"outputTokens":5}}}}}}
{"params":{"update":{"usage":{"modelUsage":{"a":{"inputTokens":33,"cachedReadTokens":20,"cacheCreationTokens":3,"outputTokens":5},"b":{"inputTokens":14,"cachedReadTokens":7,"cacheCreationTokens":1,"outputTokens":2}}}}}}"#;
        let tokens = Harness::Grok.tokens(text, None).unwrap();
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
    fn a_round_window_subtracts_the_update_before_it() {
        let grok = r#"{"ts":95,"params":{"update":{"usage":{"modelUsage":{"grok":{"inputTokens":130,"cachedReadTokens":30,"outputTokens":10}}}}}}
{"ts":105,"params":{"update":{"usage":{"modelUsage":{"grok":{"inputTokens":190,"cachedReadTokens":45,"outputTokens":18}}}}}}
{"ts":115,"params":{"update":{"usage":{"modelUsage":{"grok":{"inputTokens":255,"cachedReadTokens":55,"outputTokens":25}}}}}}"#;
        let tokens = Harness::Grok.tokens(grok, Some((100, 110))).unwrap();
        assert_eq!(
            tokens["grok"],
            Counts { input: 45, cache_read: 15, cache_write: 0, output: 8, reasoning: None }
        );
    }
}

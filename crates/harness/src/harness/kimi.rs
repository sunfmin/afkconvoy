//! Kimi Code, as a harness: its isolated data home carries only the host's
//! providers and models, plus this role's limits and the fleet's hooks. Kimi
//! has no project-level hook configuration, so each role gets a generated home
//! of its own rather than sharing one worker's configuration with another's.

use flux_rs::attrs::*;
use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};
use serde_json::Value as Json;
use toml::Value as Toml;

use super::session::{self, json_lines};
use super::{Held, Indexed, Kept, Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::RULES;
use crate::start::{Rules, Start, Told};
use crate::steps::{self, Said};
use crate::tokens::{Counts, Keys, Usage};
use crate::turn::{Marker, Note, Turn};
use crate::window::{self, Window};

pub(super) const NAME: &str = "Kimi Code";
pub(super) const KEY: &str = "kimi";
pub(super) const PROGRAM: &str = "kimi";
/// The line that installs it.
pub(super) const INSTALL: &str = "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash";
/// A session of it outside a role gets a home of its own.
pub(super) const OWN_HOME_OUTSIDE_ROLE: bool = true;
/// How it is asked whether it is logged in, and logged in.
pub(super) const LOGIN: Option<super::Login> = Some(super::Login {
    question: "kimi provider list --json",
    logged_out: "is not logged in or has no models configured",
    line: "kimi login",
});

/// Whether the answer to its login question says it is logged in: it has a
/// configured model.
pub(super) fn logged_in(succeeded: bool, printed: &str) -> bool {
    succeeded && models(printed).is_some()
}
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".kimi-code/AGENTS.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("kimi-code-cli");
/// Never Ask mode lets an unattended interactive worker run. Headless calls
/// use `headless`, since Kimi rejects `--auto` together with `--prompt`.
pub(super) const STOCK: &str = "kimi --auto";

const ADD_DIR: &str = "--add-dir";
const SKILLS_DIR: &str = "--skills-dir";
pub(super) const MODEL: &str = "--model";
pub(super) const FLAGS: [&str; 3] = [ADD_DIR, SKILLS_DIR, MODEL];
const HELP: &str = " --help";
const LIST_MODELS: &str = " provider list --json";
const PROMPT: &str = " --prompt";
const AS_JSON: &str = " --output-format stream-json";

pub(super) const USER_DIR: &str = "KIMI_CODE_HOME";
pub(super) const TRUST: &str = "KIMI_CODE_TRUST_WORKSPACE=1";
/// Its worktree trust is a variable, not flags: outside a role a launch
/// adds no flag the command must take.
pub(super) const TRUST_IS_FLAGS: bool = false;
pub(super) const ENVIRONMENT: [&str; 3] =
    ["KIMI_CODE_TRUST_WORKSPACE", "KIMI_CODE_NO_AUTO_UPDATE", "KIMI_DISABLE_TELEMETRY"];
const HOST_SKILLS: [&str; 2] = ["kimi-skills", "agent-skills"];

/// Starts Kimi with its worktree trusted before it reads that directory.
pub(super) fn in_worktree(command: &str) -> String {
    format!("export {TRUST} && {command}")
}

/// Starts one watch interactively, with Kimi's trust and isolated home.
pub(super) fn watch(command: &str, kept: &str, model: Option<&str>) -> String {
    let line = Line::new()
        .fixed("export ")
        .fixed(TRUST)
        .fixed(" ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command);
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

pub(super) const KEPT: Kept = Kept {
    links: &[Link { home: ".kimi-code/credentials", kept: "credentials" }],
    host_user: &[
        Link { home: ".kimi-code/skills", kept: HOST_SKILLS[0] },
        Link { home: ".agents/skills", kept: HOST_SKILLS[1] },
        Link { home: ".kimi-code/AGENTS.md", kept: "AGENTS.md" },
    ],
    // Each generated home is intentional: never offer to migrate the host's
    // kimi-cli configuration into a worker's isolated role home.
    files: &[(SKIP_MIGRATION, "")],
};

/// The file whose presence in its user directory answers the question Kimi
/// Code opens a fresh one with: whether to migrate kimi-cli's configuration.
const SKIP_MIGRATION: &str = ".skip-migration-from-kimi-cli";

/// What a fresh user directory of Kimi Code is given so that it asks no
/// first-run question: the migration declined.
pub(super) const FRESH: [super::Fresh; 1] =
    [super::Fresh { file: SKIP_MIGRATION, given: super::Given::Whole("") }];

pub(super) const LIMITS: &str = ".kimi-code/limits.toml";
/// The host user's Kimi config, of which a worker keeps only model routing.
pub(super) const USER_CONFIG: &str = ".kimi-code/config.toml";
pub(super) const BEFORE_THE_RULES: [&str; 1] = ["AGENTS.md"];
pub(super) const RULES_LINK: &str = "AGENTS.md";
pub(super) const REPO_RULES: [&str; 2] = ["AGENTS.md", RULES];
pub(super) const REFUSED: u8 = 2;

/// A role's limits, in Kimi's native configuration form. Nothing else belongs
/// in this file: the fleet writes the effective user configuration itself.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    #[serde(default)]
    hooks: Vec<IgnoredAny>,
    #[serde(default)]
    permission: Permissions,
}

#[derive(Default, Deserialize)]
struct Permissions {
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    decision: String,
}

pub(super) fn limited<'de, D: Deserializer<'de>>(held: Held<D>) -> Result<bool, D::Error> {
    match held {
        Held::Nothing => Ok(false),
        Held::Entries(_) => Err(D::Error::custom("it is a directory, not a TOML file")),
        Held::File(limits) => {
            let limits = Limits::deserialize(limits)?;
            Ok(!limits.hooks.is_empty()
                || limits.permission.rules.iter().any(|rule| rule.decision == "deny"))
        }
    }
}

/// Reads a role's TOML limits through the same decision as the other harnesses.
pub(super) fn limited_file(text: &str) -> Result<bool, String> {
    let deserializer = toml::de::Deserializer::new(text);
    limited(Held::File(deserializer)).map_err(|error| error.to_string())
}

/// The configuration a worker or probe gets. Only the user's model routing is
/// inherited. A worker also gets its role's limits and the fleet's hooks; a
/// probe gets neither and so cannot execute a role hook outside its role.
pub(super) fn configuration(
    user: Option<&str>,
    role: Option<&str>,
    guard: Option<&str>,
) -> Result<String, String> {
    let mut config = match user {
        Some(text) if !text.trim().is_empty() => {
            text.parse::<Toml>().map_err(|error| format!("the host user's Kimi config: {error}"))?
        }
        _ => Toml::Table(Default::default()),
    };
    let table = config.as_table_mut().ok_or("the host user's Kimi config is not a table")?;
    table.retain(|key, _| {
        matches!(key, "default_model" | "providers" | "models" | "secondary_model")
    });
    if let Some(role) = role.filter(|text| !text.trim().is_empty()) {
        let role =
            role.parse::<Toml>().map_err(|error| format!("the role's Kimi limits: {error}"))?;
        let role = role.as_table().ok_or("the role's Kimi limits are not a table")?;
        for (key, value) in role {
            if !matches!(key.as_str(), "hooks" | "permission") {
                return Err(format!("the role's Kimi limits hold unsupported key `{key}`"));
            }
            table.insert(key.clone(), value.clone());
        }
    }
    if let Some(guard) = guard {
        table.insert("default_permission_mode".into(), Toml::from("auto"));
        table.insert("default_plan_mode".into(), Toml::from(false));
        table.insert("auto_session_title".into(), Toml::from(false));
        table.insert("builtin_product_skills".into(), Toml::from(false));
        let hooks = table.entry("hooks").or_insert_with(|| Toml::Array(vec![]));
        let hooks = hooks.as_array_mut().ok_or("the role's Kimi hooks are not an array")?;
        let mut fleet = vec![hook("PreToolUse", None, &guard_asked(guard), None)];
        for event in STATE_EVENTS {
            fleet.push(hook(event, None, REPORT, Some(10)));
        }
        hooks.splice(0..0, fleet);
    }
    toml::to_string_pretty(&config).map_err(|error| error.to_string())
}

fn hook(event: &str, matcher: Option<&str>, command: &str, timeout: Option<i64>) -> Toml {
    let mut hook = toml::map::Map::new();
    hook.insert("event".into(), Toml::from(event));
    if let Some(matcher) = matcher {
        hook.insert("matcher".into(), Toml::from(matcher));
    }
    hook.insert("command".into(), Toml::from(command));
    if let Some(timeout) = timeout {
        hook.insert("timeout".into(), Toml::from(timeout));
    }
    Toml::Table(hook)
}

const STATE_EVENTS: [&str; 17] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "TurnStarted",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionResult",
    "SubagentStart",
    "SubagentStop",
    "TaskStarted",
    "Stop",
    "StopFailure",
    "Interrupt",
    "PreCompact",
    "PostCompact",
    "Notification",
];

/// Kimi hooks fail open on an unreadable or failed guard. Turn every such
/// outcome into the explicit refusal code, with the guard's reason on stderr.
fn guard_asked(guard: &str) -> String {
    format!(
        "said=$({guard} 2>&1 >/dev/null) && exit 0; \
         printf '%s\\n' \"${{said:-afkconvoy guard: it could not be asked}}\" >&2; exit 2"
    )
}

const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/kimi-hook.sh\"; if [ -x \"$hook\" ]; \
     then /bin/sh \"$hook\"; else cat; fi >/dev/null 2>&1; exit 0";

/// The fleet supplements the role's own rules with the repo's and team's.
/// `${base_prompt}` keeps Kimi's normal prompt, including the role AGENTS.md.
pub(super) fn system(told: &Told) -> Option<String> {
    if told.repo.is_none() && told.added.is_none() {
        return None;
    }
    let mut text = String::from("${base_prompt}");
    if let Some(rules) = told.repo {
        told_rules(&mut text, "The repo's own rules", rules);
    }
    if let Some(rules) = told.added {
        told_rules(&mut text, "The team rules", rules);
    }
    Some(text)
}

fn told_rules(text: &mut String, whose: &str, rules: Rules) {
    text.push_str(&format!(
        "\n\n# {whose}: {}\n\n<rules>\n{}\n</rules>\n",
        rules.file,
        rules.text.trim_end()
    ));
}

fn headless(command: &str) -> &str {
    if command == STOCK { PROGRAM } else { command }
}

#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ").fixed(USER_DIR).fixed("=").word(start.kept)
}

#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    let mut line = line
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.added)
        .fixed(" ")
        .fixed(SKILLS_DIR)
        .fixed(" ")
        .word(&format!("{}/{}", start.dir, crate::start::SKILLS));
    if start.host_user {
        for skills in HOST_SKILLS {
            line = line
                .fixed(" ")
                .fixed(SKILLS_DIR)
                .fixed(" ")
                .word(&format!("{}/{skills}", start.kept));
        }
    }
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
        .launch(headless(command))
        .fixed(LIST_MODELS)
        .done()
}

pub(super) fn prompt_question(
    command: &str,
    kept: &str,
    model: Option<&str>,
    prompt: &str,
) -> String {
    probe(command, kept, model, prompt, false)
}

pub(super) fn probe_question(
    command: &str,
    kept: &str,
    model: Option<&str>,
    prompt: &str,
) -> String {
    probe(command, kept, model, prompt, true)
}

fn probe(command: &str, kept: &str, model: Option<&str>, prompt: &str, json: bool) -> String {
    let line = Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(headless(command));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(PROMPT).fixed(" ").word(prompt).fixed(if json { AS_JSON } else { "" }).done()
}

fn json_answer(answer: &str) -> Option<Json> {
    if let Ok(value) = serde_json::from_str(answer.trim()) {
        return Some(value);
    }
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    serde_json::from_str(answer.get(start..=end)?).ok()
}

pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let answer = json_answer(answer)?;
    let listed = answer.get("models")?.as_object()?;
    let mut models = vec![];
    for name in listed.keys() {
        if !name.is_empty() {
            models.push(name.clone());
        }
    }
    if models.is_empty() { None } else { Some(models) }
}

pub(super) fn shown(answer: &str) -> Vec<(String, String)> {
    let Some(models) = json_answer(answer)
        .and_then(|answer| answer.get("models").and_then(Json::as_object).cloned())
    else {
        return vec![];
    };
    models
        .into_iter()
        .filter_map(|(name, model)| {
            let shown = model.get("display_name")?.as_str()?.to_string();
            (!name.is_empty() && !shown.is_empty()).then_some((name, shown))
        })
        .collect()
}

pub(super) fn answered_as(answer: &str) -> Option<String> {
    answer.lines().filter_map(|line| serde_json::from_str::<Json>(line).ok()).find_map(|value| {
        ["model", "message.model", "payload.model", "data.model"].into_iter().find_map(|path| {
            let mut value = &value;
            for part in path.split('.') {
                value = value.get(part)?;
            }
            value.as_str().filter(|model| !model.is_empty()).map(str::to_string)
        })
    })
}

#[derive(Deserialize)]
struct Described {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    #[serde(default)]
    command: Option<String>,
}

pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.tool_input.command {
        Some(line) if described.tool_name == "Bash" => Call::Shell(line),
        _ => Call::Other,
    })
}

/// The line Kimi Code waits for the next prompt at. Its status line, the
/// footer, is below it.
const INPUT: char = '❯';

/// Whether the end of a Kimi Code worker's screen shows a background shell
/// it started still running. The footer badges that shell as `[1 task running]`
/// or `[2 tasks running]`; a background agent (`[1 agent running]`) is not a
/// gate or a landing. What a turn quoted above the prompt is not read.
pub(super) fn shows_background(screen: &[String]) -> bool {
    let mut below = screen.iter();
    match screen.iter().rposition(|line| line.trim_start().starts_with(INPUT)) {
        Some(prompt) => {
            below.nth(prompt);
        }
        // No prompt: the badge is on the first of the two footer lines.
        None => {
            if let Some(from) = screen.len().checked_sub(2).and_then(|n| n.checked_sub(1)) {
                below.nth(from);
            }
        }
    }
    below.any(|line| background_task(line))
}

/// Kimi Code's own usage-limit error, reported on sunfmin/control#166.
/// The provider prefix is part of the words: a bare "5-hour usage limit"
/// on a screen is not this quota.
const USAGE_LIMIT: &str = "[provider.api_error] 429 You've reached your 5-hour usage limit";

/// Kimi Code's account-quota screen. The error may wrap; a terminal's
/// line break is whitespace. The same words on another harness are not it.
pub(super) fn account_quota(output: &[String]) -> Option<String> {
    let flat = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    if !flat.contains(USAGE_LIMIT) {
        return None;
    }
    Some(
        output
            .iter()
            .find(|line| {
                line.split_whitespace().collect::<Vec<_>>().join(" ").contains("5-hour usage limit")
            })
            .cloned()
            .unwrap_or_else(|| USAGE_LIMIT.to_string()),
    )
}

/// Kimi's footer badge for one or more background shells, not an agent.
fn background_task(line: &str) -> bool {
    let mut rest = line;
    while let Some((_, after)) = rest.split_once('[') {
        let Some((badge, tail)) = after.split_once(']') else { return false };
        rest = tail;
        let mut words = badge.split_whitespace();
        let (Some(count), Some(noun), Some(state)) = (words.next(), words.next(), words.next())
        else {
            continue;
        };
        if words.next().is_some() || state != "running" {
            continue;
        }
        let Ok(count) = count.parse::<u32>() else { continue };
        if (count == 1 && noun == "task") || (count > 1 && noun == "tasks") {
            return true;
        }
    }
    false
}

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".kimi-code";

const INDEX: &str = "session_index.jsonl";
const WIRE: &str = "agents/main/wire.jsonl";

/// Where Kimi keeps the sessions of every worktree: one index. The index
/// names each session file.
pub(super) fn session_layout(_cwd: &str) -> SessionLayout {
    SessionLayout::Index { file: INDEX.into() }
}

/// The turn `text`, one session's wire, records. The worktree was named by
/// the index that pointed here. Kimi records no child session. The wire's
/// other lines are huge and are not a turn: the type is the whole match, so
/// a prompt that mentions a turn is not one.
pub(super) fn recorded_turn(_cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
    session::turn_of(&notes, not_before)
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "agent.turn.started" => Some(Note { marker: Marker::Start, at, child: false }),
        "agent.turn.ended" | "turn.ended" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

/// Every session file `index` names, marked whether its line names `cwd`.
/// A worker starts in its role copy, so Kimi files its session under that
/// directory, not the worktree. A relative session is relative to the harness
/// directory.
pub(super) fn indexed_sessions(cwd: &str, index: &str) -> Vec<Indexed> {
    let mut out = Vec::new();
    for value in json_lines(index) {
        let Some(session) = value["sessionDir"].as_str() else { continue };
        if session.is_empty() {
            continue;
        }
        let here = value["workDir"].as_str().is_some_and(|dir| session::same_cwd(dir, cwd));
        let file = std::path::Path::new(session).join(WIRE);
        out.push(Indexed { file: file.to_string_lossy().into_owned(), here });
    }
    out
}

/// Whether a wire's `turn.prompt` names the task brief at `brief`. The type
/// is the whole match, as for a turn: the model's own words or a tool's
/// output that quote the brief are not the prompt.
pub(super) fn names_brief(text: &str, brief: &str) -> bool {
    json_lines(text).any(|value| {
        let mut said = Vec::new();
        strings(&value, &mut said);
        value["type"] == "turn.prompt" && said.iter().any(|text| session::names_path(text, brief))
    })
}

/// Every string `value` holds, however deep.
fn strings<'a>(value: &'a Json, out: &mut Vec<&'a str>) {
    match value {
        Json::String(text) => out.push(text),
        Json::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Json::Object(fields) => fields.values().for_each(|field| strings(field, out)),
        Json::Null | Json::Bool(_) | Json::Number(_) => {}
    }
}

/// `existing` with this worktree's line replaced. Lines for other worktrees,
/// and a line that is not JSON, stay.
pub(super) fn session_index(cwd: &str, existing: &str) -> String {
    let mut kept = String::new();
    for line in existing.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            kept.push_str(line);
            kept.push('\n');
            continue;
        };
        if value["workDir"].as_str().is_some_and(|dir| session::same_cwd(dir, cwd)) {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    let session = format!("sessions/afkconvoy-{}", session::mark(cwd));
    kept.push_str(
        &session::object(vec![
            ("sessionId", session::mark(cwd).into()),
            ("sessionDir", session.into()),
            ("workDir", cwd.into()),
        ])
        .to_string(),
    );
    kept.push('\n');
    kept
}

/// The wire a stand-in writes for this turn. The index is `session_index`.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let ms = at.saturating_mul(1000);
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("type", "agent.turn.started".into()),
            ("time", ms.into()),
            ("turnId", 0_u64.into()),
            ("kind", "event".into()),
        ])
    );
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("type", "agent.turn.ended".into()),
                ("time", ms.into()),
                ("turnId", 0_u64.into()),
                ("outcome", "completed".into()),
            ])
        ));
    }
    vec![SessionFile {
        path: format!("sessions/afkconvoy-{}/{WIRE}", session::mark(cwd)),
        body,
        compressed: false,
        replaces: None,
    }]
}

/// The names Kimi gives the counts of a usage record.
const USAGE_KEYS: Keys = Keys {
    input: "inputOther",
    cache_read: "inputCacheRead",
    cache_write: "inputCacheCreation",
    output: "output",
    reasoning: "reasoning",
};

/// What one Kimi session line says of usage: each usage record, keyed by
/// its time and agent. The loop events that repeat a usage are not records.
pub(super) fn usage(value: &Json, _line: usize) -> Option<Usage> {
    if value["type"] != "usage.record" || !value["usage"].is_object() {
        return None;
    }
    let Some(counts) = Counts::read(&value["usage"], &USAGE_KEYS, false) else {
        return Some(Usage::Uncounted);
    };
    Some(Usage::Message {
        key: format!("{}:{}", value["time"], value["agentId"]),
        model: value["model"].as_str().unwrap_or("").to_string(),
        counts,
    })
}

/// The windows Kimi Code's answer to its `/usage` says. Its session keeps
/// none: `/usage` asks `<base url>/usages` with the account's token, and the
/// answer's `usages` hold each window under Kimi's own name (`limit_5h`,
/// `limit_7d`, `limit_month_total`, `limit_month_code`), each with a
/// `used_ratio` from 0 to 1 and an ISO `reset_time`.
pub(super) fn windows(answer: &Json) -> Vec<Window> {
    let Some(usages) = answer["usages"].as_object() else { return Vec::new() };
    usages
        .iter()
        .filter_map(|(name, window)| {
            Some(Window {
                name: name.clone(),
                used_percent: window::number(&window["used_ratio"])? * 100.0,
                minutes: None,
                resets_at: window::seconds(&window["reset_time"]),
            })
        })
        .collect()
}

/// What one Kimi wire line says of a tool call: a loop event appended to the
/// context, a `tool.call` by its `toolCallId` or the `tool.result` of one,
/// `isError` where Kimi marks it an error (a command that exited other than
/// 0 is one). A sub-agent writes a wire of its own; a line of another agent
/// than `main` is not the session's own.
pub(super) fn said(value: &Json) -> Vec<Said> {
    if value["type"] != "context.append_loop_event" {
        return Vec::new();
    }
    if value["agentId"].as_str().is_some_and(|agent| agent != "main") {
        return Vec::new();
    }
    let event = &value["event"];
    let Some(id) = event["toolCallId"].as_str().filter(|id| !id.is_empty()) else {
        return Vec::new();
    };
    let id = id.to_string();
    let said = match event["type"].as_str() {
        Some("tool.call") => Said::Call {
            id,
            action: steps::action(event["name"].as_str().unwrap_or(""), &event["args"]),
        },
        Some("tool.result") => Said::Result {
            id,
            result: steps::content(&event["result"]["output"]),
            error: event["result"]["isError"] == true,
            at: value["time"].as_u64().map(session::epoch_number),
        },
        _ => return Vec::new(),
    };
    vec![said]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    #[test]
    fn its_5_hour_usage_limit_is_an_account_quota_and_a_wrapped_line_still_is() {
        let line = "Error: [provider.api_error] 429 You've reached your 5-hour usage limit. \
                    Your quota will reset when the current 5-hour window";
        let screen = lines(&[line, "ends. To continue now."]);
        assert_eq!(account_quota(&screen).as_deref(), Some(line));
        let wrapped =
            lines(&["Error: [provider.api_error] 429 You've reached your", "5-hour usage limit."]);
        assert_eq!(account_quota(&wrapped).as_deref(), Some("5-hour usage limit."));
        assert!(account_quota(&lines(&["429 You've reached your 5-hour usage limit"])).is_none());
        assert!(Harness::Pi.account_quota(&screen).is_none());
        assert!(Harness::Codex.account_quota(&screen).is_none());
        assert!(Harness::ClaudeCode.account_quota(&wrapped).is_none());
    }

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            added: "/copies/acme/team/def",
            model,
            hook: "/bin/afkconvoy guard",
            kept: "/cache/harness/kimi-role",
            host_user: false,
            kept_config: false,
            dsh_profile: crate::start::DshProfile::Fleet,
            host_rules: &[],
            shadowed: &[],
            told: Told::default(),
            env: crate::harness::stage_env(crate::harness::Harness::Kimi, model),
        }
    }

    #[test]
    fn a_worker_starts_in_its_role_with_explicit_skills_and_no_shell_expansion() {
        let line = Harness::Kimi.in_role(STOCK, &start(Some("kimi-code/k3")));
        assert!(line.starts_with("cd '/copies/acme/dev/abc' && export KIMI_CODE_TRUST_WORKSPACE=1 KIMI_CODE_NO_AUTO_UPDATE=1 KIMI_DISABLE_TELEMETRY=1 KIMI_CODE_HOME='/cache/harness/kimi-role'"), "{line}");
        assert!(line.contains(" AFKCONVOY_HARNESS='kimi' AFKCONVOY_MODEL='kimi-code/k3' && kimi --auto --add-dir '/wt/issue 7' --add-dir '/copies/acme/team/def' --skills-dir '/copies/acme/dev/abc/.claude/skills' --model 'kimi-code/k3'"), "{line}");
        let odd = Start { kept: "/it's kept/$HOME", host_user: true, ..start(None) };
        let line = Harness::Kimi.in_role("kimi", &odd);
        assert!(line.contains("KIMI_CODE_HOME='/it'\\''s kept/$HOME'"), "{line}");
        assert!(line.contains("--skills-dir '/it'\\''s kept/$HOME/kimi-skills' --skills-dir '/it'\\''s kept/$HOME/agent-skills'"), "{line}");
        assert!(!line.contains("--model"), "{line}");
    }

    #[test]
    fn configuration_keeps_model_routing_but_not_user_hooks_or_settings() {
        let user = r#"
default_model = "kimi-code/k3"
default_permission_mode = "manual"
telemetry = true
[providers.managed]
type = "kimi"
api_key = "secret"
[models."kimi-code/k3"]
provider = "managed"
model = "k3"
max_context_size = 100
[hooks]
event = "PreToolUse"
command = "user hook"
[mcp]
startup_timeout_ms = 1
"#;
        let role = r#"
[[hooks]]
event = "PreToolUse"
command = "role hook"
[[permission.rules]]
decision = "deny"
pattern = "Bash(rm -rf*)"
"#;
        let config = configuration(Some(user), Some(role), Some("/bin/afkconvoy guard")).unwrap();
        let parsed: Toml = config.parse().unwrap();
        assert_eq!(parsed["default_model"].as_str(), Some("kimi-code/k3"));
        assert_eq!(parsed["providers"]["managed"]["api_key"].as_str(), Some("secret"));
        assert_eq!(parsed["default_permission_mode"].as_str(), Some("auto"));
        assert!(parsed.get("telemetry").is_none() && parsed.get("mcp").is_none());
        let hooks = parsed["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1 + STATE_EVENTS.len() + 1);
        assert!(hooks[0]["command"].as_str().unwrap().contains("/bin/afkconvoy guard"));
        assert!(hooks.iter().any(|hook| hook["command"].as_str() == Some("role hook")));
        assert!(!hooks.iter().any(|hook| hook["command"].as_str() == Some("user hook")));
        assert_eq!(parsed["permission"]["rules"][0]["decision"].as_str(), Some("deny"));
        assert!(hooks[0]["command"].as_str().unwrap().ends_with("exit 2"));
        assert!(REPORT.contains("kimi-hook.sh"));
    }

    #[test]
    fn probes_use_the_isolated_home_and_no_interactive_auto_flag() {
        assert_eq!(
            Harness::Kimi.models_question(STOCK, "/it's kept"),
            "export KIMI_CODE_HOME='/it'\\''s kept' && kimi provider list --json"
        );
        assert_eq!(
            Harness::Kimi.probe_question(STOCK, "/k", Some("a b"), "Say OK."),
            "export KIMI_CODE_HOME='/k' && kimi --model 'a b' --prompt 'Say OK.' --output-format stream-json"
        );
        assert_eq!(
            Harness::Kimi.prompt_question("mykimi --verbose", "/k", None, "Say OK."),
            "export KIMI_CODE_HOME='/k' && mykimi --verbose --prompt 'Say OK.'"
        );
    }

    #[test]
    fn models_and_the_answered_model_are_read_from_kimi_json() {
        let listed = r#"banner
{"providers":{},"models":{"kimi-code/k3":{"display_name":"K3"},"other/x":{"model":"x"}}}"#;
        assert_eq!(Harness::Kimi.models(listed).unwrap(), ["kimi-code/k3", "other/x"]);
        assert_eq!(Harness::Kimi.shown(listed), [("kimi-code/k3".into(), "K3".into())]);
        assert_eq!(Harness::Kimi.models("{}"), None);
        let answered =
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"model\":\"k3\"}}\n";
        assert_eq!(
            Harness::Kimi.answered_as(answered),
            super::super::AnsweredAs::Said("k3".into())
        );
        assert_eq!(Harness::Kimi.answered_as("OK\n"), super::super::AnsweredAs::Unsaid);
    }

    #[test]
    fn role_limits_and_calls_are_read_in_kimi_s_own_form() {
        let some = "[[hooks]]\nevent='PreToolUse'\ncommand='x'\n";
        assert!(limited_file(some).unwrap());
        assert!(limited_file("[[permission.rules]]\ndecision='deny'\npattern='Bash'\n").unwrap());
        assert!(!limited_file("[[permission.rules]]\ndecision='ask'\npattern='Bash'\n").unwrap());
        assert!(limited_file("[hooks]\nevent='PreToolUse'\n").is_err());
        let bash = r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m x"}}"#;
        let judged = call(serde_yaml::Deserializer::from_str(bash)).unwrap();
        assert_eq!(judged, Call::Shell("git commit -m x".into()));
        let other = r#"{"tool_name":"Edit","tool_input":{"command":"git commit"}}"#;
        assert_eq!(call(serde_yaml::Deserializer::from_str(other)).unwrap(), Call::Other);
    }

    #[test]
    fn its_footer_badge_says_a_background_shell_is_still_running() {
        let screen =
            |lines: &[&str]| lines.iter().map(|line| (*line).to_string()).collect::<Vec<_>>();
        assert!(shows_background(&screen(&[
            "Error: [provider.api_error] 429 You've reached your 5-hour usage limit.",
            "❯",
            "kimi-code/k3.1-pro  [1 task running]",
        ])));
        assert!(shows_background(&screen(&["❯", "[2 tasks running]", "ctx 10%"])));
        assert!(shows_background(&screen(&["[1 task running]", "ctx 10%"])));
        assert!(!shows_background(&screen(&["[1 task running]", "❯", "kimi-code/k3.1-pro"])));
        assert!(!shows_background(&screen(&["❯", "[1 agent running]"])));
        assert!(!shows_background(&screen(&["❯", "[1 tasks running]"])));
        assert!(!shows_background(&screen(&["❯", "[0 tasks running]"])));
        assert!(!shows_background(&screen(&[])));
    }

    #[test]
    fn kimi_is_logged_in_when_it_has_a_configured_model() {
        let kimi = Harness::Kimi;
        assert_eq!(kimi.install(), "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash");
        let login = kimi.login().unwrap();
        assert_eq!((login.question, login.line), ("kimi provider list --json", "kimi login"));
        assert_eq!(login.logged_out, "is not logged in or has no models configured");
        let listed = r#"{"models":{"kimi-code/k3":{"display_name":"K3"}}}"#;
        assert!(kimi.logged_in(true, listed));
        assert!(!kimi.logged_in(false, listed));
        assert!(!kimi.logged_in(true, "{}"));
        assert!(kimi.own_home_outside_role());
    }
}

#[cfg(test)]
mod usage_tests {
    use crate::harness::Harness;
    use crate::tokens::Counts;

    #[test]
    fn only_a_usage_record_counts() {
        let text = r#"{"type":"usage.record","time":1,"model":"a","usage":{"inputOther":10,"inputCacheRead":20,"inputCacheCreation":3,"output":5}}
{"type":"context.append_loop_event","event":{"usage":{"inputOther":10,"inputCacheRead":20,"inputCacheCreation":3,"output":5}}}
{"type":"usage.record","time":2,"model":"b","usage":{"inputOther":6,"inputCacheRead":7,"inputCacheCreation":1,"output":2}}"#;
        let tokens = Harness::Kimi.tokens(text, None).unwrap();
        assert_eq!(
            tokens["a"],
            Counts { input: 10, cache_read: 20, cache_write: 3, output: 5, reasoning: None }
        );
        assert_eq!(
            tokens["b"],
            Counts { input: 6, cache_read: 7, cache_write: 1, output: 2, reasoning: None }
        );
    }
}

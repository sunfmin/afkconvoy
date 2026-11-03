//! Codex, as a harness: everything of a worker's start, and of the guard's
//! asking, that is Codex's own. It is started as the session a human has at a
//! terminal, never as one run of a prompt: the fleet sends to, nudges and
//! wakes a worker after it starts. It has no way to be told to leave the host
//! user's configuration out, so it is given a user directory of the fleet's
//! own (docs/adr/0013). It writes its configuration file in that directory
//! by itself: what the fleet says to it is said on the line that starts it,
//! and in no shared file — each session's user directory is private, so a
//! saved /model choice cannot affect a later worker. The hooks that ask the
//! guard and tell the worker backend are said on the command too. It loads
//! rules from the directory it starts in and from no
//! directory added to it: a role's it is told to load from the file every
//! harness does, and the repo's own and the team rules are said on the line
//! too. It finds skills in directories of its own and not where a role's are:
//! the fleet's copy of a role links the one to the other. No flag has it look
//! nowhere else, so it also loads the skills of the host user's home, wherever
//! its user directory is: the one thing of the host user's a worker is left
//! with where it is given none of the others (docs/adr/0013). Given them
//! (docs/adr/0017), it finds the host user's skills and rules where it finds a
//! user's, in its user directory, linked there; of two skills of one name it
//! shows both, so the fleet turns off the host user's where a role's stands
//! for it.

use std::collections::BTreeMap;

use flux_rs::attrs::*;
use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::{Held, Kept, Link, SessionFile, SessionLayout};
use crate::guard::Call;
use crate::line::Line;
use crate::role::{Rules, Start, Told};
use crate::team::RULES;
use crate::turn::{Marker, Note, Turn};

/// Codex's task-refusal screen reported on sunfmin/control#98, 2026-10-26.
pub(super) fn task_refusal(output: &[String]) -> Option<String> {
    let screen = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    let words = "We take extra care with some cybersecurity requests. If you're doing authorized security work, apply for Daybreak to get broader access.";
    screen.contains(words).then(|| words.to_string())
}

/// Codex's capacity words reported on sunfmin/control#104, 2026-10-26.
pub(super) fn capacity_stop(output: &[String]) -> Option<String> {
    let screen = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    let words = "Selected model is at capacity. Please try a different model.";
    screen.contains(words).then(|| words.to_string())
}

/// Codex's usage-limit screen, an account quota. The two clauses may wrap
/// and differ in case; both have to be on the screen. Another harness's
/// screen that says only that a usage limit was reached is not this one.
pub(super) fn account_quota(output: &[String]) -> Option<String> {
    let screen = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = screen.to_lowercase();
    if !lower.contains("usage limit reached")
        || !lower.contains("increase your limits to continue using codex")
    {
        return None;
    }
    Some(
        output
            .iter()
            .find(|line| line.to_lowercase().contains("usage limit reached"))
            .cloned()
            .unwrap_or_else(|| output.join("\n")),
    )
}

pub(super) const NAME: &str = "Codex";
pub(super) const KEY: &str = "codex";
pub(super) const PROGRAM: &str = "codex";
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".codex/AGENTS.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("codex");
/// Codex with the flag that lets it work with nobody at its terminal, and in
/// no sandbox: a worker is limited by the guard (docs/adr/0012). A host that
/// wants a sandbox says a command of its own.
pub(super) const STOCK: &str = "codex --dangerously-bypass-approvals-and-sandbox";
pub(super) const TRUST: &str = "trust_level=\"trusted\"";
/// Its worktree trust is the `--config` flags `in_worktree` adds: a launch
/// checks the command takes `FLAGS`.
pub(super) const TRUST_IS_FLAGS: bool = true;

/// Codex's flag for a directory added to the one it was started in.
const ADD_DIR: &str = "--add-dir";
/// Codex's flag for one key of its configuration, said as `key=value` with
/// the value in TOML.
const CONFIG: &str = "--config";
/// Codex's flag that answers, for one run, the review it asks of every hook
/// nobody has reviewed: the fleet's, and a role's own. Asked at the terminal,
/// the review would take the worker's brief for its answer.
const HOOKS_REVIEWED: &str = "--dangerously-bypass-hook-trust";
/// Codex's flag for the model of the session.
pub(super) const MODEL: &str = "--model";
/// Every flag the fleet adds: what a worker launch command that starts
/// Codex must take.
pub(super) const FLAGS: [&str; 4] = [ADD_DIR, HOOKS_REVIEWED, CONFIG, MODEL];
/// What Codex is asked for the flags it takes.
const HELP: &str = " --help";

/// What Codex is asked for the models it knows: those its login runs, which
/// it fetches from its server with that login and keeps a while. Not the
/// ones it was built with (`--bundled`): those differ from an account's.
const LIST_MODELS: &str = " debug models";
/// The catalog also carries hundreds of kilobytes of model instructions.
/// Keep only names before it reaches a terminal's bounded scrollback: losing
/// the opening question mark makes a successful listing look like a timeout.
const MODEL_NAMES: &str = " | awk '{ while (match($0, /\"slug\"[[:space:]]*:[[:space:]]*\"[^\"]+\"/)) \
    { print substr($0, RSTART, RLENGTH); $0 = substr($0, RSTART + RLENGTH) } }'";
/// What comes before each model's name in what Codex answers: the key of it,
/// in the JSON it prints. A `"` inside a JSON string is written `\"`, so this
/// is in the answer only where a key is.
const NAMED: &str = "\"slug\":\"";

/// The variable that names Codex's user directory: the fleet's own, in place
/// of the host user's.
const USER_DIR: &str = "CODEX_HOME";

/// What a host keeps for Codex: its user directory, of the fleet's own. Of
/// the host user's it holds the login, and nothing else but, in the directory
/// kept for a worker given them, their skills and rules: no hook or
/// configuration — so no provider the user defined, which Codex keeps in
/// that configuration. It is a link: a login Codex renews is
/// written where the host user's own is, and neither goes stale. A user
/// whose login is in a keyring has no such file, and no worker a login.
pub(super) const KEPT: Kept = Kept {
    links: &[Link { home: ".codex/auth.json", kept: "auth.json" }],
    host_user: &[
        Link { home: ".codex/skills", kept: "skills" },
        Link { home: ".codex/AGENTS.override.md", kept: "AGENTS.override.md" },
        Link { home: ".codex/AGENTS.md", kept: "AGENTS.md" },
    ],
    files: &[],
};
/// The directories of the host user's home Codex finds skills in beside a
/// role's: the one it reads wherever its user directory is, and the one in
/// its user directory, which the directory kept for a worker given the host
/// user's skills links to.
pub(super) const SHADOWED_IN: [&str; 2] = [".agents/skills", ".codex/skills"];
/// The key of Codex's configuration that turns a skill off, by the path of
/// its file.
const SKILLS_CONFIG: &str = "skills.config";

/// The key of Codex's configuration under which it is told, of each
/// directory, whether to load what the directory holds of Codex's own: a
/// role's limits in Codex's form among it. Untold, Codex asks at its
/// terminal. The key is given whole, as one table: Codex cuts a dotted key
/// at every dot, and a directory's path holds some.
const DIRECTORIES: &str = "projects";
/// The key of Codex's configuration that names its hooks, by event.
const HOOKS: &str = "hooks";
/// The event of Codex before each tool call: where the guard is asked.
const BEFORE_A_CALL: &str = "PreToolUse";
/// The events of Codex at which a worker's state changes, the one before
/// each tool call apart (`BEFORE_A_CALL`): what the worker backend hears of,
/// to report it (`REPORT`). With the user's hooks left out, nothing else
/// tells it.
const STATE_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "PostToolUse",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];
/// How long Codex waits on one report, in seconds: a worker backend that
/// does not answer holds no tool call up.
const REPORT_TIMEOUT: u64 = 10;
/// Keeps Codex from carrying what one worker learned to the next on this
/// host: a role is what its repo says, the same on every host.
const NO_MEMORY: &str = "features.memories=false";

/// Keeps out the skills Codex carries itself: a role's skills are what its
/// directory says. It is all of its own discovery of skills Codex can be told
/// to leave out.
const NO_CARRIED_SKILLS: &str = "skills.bundled.enabled=false";
/// The directories of the one it starts in where Codex finds skills. A role's
/// are in neither (`team::SKILLS`): the fleet's copy of a role that holds
/// none of these links the first to the role's skills.
pub(super) const SKILLS: [&str; 2] = [".agents/skills", ".codex/skills"];

/// The files Codex loads the rules of a directory from before any other,
/// and in their place: it loads the first of them a directory holds.
pub(super) const BEFORE_THE_RULES: [&str; 2] = ["AGENTS.override.md", "AGENTS.md"];
/// The key of Codex's configuration that names the files it loads a
/// directory's rules from where the directory holds none of
/// `BEFORE_THE_RULES`: the one every harness loads them from. A name with a
/// path in it is left out by Codex, so this reaches no other directory.
const RULES_FILES: &str = "project_doc_fallback_filenames";
/// Has Codex load the rules of the directory it starts in alone. It loads
/// those of every directory from the root of a project down, and with no
/// file that marks a root there is none: nothing above a role's directory is
/// the role's (docs/adr/0014).
const NO_ROOT: &str = "project_root_markers=[]";
/// The key of Codex's configuration whose text is put before the model as a
/// developer's: the one way Codex is given rules from outside the directory
/// it starts in.
const TOLD: &str = "developer_instructions";
/// The files the repo's own rules are read from, for Codex: the first of
/// them the worktree holds, as pi reads a directory's.
pub(super) const REPO_RULES: [&str; 2] = ["AGENTS.md", RULES];
/// What a worker is told before the rules it is told, of what they are.
const TOLD_OPENS: &str = "afkconvoy started you, and gives you the rules below. You work under \
     them as you do under your role's, which are those of the directory you started in.";
/// What each set of rules a worker is told is said to be.
const REPO_S_OWN: &str = "The repo's own rules";
const TEAM_S: &str = "The team rules";

/// What Codex runs the guard as, around the line that runs it. Codex takes a
/// hook for refusing only when it exits `REFUSED` having said why, and makes
/// the call on any other failure: so a guard that refuses, one that is not
/// there and one that fails saying nothing all end the same way here.
const GUARD_ASKED: (&str, &str) = (
    "said=$(",
    " 2>&1 >/dev/null) && exit 0; \
     printf '%s\\n' \"${said:-afkconvoy guard: it could not be asked}\" >&2; exit 2",
);

/// The shell line that tells orca of a worker's state: the hook orca installs
/// on its host for Codex, which reads the event on standard input and knows
/// its terminal from the environment orca gave it. orca names it in the
/// user's hooks, which a worker is given none of. On a host where orca
/// installed none, nothing is told. Whatever the hook does, the line says
/// nothing and succeeds: Codex takes what a hook says for an answer, and a
/// report has none to give — only the guard refuses a call.
const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/codex-hook.sh\"; if [ -x \"$hook\" ]; \
     then /bin/sh \"$hook\"; else cat; fi >/dev/null 2>&1; exit 0";

/// The tool Codex runs shell lines with. Its tool that edits files is handed
/// a `command` too — the text of a patch — and is another tool.
const SHELL_TOOL: &str = "Bash";
/// The exit code that tells Codex a hook refuses the call.
pub(super) const REFUSED: u8 = 2;

/// Where a role's limits for Codex are written, in its directory: the hooks
/// Codex loads from the directory it starts in, once it is told to
/// (`DIRECTORIES`).
pub(super) const LIMITS: &str = ".codex/hooks.json";

/// A role's hooks for Codex, as far as they say whether they limit a worker.
#[derive(Deserialize)]
struct Hooks {
    /// Each event of Codex's, with the hooks it runs at it.
    #[serde(default)]
    hooks: BTreeMap<String, Vec<IgnoredAny>>,
}

/// Whether what a role's directory holds at `LIMITS` limits a worker: a file
/// that names a hook at some event. One that names none — a skeleton's —
/// holds no limit.
pub(super) fn limited<'de, D: Deserializer<'de>>(held: Held<D>) -> Result<bool, D::Error> {
    match held {
        Held::Nothing => Ok(false),
        Held::Entries(_) => Err(D::Error::custom("it is a directory, not a file of hooks")),
        Held::File(hooks) => {
            let hooks = Hooks::deserialize(hooks)?;
            Ok(hooks.hooks.values().any(|hooks| !hooks.is_empty()))
        }
    }
}

/// `text` as a TOML string, quotes and all.
fn toml_text(text: &str) -> String {
    let mut toml = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                toml.push('\\');
                toml.push(c);
            }
            c if c.is_control() => toml.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => toml.push(c),
        }
    }
    toml.push('"');
    toml
}

/// What tells Codex to load what the directory `dir` holds of Codex's own,
/// as one key of its configuration.
fn trusted(dir: &str) -> String {
    format!("{DIRECTORIES}={{{}={{trust_level=\"trusted\"}}}}", toml_text(dir))
}

/// The hooks added to a worker's, as one key of Codex's configuration: the
/// guard, asked before every tool call with the line `guard`, and the report,
/// the line `report` run wherever the worker's state changes. Codex adds them
/// to the hooks a role holds, so a role's run beside them and no role is
/// without the guard (docs/adr/0012) — or unheard of by the worker backend,
/// which would read a worker it has no report of off its terminal alone.
fn added_hooks(guard: &str, report: &str) -> String {
    let (before, after) = GUARD_ASKED;
    let guard =
        format!("{{type=\"command\",command={}}}", toml_text(&format!("{before}{guard}{after}")));
    let report =
        format!("{{type=\"command\",command={},timeout={REPORT_TIMEOUT}}}", toml_text(report));
    let mut hooks = format!("{HOOKS}={{{BEFORE_A_CALL}=[{{hooks=[{guard},{report}]}}]");
    for event in STATE_EVENTS {
        hooks.push_str(&format!(",{event}=[{{hooks=[{report}]}}]"));
    }
    hooks.push('}');
    hooks
}

/// Adds the fleet's hooks to an otherwise prepared interactive command.
pub(super) fn guarded(command: &str, guard: &str) -> String {
    Line::new()
        .launch(command)
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .word(&added_hooks(guard, REPORT))
        .done()
}

/// The file Codex is told to load a directory's rules from, as one key of
/// its configuration.
fn rules_files() -> String {
    format!("{RULES_FILES}=[{}]", toml_text(RULES))
}

/// One set of rules a worker is told, said to be `whose`, with the file it
/// was read from.
fn told_rules(whose: &str, rules: Rules) -> String {
    format!("\n\n# {whose}: {}\n\n<rules>\n{}\n</rules>", rules.file, rules.text.trim_end())
}

/// The rules `told`, each said to be what it is, as one key of Codex's
/// configuration. `None` when there are none to tell.
fn told(told: &Told) -> Option<String> {
    let repo = told.repo.map(|rules| told_rules(REPO_S_OWN, rules));
    let team = told.team.map(|rules| told_rules(TEAM_S, rules));
    if repo.is_none() && team.is_none() {
        return None;
    }
    let text = format!("{TOLD_OPENS}{}{}", repo.unwrap_or_default(), team.unwrap_or_default());
    Some(format!("{TOLD}={}", toml_text(&text)))
}

/// What turns off the skills `shadowed`, each by the path of its file, as
/// one key of Codex's configuration. `None` when there are none.
fn turned_off(shadowed: &[String]) -> Option<String> {
    if shadowed.is_empty() {
        return None;
    }
    let off: Vec<String> =
        shadowed.iter().map(|path| format!("{{path={},enabled=false}}", toml_text(path))).collect();
    Some(format!("{SKILLS_CONFIG}=[{}]", off.join(",")))
}

/// `line`, which exports a worker's environment, with what Codex reads from
/// it: the user directory the host keeps.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ").fixed(USER_DIR).fixed("=").word(start.kept)
}

/// Starts Codex outside a role, with the same trust and session isolation
/// as a role's worker. Repo rules load from the working directory itself;
/// there is no role, team, stage or guard to add.
pub(super) fn in_worktree(command: &str, worktree: &str, kept: &str) -> String {
    let line = Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command);
    let line = trust(line, worktree).fixed(" ").fixed(CONFIG).fixed(" ").word(&rules_files());
    isolated(line).done()
}

/// Starts one watch interactively, with Codex's trust and session isolation.
pub(super) fn watch(command: &str, worktree: &str, kept: &str, model: Option<&str>) -> String {
    let line = Line::new().launch(&in_worktree(command, worktree, kept));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Codex keeps each session apart from every other. These names are the
/// facts a worker start, the directory the host keeps and a watch start
/// ask the harness for (docs/adr/0013).
pub(super) const SESSION: super::Session = super::Session {
    fresh: "codex",
    note: HOME_NOTE,
    config: ".codex/config.toml",
    record: "convoy-codex-launch.json",
};

/// The key of the user's config that names the model of an ordinary session.
const PREFERENCE_MODEL: &str = "model";
/// The key of the user's config that names the effort of that model.
const EFFORT: &str = "model_reasoning_effort";
/// The key of the user's config that names the service tier of a session.
const SERVICE_TIER: &str = "service_tier";
const USER_SOURCE: &str = "user_selection";
const DEFAULT_SOURCE: &str = "codex_default";
const LABEL_SOURCE: &str = "model_label";

fn config_text(text: &str, file: &str) -> Result<toml::Table, String> {
    text.parse().map_err(|_| format!("cannot parse {file}; fix its TOML"))
}

/// The model an automatic start uses. An explicit model is that model.
/// Otherwise the user's, and it must be one the host kit lists.
pub(super) fn automatic_model(
    explicit: Option<&str>,
    text: &str,
    file: &str,
    listed: &[String],
) -> Result<(Option<String>, &'static str), String> {
    if let Some(model) = explicit {
        return Ok((Some(model.to_string()), LABEL_SOURCE));
    }
    match config_text(text, file)?.get(PREFERENCE_MODEL) {
        Some(toml::Value::String(model)) if !model.trim().is_empty() => {
            if !listed.iter().any(|listed| listed == model) {
                return Err(format!(
                    "Codex user selection `{model}` from {file}:{PREFERENCE_MODEL} is not listed \
                     by the host kit; confirm the model is available to this Codex login and \
                     refresh `afkconvoy kit`"
                ));
            }
            Ok((Some(model.clone()), USER_SOURCE))
        }
        None => Ok((None, DEFAULT_SOURCE)),
        _ => Err(format!(
            "{file}: {PREFERENCE_MODEL} must be a nonempty string; fix it or remove it for \
             Codex's default"
        )),
    }
}

fn preference(text: &str, file: &str, key: &str) -> Result<Option<String>, String> {
    match config_text(text, file)?.get(key) {
        Some(toml::Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        None => Ok(None),
        _ => Err(format!(
            "{file}: {key} must be a nonempty string; fix it or remove it for Codex's default"
        )),
    }
}

/// The effort of a start, from the same file as the model, read on its own:
/// a model the issue names does not skip it.
pub(super) fn effort(text: &str, file: &str) -> Result<(Option<String>, String), String> {
    let effort = preference(text, file, EFFORT)?;
    let source = match &effort {
        Some(_) => format!("{file}:{EFFORT}"),
        None => DEFAULT_SOURCE.to_string(),
    };
    Ok((effort, source))
}

/// The service tier of a worker start. With none, Codex chooses its own.
pub(super) fn service_tier(text: &str, file: &str) -> Result<Option<String>, String> {
    preference(text, file, SERVICE_TIER)
}

pub(super) fn outside_issue(model: &str) -> String {
    format!(
        "Codex user selection `{model}` from ~/{} is outside this issue's role range; update the \
         role's roster to allow it or choose a model the role allows",
        SESSION.config
    )
}

pub(super) fn outside_role(model: &str, role: &str) -> String {
    format!(
        "Codex user selection `{model}` from ~/{} is outside role `{role}`'s range; update the \
         role's roster to allow it or choose a model the role allows",
        SESSION.config
    )
}

/// What this session saved after it started. `None` when `text` is not Codex's
/// config: the launch's record stays as it was.
pub(super) fn saved(file: &str, text: &str) -> Option<super::Saved> {
    let config: toml::Table = text.parse().ok()?;
    let model = config.get(PREFERENCE_MODEL).and_then(toml::Value::as_str).map(str::to_string);
    let effort = config.get(EFFORT).and_then(toml::Value::as_str).map(str::to_string);
    Some(super::Saved {
        model: model.clone(),
        model_source: model.map(|_| format!("session:{file}:{PREFERENCE_MODEL}")),
        effort: effort.clone(),
        effort_source: effort.map(|_| format!("session:{file}:{EFFORT}")),
    })
}

/// `command` naming `model`, unless it already does. The model is said once.
pub(super) fn ensure_model(command: &str, model: Option<&str>) -> String {
    match model {
        Some(model) if !command.contains(MODEL) => {
            Line::new().launch(command).fixed(" ").fixed(MODEL).fixed(" ").word(model).done()
        }
        _ => command.to_string(),
    }
}

fn with_preference(line: &str, key: &str, preference: Option<&str>) -> String {
    let line = Line::new().launch(line);
    match preference {
        Some(preference) => {
            let value = format!("{key}={}", toml::Value::String(preference.to_string()));
            line.fixed(" --config ").word(&value).done()
        }
        None => line.done(),
    }
}

/// `line` with the effort, said as Codex takes one key of its configuration.
pub(super) fn with_effort(line: &str, effort: Option<&str>) -> String {
    with_preference(line, EFFORT, effort)
}

/// `line` with the service tier, said as Codex takes one key of its
/// configuration. None adds nothing, leaving Codex's default in force.
pub(super) fn with_service_tier(line: &str, service_tier: Option<&str>) -> String {
    with_preference(line, SERVICE_TIER, service_tier)
}

/// A watch's launch line: the worker's, with search, the model inside the
/// command, and the effort after it.
pub(super) fn session_watch(
    command: &str,
    worktree: &str,
    kept: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> String {
    let command = format!("{command} --search");
    let command = match model {
        Some(model) => {
            Line::new().launch(&command).fixed(" ").fixed(MODEL).fixed(" ").word(model).done()
        }
        None => command,
    };
    with_effort(&in_worktree(&command, worktree, kept), effort)
}

/// Answers both questions Codex would otherwise ask before loading the
/// directory's own configuration. Adding this never adds a launch command.
#[spec(fn(Line[@l], &str) -> Line[l])]
fn trust(line: Line, dir: &str) -> Line {
    line.fixed(" ").fixed(HOOKS_REVIEWED).fixed(" ").fixed(CONFIG).fixed(" ").word(&trusted(dir))
}

/// Keeps a worker's session from carrying memories to another worker, or
/// adding bundled skills. Adding this never adds a launch command.
#[spec(fn(Line[@l]) -> Line[l])]
fn isolated(line: Line) -> Line {
    line.fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .fixed(NO_MEMORY)
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .fixed(NO_CARRIED_SKILLS)
}

/// `line`, which starts the worker launch command, with what Codex is given
/// to put a worker in its role: the worktree added, the review of its hooks
/// and the question about its role's directory answered, the role's rules
/// and none from above them, the repo's own and the team rules, the guard and
/// the report, no memory, none of the skills it carries nor of the host
/// user's a role's stands for, and the stage's model.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    let line = line.fixed(" ").fixed(ADD_DIR).fixed(" ").word(start.worktree);
    let line = trust(line, start.role_dir)
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .word(&rules_files())
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .word(NO_ROOT);
    let line = match told(&start.told) {
        Some(told) => line.fixed(" ").fixed(CONFIG).fixed(" ").word(&told),
        None => line,
    };
    let line =
        isolated(line.fixed(" ").fixed(CONFIG).fixed(" ").word(&added_hooks(start.guard, REPORT)));
    let line = match turned_off(start.shadowed) {
        Some(off) => line.fixed(" ").fixed(CONFIG).fixed(" ").word(&off),
        None => line,
    };
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

/// The line that asks `command`, a worker launch command of Codex's, which
/// models it knows, under the user directory `kept` a worker of it is given.
pub(super) fn models_question(command: &str, kept: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command)
        .fixed(LIST_MODELS)
        .fixed(MODEL_NAMES)
        .done()
}

/// The line that sends `model` of Codex's the one-line `prompt` through
/// `command`, under the user directory `kept` a worker of it is given, and
/// prints its answer: one session, outside any repo, kept nowhere.
pub(super) fn prompt_question(
    command: &str,
    kept: &str,
    model: Option<&str>,
    prompt: &str,
) -> String {
    let line = Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command);
    let line = line.fixed(" exec --skip-git-repo-check --ephemeral");
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" ").word(prompt).done()
}

/// What Codex's probe adds: its events as JSON, a line each. None says
/// which model answered; the first names the thread, whose session file,
/// kept under `SESSIONS` (`answered_as`), does.
const AS_JSON: &str = " --json";

/// Where Codex keeps a session, under its user directory: in a file whose
/// name ends with its thread's id and `SESSION_ENDS`.
const SESSIONS: &str = "sessions";
const SESSION_ENDS: &str = ".jsonl";

/// The line that probes `model` with `prompt` through `command`, under the
/// user directory `kept`, as `prompt_question` sends it, printing its events
/// in JSON — and keeping its session, which says the model that answered
/// (`answered_as`), and which the probe deletes.
pub(super) fn probe_question(
    command: &str,
    kept: &str,
    model: Option<&str>,
    prompt: &str,
) -> String {
    let line = Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command);
    let line = line.fixed(" exec --skip-git-repo-check").fixed(AS_JSON);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" ").word(prompt).done()
}

/// What comes before the thread's id in Codex's events, and before the
/// model in its session file's turn, with no whitespace.
const THREAD: &str = "\"thread_id\":\"";
const TURN: &str = "\"type\":\"turn_context\"";
const TURN_MODEL: &str = "\"model\":\"";

/// Where `answer`, what `probe_question` printed, says which model answered:
/// the session file of the thread it names.
pub(super) fn answered_as(answer: &str) -> super::AnsweredAs {
    let dense: String = answer.split_whitespace().collect();
    let thread = dense.split_once(THREAD).and_then(|(_, after)| after.split_once('"'));
    match thread {
        Some((thread, _)) if !thread.is_empty() => super::AnsweredAs::InSession {
            under: SESSIONS,
            ends: format!("{thread}{SESSION_ENDS}"),
        },
        _ => super::AnsweredAs::Unsaid,
    }
}

/// The model of the first turn `session`, a session file of Codex's, holds.
pub(super) fn answered_in_session(session: &str) -> Option<String> {
    let dense: String = session.split_whitespace().collect();
    let (_, turn) = dense.split_once(TURN)?;
    let (_, after) = turn.split_once(TURN_MODEL)?;
    let (model, _) = after.split_once('"')?;
    (!model.is_empty()).then(|| model.to_string())
}

/// The models Codex lists in `answer`, what it printed for `models_question`,
/// each by the name its `MODEL` flag takes: the `slug` of each model in the
/// JSON there, whatever a shell printed around it, the ones Codex shows in no
/// menu among them. `None` when it lists none: it said something else.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let dense: String = answer.split_whitespace().collect();
    let names = dense.split(NAMED).skip(1).filter_map(|after| after.split_once('"'));
    let listed: Vec<String> =
        names.map(|(name, _)| name.to_string()).filter(|name| !name.is_empty()).collect();
    if listed.is_empty() { None } else { Some(listed) }
}

/// One tool call, as Codex describes it to a hook before making it.
#[derive(Deserialize)]
struct Described {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    /// The shell line, when the tool runs one; of the tool that edits files,
    /// the text of the patch.
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

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".codex";

/// The note beside a worktree's git directory whose text is the home Codex
/// kept for that worktree. The launch writes it; the reader opens it.
pub(super) const HOME_NOTE: &str = "convoy-codex-home";

/// Where Codex keeps a session of any worktree: every JSONL file under
/// [`SESSIONS`]. Which worktree a file is belongs to the file.
pub(super) fn session_layout(_cwd: &str) -> SessionLayout {
    SessionLayout::Tree { dir: SESSIONS.into(), extension: "jsonl" }
}

/// The turn `text` records for this worker. Codex starts in the role copy
/// when bound: its initial brief pointer identifies that worktree instead
/// of the session cwd. A header and a turn are still required.
pub(super) fn recorded_turn(
    cwd: &str,
    text: &str,
    not_before: Option<u64>,
    brief: Option<&str>,
) -> Option<Turn> {
    let mut saw_header = false;
    let mut notes = Vec::new();
    for value in json_lines(text) {
        if value["type"] == "session_meta" {
            let recorded = value["payload"]["cwd"].as_str().unwrap_or("");
            if !session::same_cwd(recorded, cwd)
                && !brief.is_some_and(|brief| names_brief(text, brief))
            {
                return None;
            }
            saw_header = true;
        }
        if let Some(note) = note(&value) {
            notes.push(note);
        }
    }
    if !saw_header {
        return None;
    }
    session::turn_of(&notes, not_before)
}

/// Only the first brief pointer in the initial user messages associates a
/// role session with a worker. Codex may write AGENTS.md and environment
/// messages before it. A later mention, tool output or model reply cannot
/// take another worker's session.
pub(super) fn names_brief(text: &str, brief: &str) -> bool {
    for value in json_lines(text) {
        let payload = &value["payload"];
        if value["type"] == "response_item"
            && payload["role"] != "developer"
            && payload["role"] != "system"
            && (payload["role"] == "assistant" || payload["type"] != "message")
        {
            return false;
        }
        if let Some(matches) = session::brief_match(&value, brief) {
            return matches;
        }
    }
    false
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let payload = &value["payload"];
    match payload["type"].as_str()? {
        "task_started" => {
            let at =
                session::number_at(payload["started_at"].as_u64(), value["timestamp"].as_str())?;
            Some(Note { marker: Marker::Start, at, child: false })
        }
        "task_complete" => {
            let at =
                session::number_at(payload["completed_at"].as_u64(), value["timestamp"].as_str())?;
            Some(Note { marker: Marker::End, at, child: false })
        }
        "turn_aborted" => {
            let at = session::parse_iso(value["timestamp"].as_str()?)?;
            Some(Note { marker: Marker::End, at, child: false })
        }
        _ => None,
    }
}

/// One session file a stand-in writes for this turn. An older file of the
/// same name, from another day, is replaced.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let name = format!("rollout-afkconvoy-{}.jsonl", session::mark(cwd));
    let (year, month, day) = session::civil_from_days((at / 86_400) as i64);
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("timestamp", iso.clone().into()),
            ("type", "session_meta".into()),
            ("payload", session::object(vec![("cwd", cwd.into()), ("id", "afkconvoy".into())]),),
        ])
    );
    body.push_str(&format!(
        "{}\n",
        session::object(vec![
            ("timestamp", iso.clone().into()),
            ("type", "event_msg".into()),
            (
                "payload",
                session::object(vec![
                    ("type", "task_started".into()),
                    ("started_at", at.into()),
                    ("turn_id", "afkconvoy".into()),
                ]),
            ),
        ])
    ));
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("timestamp", iso.into()),
                ("type", "event_msg".into()),
                (
                    "payload",
                    session::object(vec![
                        ("type", "task_complete".into()),
                        ("completed_at", at.into()),
                        ("turn_id", "afkconvoy".into()),
                    ]),
                ),
            ])
        ));
    }
    vec![SessionFile {
        path: format!("{SESSIONS}/{year:04}/{month:02}/{day:02}/{name}"),
        body,
        compressed: false,
        replaces: Some(name),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::line::quoted;
    use crate::role::{Facts, On, Told};

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    #[test]
    fn its_usage_limit_is_an_account_quota_and_a_wrapped_line_still_is() {
        let line = "■ Usage limit reached. You've reached your usage limit. \
                    Increase your limits to continue using codex.";
        assert_eq!(account_quota(&lines(&[line])).as_deref(), Some(line));
        let wrapped =
            lines(&["Usage limit", "reached. Increase your limits to", "continue using codex."]);
        assert_eq!(account_quota(&wrapped).as_deref(), Some(wrapped.join("\n").as_str()));
        // The watch's transcript spells Codex with a capital and a semicolon.
        let watched = "Usage limit reached; increase your limits to continue using Codex";
        assert_eq!(account_quota(&lines(&[watched])).as_deref(), Some(watched));
        assert!(
            account_quota(&lines(&["Usage limit reached. Increase your limits to continue."]))
                .is_none()
        );
        assert!(
            account_quota(&lines(&["Increase your limits to continue using codex."])).is_none()
        );
        assert!(Harness::ClaudeCode.account_quota(&lines(&[line])).is_none());
        assert!(Harness::Kimi.account_quota(&lines(&[line])).is_none());
        assert!(Harness::Pi.account_quota(&lines(&[line])).is_none());
    }

    /// The hooks that add the guard `/bin/afkconvoy guard` and the report.
    fn added() -> String {
        let report = format!(r#"{{type="command",command={},timeout=10}}"#, toml_text(REPORT));
        let mut added = format!(
            r#"hooks={{PreToolUse=[{{hooks=[{{type="command",command="said=$(/bin/afkconvoy guard 2>&1 >/dev/null) && exit 0; printf '%s\\n' \"${{said:-afkconvoy guard: it could not be asked}}\" >&2; exit 2"}},{report}]}}]"#
        );
        for event in STATE_EVENTS {
            added.push_str(&format!(",{event}=[{{hooks=[{report}]}}]"));
        }
        added + "}"
    }

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model,
            guard: "/bin/afkconvoy guard",
            kept: "/cache/harness/codex",
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
        assert_eq!(
            Harness::Codex.in_role(STOCK, &start(Some("gpt-5.5"))),
            format!(
                "cd '/copies/acme/dev/abc' && export CODEX_HOME='/cache/harness/codex' \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' \
             AFKCONVOY_HARNESS='codex' AFKCONVOY_MODEL='gpt-5.5' && \
             codex --dangerously-bypass-approvals-and-sandbox --add-dir '/wt/issue 7' \
             --dangerously-bypass-hook-trust \
             --config 'projects={{\"/copies/acme/dev/abc\"={{trust_level=\"trusted\"}}}}' \
             --config 'project_doc_fallback_filenames=[\"CLAUDE.md\"]' \
             --config 'project_root_markers=[]' \
             --config {} --config features.memories=false \
             --config skills.bundled.enabled=false --model 'gpt-5.5'",
                quoted(&added())
            )
        );
    }

    #[test]
    fn a_stage_with_no_model_names_none_and_nothing_is_read_by_the_shell_or_as_toml() {
        let odd = Start {
            role_dir: "/it's \"here\"/a\\b/$HOME",
            guard: "/it's \"here\"/afkconvoy guard",
            kept: "/it's kept/$HOME",
            ..start(None)
        };
        let line = Harness::Codex.in_role("kim  --search", &odd);
        assert!(line.starts_with("cd '/it'\\''s \"here\"/a\\b/$HOME' && export "), "{line}");
        assert!(line.contains(" export CODEX_HOME='/it'\\''s kept/$HOME' "), "{line}");
        assert!(line.contains(" && kim  --search --add-dir '/wt/issue 7' "), "{line}");
        let ends = " --config features.memories=false --config skills.bundled.enabled=false";
        assert!(line.ends_with(ends), "{line}");
        assert_eq!(
            trusted(odd.role_dir),
            r#"projects={"/it's \"here\"/a\\b/$HOME"={trust_level="trusted"}}"#
        );
        let hooks = added_hooks(odd.guard, r#"tell "$HOME""#);
        assert!(
            hooks.contains(r#"command="said=$(/it's \"here\"/afkconvoy guard 2>&1 "#),
            "{hooks}"
        );
        assert!(hooks.contains(r#"command="tell \"$HOME\"",timeout=10"#), "{hooks}");
        assert_eq!(toml_text("a\tb\u{7f}"), r#""a\u0009b\u007F""#);
    }

    #[test]
    fn the_worker_backend_is_told_wherever_a_worker_s_state_changes_and_refuses_nothing() {
        let hooks = added_hooks("/bin/afkconvoy guard", REPORT);
        assert_eq!(hooks, added());
        // At every event orca itself names its hook at, each with the timeout.
        let report = format!("{{type=\"command\",command={},timeout=10}}", toml_text(REPORT));
        assert!(REPORT.contains("/.orca/agent-hooks/codex-hook.sh"));
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PermissionRequest",
            "PostToolUse",
            "SubagentStart",
            "SubagentStop",
            "Stop",
        ] {
            assert!(hooks.contains(&format!(",{event}=[{{hooks=[{report}]}}]")), "{event}");
        }
        // Before a tool call it is told beside the guard, which alone has no
        // timeout — and alone can end in the code that refuses.
        assert!(hooks.contains(&format!("exit 2\"}},{report}]}}],SessionStart=")), "{hooks}");
        assert!(REPORT.ends_with(">/dev/null 2>&1; exit 0"));
    }

    #[test]
    fn the_repo_s_own_rules_and_the_team_rules_are_told_each_said_to_be_what_it_is() {
        let repo = Rules { file: "/wt/issue 7/AGENTS.md", text: "Run `make`.\nIt's \"so\".\n\n" };
        let team = Rules { file: "/copies/acme/team/def/CLAUDE.md", text: "Be kind." };
        let told = |told: Told| {
            let line = Harness::Codex.in_role(STOCK, &Start { told, ..start(None) });
            let (before, after) = line.split_once(" --config 'project_root_markers=[]' ").unwrap();
            assert!(before.ends_with("=[\"CLAUDE.md\"]'"), "{line}");
            after.split_once("--config 'hooks=").unwrap().0.to_string()
        };
        // A repo with no rules of its own, and a team with none: nothing is
        // told, and the worker starts all the same.
        assert_eq!(told(Told::default()), "");
        assert_eq!(
            told(Told { repo: Some(repo), team: Some(team), role: None }),
            format!(
                "--config 'developer_instructions=\"{TOLD_OPENS}\\u000A\\u000A\
                 # The repo'\\''s own rules: /wt/issue 7/AGENTS.md\\u000A\\u000A\
                 <rules>\\u000ARun `make`.\\u000AIt'\\''s \\\"so\\\".\\u000A</rules>\\u000A\\u000A\
                 # The team rules: /copies/acme/team/def/CLAUDE.md\\u000A\\u000A\
                 <rules>\\u000ABe kind.\\u000A</rules>\"' ",
                TOLD_OPENS = TOLD_OPENS.replace('\'', "'\\''")
            )
        );
        let one = told(Told { repo: None, team: Some(team), role: None });
        assert!(one.contains("# The team rules: ") && !one.contains("own rules"), "{one}");
        let one = told(Told { repo: Some(repo), team: None, role: None });
        assert!(one.contains("s own rules: ") && !one.contains("The team rules"), "{one}");
    }

    #[test]
    fn a_role_holds_no_file_codex_would_load_in_place_of_its_rules() {
        assert_eq!(Harness::Codex.before_the_rules(), ["AGENTS.override.md", "AGENTS.md"]);
        // The repo's own rules are read as pi reads a directory's.
        assert_eq!(Harness::Codex.told(), ["AGENTS.md", RULES]);
        assert!(Harness::Pi.told().is_empty() && Harness::ClaudeCode.told().is_empty());
    }

    #[test]
    fn codex_finds_skills_in_directories_of_its_own_and_no_other_harness_does() {
        assert_eq!(Harness::Codex.skills(), [".agents/skills", ".codex/skills"]);
        assert!(Harness::Pi.skills().is_empty() && Harness::ClaudeCode.skills().is_empty());
    }

    #[test]
    fn what_is_kept_for_codex_is_a_link_to_the_login_and_nothing_else() {
        assert_eq!(KEPT.links, [Link { home: ".codex/auth.json", kept: "auth.json" }]);
        assert!(KEPT.files.is_empty());
    }

    #[test]
    fn a_command_is_asked_for_the_flags_codex_takes() {
        let help = "Usage: codex [OPTIONS] [PROMPT]\n  -c, --config <key=value>\n  \
                    -m, --model <MODEL>\n      --dangerously-bypass-hook-trust\n      \
                    --add-dir <DIR>\n";
        assert_eq!(Harness::Codex.flags_question("kim"), "kim --help");
        assert_eq!(Harness::Codex.takes("kim", Some(help)), Ok(()));
        let old = help.replace("--dangerously-bypass-hook-trust", "");
        let why = Harness::Codex.takes("kim", Some(&old)).unwrap_err();
        assert!(why.contains("`kim --help` names no --dangerously-bypass-hook-trust."), "{why}");
        assert!(why.contains("must end in Codex taking flags"), "{why}");
    }

    #[test]
    fn a_role_holds_limits_for_codex_where_its_hooks_file_names_a_hook() {
        let limited = |hooks: &str| {
            Harness::Codex.limited(Held::File(serde_yaml::Deserializer::from_str(hooks)))
        };
        for none in ["{}", r#"{"hooks": {}}"#, r#"{"hooks": {"PreToolUse": []}}"#] {
            assert!(!limited(none).unwrap(), "{none}");
        }
        let some = r#"{"hooks": {"Stop": [], "PreToolUse": [{"matcher": "Bash", "hooks": []}]}}"#;
        assert!(limited(some).unwrap());
        assert!(limited(r#"{"hooks": ["PreToolUse"]}"#).is_err());
        let held = |held: Held<serde_yaml::Deserializer>| Harness::Codex.limited(held);
        assert!(!held(Held::Nothing).unwrap());
        assert!(held(Held::Entries(1)).unwrap_err().to_string().contains("a directory"));
    }

    const LISTED: &str = "a line an rc file printed\n\
        {\"models\":[{\"slug\":\"gpt-5.5\",\"display_name\":\"GPT-5.5\",\
        \"description\":\"said \\\"slug\\\":\\\"no-model\\\" once\",\"visibility\":\"list\"},\n\
        { \"slug\": \"codex-auto-review\", \"visibility\": \"hide\" }]}\n";

    #[test]
    fn codex_is_asked_its_models_under_the_directory_a_worker_is_given() {
        let question = Harness::Codex.models_question("kim  --search", "/it's kept");
        assert!(
            question.starts_with(
                "export CODEX_HOME='/it'\\''s kept' && kim  --search debug models | awk "
            ),
            "{question}"
        );
        assert_eq!(Harness::Codex.models(LISTED).unwrap(), ["gpt-5.5", "codex-auto-review"]);
        for nothing in ["", "{\"models\":[]}\n", "error: unrecognized subcommand 'models'\n"] {
            assert_eq!(Harness::Codex.models(nothing), None, "{nothing}");
        }
        assert_eq!(Harness::Codex.provider_variable(), None);
    }

    #[test]
    fn codex_knows_a_model_it_lists_by_its_whole_name_and_no_other() {
        let listed = Harness::Codex.models(LISTED).unwrap();
        for known in ["gpt-5.5", " gpt-5.5 ", "codex-auto-review"] {
            assert!(Harness::Codex.knows(&listed, known), "{known}");
            assert!(Harness::Codex.names(&listed, known), "{known}");
        }
        for unknown in ["opus", "gpt", "GPT-5.5", "gpt-5.5:high", "openai/gpt-5.5", "no-model", ""]
        {
            assert!(!Harness::Codex.knows(&listed, unknown), "{unknown}");
            assert!(!Harness::Codex.names(&listed, unknown), "{unknown}");
        }
    }

    /// JSON is read here as the YAML it also is.
    fn judged(described: &str) -> Result<Call, serde_yaml::Error> {
        call(serde_yaml::Deserializer::from_str(described))
    }

    #[test]
    fn a_call_of_codex_s_shell_tool_is_a_shell_line_and_a_patch_s_text_is_not() {
        let bash = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_input":{"command":"git commit -m x"}}"#;
        assert_eq!(judged(bash).unwrap(), Call::Shell("git commit -m x".to_string()));
        for other in [
            r#"{"tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n+git push origin main\n*** End Patch"}}"#,
            r#"{"tool_name":"mcp__x__run","tool_input":{"command":"git commit"}}"#,
            r#"{"tool_name":"view_image","tool_input":{"path":"a.png"}}"#,
            r#"{"tool_name":"update_plan"}"#,
        ] {
            assert_eq!(judged(other).unwrap(), Call::Other, "{other}");
        }
        // pi's description is none of Codex's.
        for unread in [r#"{"toolName":"bash","input":{"command":"ls"}}"#, "{}", "[]", ""] {
            assert!(judged(unread).is_err(), "{unread}");
        }
    }

    #[test]
    fn a_worker_service_tier_is_an_explicit_nonempty_user_preference() {
        let file = "home/.codex/config.toml";
        for tier in ["default", "priority"] {
            let text = format!("service_tier = '{tier}'\n");
            assert_eq!(
                Harness::Codex.service_tier(&text, file).unwrap().unwrap().as_deref(),
                Some(tier)
            );
            let line = Harness::Codex.with_service_tier("codex", Some(tier));
            assert!(line.contains(&format!("--config 'service_tier=\"{tier}\"'")), "{line}");
        }
        for absent in
            ["", "model_verbosity = 'low'\n", "[profiles.saved]\nservice_tier = 'default'\n"]
        {
            assert_eq!(Harness::Codex.service_tier(absent, file).unwrap().unwrap(), None);
            assert_eq!(Harness::Codex.with_service_tier("codex", None), "codex");
        }
        for invalid in ["service_tier = 4\n", "service_tier = ''\n"] {
            let bad = Harness::Codex.service_tier(invalid, file).unwrap().unwrap_err();
            assert!(bad.contains("service_tier must be a nonempty string"), "{bad}");
        }
        assert!(Harness::Pi.service_tier("service_tier = 'default'", file).is_none());
    }

    #[test]
    fn a_session_s_model_effort_and_watch_line_are_codex_s() {
        assert!(
            Harness::ALL
                .iter()
                .all(|harness| harness.session().is_some() == (*harness == Harness::Codex))
        );
        let file = "home/.codex/config.toml";
        let (model, source) =
            Harness::Codex.automatic_model(Some("gpt-x"), "", file, &[]).unwrap().unwrap();
        assert_eq!((model.as_deref(), source), (Some("gpt-x"), "model_label"));
        let (model, source) = Harness::Codex
            .automatic_model(None, "model = 'gpt-x'\n", file, &["gpt-x".into()])
            .unwrap()
            .unwrap();
        assert_eq!((model.as_deref(), source), (Some("gpt-x"), "user_selection"));
        let (model, source) = Harness::Codex.automatic_model(None, "", file, &[]).unwrap().unwrap();
        assert_eq!((model, source), (None, "codex_default"));
        let missing = Harness::Codex
            .automatic_model(None, "model = 'nope'\n", file, &[])
            .unwrap()
            .unwrap_err();
        assert!(missing.contains("not listed by the host kit"), "{missing}");
        assert!(Harness::ClaudeCode.automatic_model(None, "", file, &[]).is_none());

        let (effort, source) =
            Harness::Codex.effort("model_reasoning_effort = 'high'\n", file).unwrap().unwrap();
        assert_eq!(effort.as_deref(), Some("high"));
        assert!(source.ends_with("model_reasoning_effort"), "{source}");
        let bad = Harness::Codex.effort("model_reasoning_effort = 4\n", file).unwrap().unwrap_err();
        assert!(bad.contains("must be a nonempty string"), "{bad}");

        let command = Harness::Codex.with_model("codex --yolo --search", "gpt-x");
        let inner = Harness::Codex.in_worktree(&command, "/wt", "/kept");
        let line = Harness::Codex.session_watch(
            "codex --yolo",
            "/wt",
            "/kept",
            Some("gpt-x"),
            Some("high"),
        );
        assert!(line.starts_with(&inner), "{line}");
        assert!(line.contains(" --search "), "{line}");
        assert!(line.contains("model_reasoning_effort"), "{line}");
        let plain = Harness::Codex.session_watch("codex --yolo", "/wt", "/kept", None, None);
        assert_eq!(plain, Harness::Codex.in_worktree("codex --yolo --search", "/wt", "/kept"));
        assert_eq!(
            Harness::Pi.session_watch("agent", "/wt", "/kept", Some("m"), Some("high")),
            Harness::Pi.watch("agent", "/wt", "/kept", Some("m"))
        );
    }
}

//! Codex, as a harness: everything of a worker's start, and of the guard's
//! asking, that is Codex's own. It is started as the session a human has at a
//! terminal, never as one run of a prompt: the fleet sends to, nudges and
//! wakes a worker after it starts. It has no way to be told to leave the host
//! user's configuration out, so it is given a user directory of the fleet's
//! own (docs/adr/0013). It writes its configuration file in that directory
//! by itself: what the fleet says to it is said on the line that starts it,
//! and in no file — the hook that asks the guard among it.

use std::collections::BTreeMap;

use flux_rs::attrs::*;
use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};

use super::{Held, Kept, Link};
use crate::guard::Call;
use crate::line::Line;
use crate::role::Start;

pub(super) const NAME: &str = "Codex";
pub(super) const KEY: &str = "codex";
pub(super) const PROGRAM: &str = "codex";
/// Codex with the flag that lets it work with nobody at its terminal, and in
/// no sandbox: a worker is limited by the guard (docs/adr/0012). A host that
/// wants a sandbox says a command of its own.
pub(super) const STOCK: &str = "codex --dangerously-bypass-approvals-and-sandbox";

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
const MODEL: &str = "--model";
/// Every flag the fleet adds: what a worker launch command that starts
/// Codex must take.
pub(super) const FLAGS: [&str; 4] = [ADD_DIR, HOOKS_REVIEWED, CONFIG, MODEL];
/// What Codex is asked for the flags it takes.
const HELP: &str = " --help";

/// The variable that names Codex's user directory: the fleet's own, in place
/// of the host user's.
const USER_DIR: &str = "CODEX_HOME";

/// What a host keeps for Codex: its user directory, of the fleet's own. Of
/// the host user's it holds the login, and nothing else: no rule, skill,
/// hook or configuration — so no provider the user defined, which Codex
/// keeps in that configuration. It is a link: a login Codex renews is
/// written where the host user's own is, and neither goes stale. A user
/// whose login is in a keyring has no such file, and no worker a login.
pub(super) const KEPT: Kept =
    Kept { links: &[Link { home: ".codex/auth.json", kept: "auth.json" }], files: &[] };

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
/// Keeps Codex from carrying what one worker learned to the next on this
/// host: a role is what its repo says, the same on every host.
const NO_MEMORY: &str = "features.memories=false";

/// What Codex runs the guard as, around the line that runs it. Codex takes a
/// hook for refusing only when it exits `REFUSED` having said why, and makes
/// the call on any other failure: so a guard that refuses, one that is not
/// there and one that fails saying nothing all end the same way here.
const GUARD_ASKED: (&str, &str) = (
    "said=$(",
    " 2>&1 >/dev/null) && exit 0; \
     printf '%s\\n' \"${said:-afkconvoy guard: it could not be asked}\" >&2; exit 2",
);

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

/// The hook that asks the guard before every tool call, with the line
/// `guard`, as one key of Codex's configuration. Codex adds it to the hooks a
/// role holds, so a role's run beside it and no role is without the guard
/// (docs/adr/0012).
fn guard_hook(guard: &str) -> String {
    let (before, after) = GUARD_ASKED;
    let asked = toml_text(&format!("{before}{guard}{after}"));
    format!("{HOOKS}={{{BEFORE_A_CALL}=[{{hooks=[{{type=\"command\",command={asked}}}]}}]}}")
}

/// `line`, which exports a worker's environment, with what Codex reads from
/// it: the user directory the host keeps.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ").fixed(USER_DIR).fixed("=").word(start.kept)
}

/// `line`, which starts the worker launch command, with what Codex is given
/// to put a worker in its role: the worktree added, the review of its hooks
/// and the question about its role's directory answered, the guard, no
/// memory, and the stage's model.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .fixed(HOOKS_REVIEWED)
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .word(&trusted(start.role_dir))
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .word(&guard_hook(start.guard))
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .fixed(NO_MEMORY);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::role::{Facts, On};

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model,
            guard: "/bin/afkconvoy guard",
            kept: "/cache/harness/codex",
            facts: Facts {
                on: On::Stage { issue: 7, flow: "feature", stage: "build" },
                role: "dev",
                skill: "implement",
                writes: false,
                consult: false,
                base_branch: "main",
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
            "cd '/copies/acme/dev/abc' && export CODEX_HOME='/cache/harness/codex' \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' \
             AFKCONVOY_HARNESS='codex' && \
             codex --dangerously-bypass-approvals-and-sandbox --add-dir '/wt/issue 7' \
             --dangerously-bypass-hook-trust \
             --config 'projects={\"/copies/acme/dev/abc\"={trust_level=\"trusted\"}}' \
             --config 'hooks={PreToolUse=[{hooks=[{type=\"command\",command=\"said=$(/bin/afkconvoy \
             guard 2>&1 >/dev/null) && exit 0; printf '\\''%s\\\\n'\\'' \\\"${said:-afkconvoy \
             guard: it could not be asked}\\\" >&2; exit 2\"}]}]}' \
             --config features.memories=false --model 'gpt-5.5'"
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
        assert!(line.ends_with(" --config features.memories=false"), "{line}");
        assert_eq!(
            trusted(odd.role_dir),
            r#"projects={"/it's \"here\"/a\\b/$HOME"={trust_level="trusted"}}"#
        );
        let hook = guard_hook(odd.guard);
        assert!(hook.contains(r#"command="said=$(/it's \"here\"/afkconvoy guard 2>&1 "#), "{hook}");
        assert_eq!(toml_text("a\tb\u{7f}"), r#""a\u0009b\u007F""#);
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

    #[test]
    fn codex_has_no_way_to_say_which_models_it_knows_and_no_provider_variable() {
        assert_eq!(Harness::Codex.models_question(STOCK, "/kept"), None);
        assert!(Harness::Codex.knows(&[], "anything"));
        assert_eq!(Harness::Codex.provider_variable(), None);
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
}

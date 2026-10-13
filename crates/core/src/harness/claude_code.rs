//! Claude Code, as a harness: everything of a worker's start, and of the
//! guard's asking, that is Claude Code's own. It loads a role's rules, skills
//! and hooks from the directory it starts in (docs/adr/0009), so what the
//! fleet adds is the rest: the directories beside the role's, the settings it
//! may load, and the hooks that ask the guard and tell the worker backend.

use flux_rs::attrs::*;
use serde::{Deserialize, Deserializer};

use crate::guard::Call;
use crate::line::Line;
use crate::role::Start;

pub(super) const NAME: &str = "Claude Code";
pub(super) const PROGRAM: &str = "claude";
/// Claude Code with the flag that lets it work with nobody at its terminal.
pub(super) const STOCK: &str = "claude --dangerously-skip-permissions";
/// The variable Claude Code reads a custom provider's address from.
pub(super) const PROVIDER: &str = "ANTHROPIC_BASE_URL";

/// Claude Code's flag for a directory added to the one it was started in.
const ADD_DIR: &str = "--add-dir";
/// Claude Code's flag for which settings it loads.
const SETTING_SOURCES: &str = "--setting-sources";
/// Claude Code's flag for the model of the session.
const MODEL: &str = "--model";
/// Claude Code's flag for settings added to those it loads.
const SETTINGS: &str = "--settings";
/// Every flag the fleet adds: what a worker launch command that starts
/// Claude Code must take.
pub(super) const FLAGS: [&str; 4] = [ADD_DIR, SETTING_SOURCES, SETTINGS, MODEL];
/// What Claude Code is asked for the flags it takes.
const HELP: &str = " --help";

/// The settings a worker loads: those of the directory it starts in — its
/// role's — and nothing of the host user's.
const SOURCES: &str = "project";
/// The event of Claude Code before each tool call: where the guard is asked.
const BEFORE_A_CALL: &str = "PreToolUse";
/// The events of Claude Code at which a worker's state changes: what the
/// worker backend hears of, to report it (`Start::report`). With the user's
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
/// Has Claude Code load the rules of the directories added to it. Their hooks
/// are not loaded: what a repo must enforce belongs in its gate.
const ADDED_RULES: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";
/// Keeps Claude Code from carrying what one worker learned to the next on
/// this host: a role is what its repo says, the same on every host.
const NO_MEMORY: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";
/// The variables a worker's environment sets for Claude Code.
pub(super) const ENVIRONMENT: [&str; 2] = [ADDED_RULES, NO_MEMORY];

/// The shell line that tells orca of a worker's state: the hook orca installs
/// on its host for Claude Code, which reads the event on standard input and
/// knows its terminal from the environment orca gave it. orca puts the same
/// line in the user's settings, which a worker in its role does not load. On
/// a host where orca installed none, nothing is told.
pub(super) const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/claude-hook.sh\"; if [ -x \"$hook\" ]; \
     then ORCA_HOOK_AGENT_PID=\"$PPID\" /bin/sh \"$hook\"; else cat >/dev/null; fi";

/// The tool Claude Code runs shell lines with.
const SHELL_TOOL: &str = "Bash";
/// The exit code that tells Claude Code a hook refuses the call.
pub(super) const REFUSED: u8 = 2;

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
/// before every tool call with the line `guard`, and the report, a hook it
/// runs with the line `report` wherever the worker's state changes. They are
/// added to the role's own, so a role's hooks run beside them and no role is
/// without the guard (docs/adr/0012) — or unheard of by the worker backend,
/// which would read a worker it has no report of off its terminal alone.
fn added_settings(guard: &str, report: &str) -> String {
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
    settings.push_str("}}");
    settings
}

/// `line`, which starts the worker launch command, with what Claude Code is
/// given to put a worker in its role: the worktree and the team's copy added,
/// the user's settings left out, the guard and the report added, and the
/// stage's model.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.team_dir)
        .fixed(" ")
        .fixed(SETTING_SOURCES)
        .fixed(" ")
        .fixed(SOURCES)
        .fixed(" ")
        .fixed(SETTINGS)
        .fixed(" ")
        .word(&added_settings(start.guard, start.report));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::line::quoted;
    use crate::role::{Facts, On};

    fn in_role(command: &str, start: &Start) -> String {
        Harness::ClaudeCode.in_role(command, start)
    }

    fn flags_question(command: &str) -> String {
        Harness::ClaudeCode.flags_question(command)
    }

    fn takes_flags(command: &str, answer: Option<&str>) -> Result<(), String> {
        Harness::ClaudeCode.takes(command, answer)
    }

    /// The settings that add the guard `/bin/afkconvoy guard` and the report
    /// `tell "$HOME"`.
    fn added() -> String {
        let report = r#"{"type":"command","command":"tell \"$HOME\"","timeout":10}"#;
        let mut added = format!(
            r#"{{"hooks":{{"PreToolUse":[{{"hooks":[{{"type":"command","command":"/bin/afkconvoy guard"}},{report}]}}]"#
        );
        for event in STATE_EVENTS {
            added.push_str(&format!(r#","{event}":[{{"hooks":[{report}]}}]"#));
        }
        added + "}}"
    }

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model,
            guard: "/bin/afkconvoy guard",
            report: r#"tell "$HOME""#,
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
        let line = in_role("kim --yolo", &start(Some("opus")));
        assert_eq!(
            line,
            "cd '/copies/acme/dev/abc' && export CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 \
             CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' && \
             kim --yolo --add-dir '/wt/issue 7' --add-dir '/copies/acme/team/def' \
             --setting-sources project --settings '"
                .to_string()
                + &added()
                + "' --model 'opus'"
        );
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
        let settings = added_settings(odd.guard, odd.report);
        assert!(settings.contains(r#""command":"/it's \"here\"/afk\\convoy guard""#), "{settings}");
        assert!(settings.contains(r#""command":"tell \"$HOME\"","timeout":10"#), "{settings}");
        assert!(in_role("claude", &odd).ends_with(&format!("{SETTINGS} {}", quoted(&settings))));
    }

    const HELP: &str = "Usage: claude [options]\n  --add-dir <directories...>\n  --model <model>\n  \
                        --setting-sources <sources>\n  --settings <file-or-json>\n";

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
        assert!(why.contains("bound to a team") && why.contains("\"agent\""), "{why}");
        for silent in [None, Some(""), Some(" \n")] {
            let why = takes_flags("agent ; true", silent).unwrap_err();
            assert!(why.contains("`agent ; true --help` failed in a login shell"), "{why}");
        }
    }
}

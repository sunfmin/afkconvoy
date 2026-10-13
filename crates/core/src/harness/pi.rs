//! pi, as a harness: everything of a worker's start, and of the guard's
//! asking, that is pi's own. It loads a role's rules from the directory it
//! starts in, and little else of what a worker is to have (docs/adr/0013), so
//! the fleet adds the rest: the role's skills, named; the answer to the
//! question pi asks before it loads a role's own files; a user directory of
//! the fleet's own, since pi has no way to be told to leave the host user's
//! out; and an extension the tool carries, which asks the guard and loads the
//! rules of the issue's worktree and the team rules.

use flux_rs::attrs::*;
use serde::{Deserialize, Deserializer};

use super::{Kept, Link};
use crate::guard::Call;
use crate::line::Line;
use crate::role::Start;
use crate::team::SKILLS;

pub(super) const NAME: &str = "pi";
pub(super) const KEY: &str = "pi";
pub(super) const PROGRAM: &str = "pi";
/// pi asks nothing before a tool call: it takes no flag to work with nobody
/// at its terminal.
pub(super) const STOCK: &str = "pi";

/// pi's flag that answers, for one run, the question it asks before it loads
/// what the directory it starts in holds of pi's own: a role's limits in pi's
/// form among it. Asked at the terminal, the question would take the
/// worker's brief for its answer.
const APPROVE: &str = "--approve";
/// pi's flag that leaves out every skill it would find by itself: those of
/// the host user's home among them, which it reads wherever its user
/// directory is.
const NO_SKILLS: &str = "--no-skills";
/// pi's flag for a directory of skills to load all the same.
const SKILL: &str = "--skill";
/// pi's flag for an extension to load.
const EXTENSION: &str = "--extension";
/// pi's flag for the model of the session.
const MODEL: &str = "--model";
/// Every flag the fleet adds: what a worker launch command that starts pi
/// must take.
pub(super) const FLAGS: [&str; 5] = [APPROVE, NO_SKILLS, SKILL, EXTENSION, MODEL];
/// What pi is asked for the flags it takes.
const HELP: &str = " --help";

/// The variable that names pi's user directory: the fleet's own, in place of
/// the host user's.
const USER_DIR: &str = "PI_CODING_AGENT_DIR";
/// The variables the fleet's extension reads the line that runs the guard,
/// the issue's worktree, and the fleet's copy of the team's directory, from.
const GUARD_ENV: &str = "AFKCONVOY_GUARD";
const WORKTREE_ENV: &str = "AFKCONVOY_WORKTREE";
const TEAM_ENV: &str = "AFKCONVOY_TEAM_DIR";

/// The fleet's extension, as the directory the host keeps for pi holds it.
/// It is named on the line that starts a worker and is not among those pi
/// finds by itself: a worker whose guard is not there does not start.
const EXTENSION_FILE: &str = "afkconvoy.js";

/// What a host keeps for pi: its user directory, of the fleet's own. Of the
/// host user's it holds what the host knows of providers — the logins, and
/// the models the user defined — and the extension the worker backend
/// installed to hear of a pi session's state; pi finds that one by itself,
/// so on a host where the backend installed none, nothing is told. They are
/// links: a login pi renews is renewed for the host user too, and neither
/// goes stale. No rule, skill, setting or other extension of the user's is
/// there.
pub(super) const KEPT: Kept = Kept {
    links: &[
        Link { home: ".pi/agent/auth.json", kept: "auth.json" },
        Link { home: ".pi/agent/models.json", kept: "models.json" },
        Link {
            home: ".pi/agent/extensions/orca-agent-status.ts",
            kept: "extensions/orca-agent-status.ts",
        },
    ],
    files: &[(EXTENSION_FILE, include_str!("pi/extension.js"))],
};

/// The tool pi runs shell lines with.
const SHELL_TOOL: &str = "bash";
/// The exit code with which the guard refuses a call of pi's: the fleet's
/// extension takes any but 0 for a refusal.
pub(super) const REFUSED: u8 = 2;

/// `line`, which exports a worker's environment, with what pi and the
/// fleet's extension read from it: the user directory the host keeps, the
/// line that runs the guard, the issue's worktree, and the team's directory,
/// whose rules a pi worker is given as it is its worktree's.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(start.kept)
        .fixed(" ")
        .fixed(GUARD_ENV)
        .fixed("=")
        .word(start.guard)
        .fixed(" ")
        .fixed(WORKTREE_ENV)
        .fixed("=")
        .word(start.worktree)
        .fixed(" ")
        .fixed(TEAM_ENV)
        .fixed("=")
        .word(start.team_dir)
}

/// `line`, which starts the worker launch command, with what pi is given to
/// put a worker in its role: its question answered, the role's skills and no
/// others, the fleet's extension, and the stage's model.
#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(APPROVE)
        .fixed(" ")
        .fixed(NO_SKILLS)
        .fixed(" ")
        .fixed(SKILL)
        .fixed(" ")
        .word(&format!("{}/{SKILLS}", start.role_dir))
        .fixed(" ")
        .fixed(EXTENSION)
        .fixed(" ")
        .word(&format!("{}/{EXTENSION_FILE}", start.kept));
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

/// One tool call, as the fleet's extension passes on what pi describes to it
/// before making the call.
#[derive(Deserialize)]
struct Described {
    #[serde(rename = "toolName")]
    tool_name: String,
    #[serde(default)]
    input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    /// The shell line, when the tool runs one.
    #[serde(default)]
    command: Option<String>,
}

/// The tool call `described` to the fleet's extension, as the guard judges
/// one.
pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.input.command {
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
            kept: "/cache/harness/pi",
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
            Harness::Pi.in_role("pi", &start(Some("openai/gpt-5.5"))),
            "cd '/copies/acme/dev/abc' && export PI_CODING_AGENT_DIR='/cache/harness/pi' \
             AFKCONVOY_GUARD='/bin/afkconvoy guard' AFKCONVOY_WORKTREE='/wt/issue 7' \
             AFKCONVOY_TEAM_DIR='/copies/acme/team/def' \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' \
             AFKCONVOY_HARNESS='pi' && \
             pi --approve --no-skills --skill '/copies/acme/dev/abc/.claude/skills' \
             --extension '/cache/harness/pi/afkconvoy.js' --model 'openai/gpt-5.5'"
        );
    }

    #[test]
    fn a_stage_with_no_model_names_none_and_nothing_is_read_by_the_shell() {
        let odd = Start { kept: "/it's here/$HOME", ..start(None) };
        let line = Harness::Pi.in_role("kim  --thinking high", &odd);
        assert!(line.contains(" export PI_CODING_AGENT_DIR='/it'\\''s here/$HOME' "), "{line}");
        assert!(
            line.ends_with(
                " && kim  --thinking high --approve --no-skills --skill \
                 '/copies/acme/dev/abc/.claude/skills' --extension \
                 '/it'\\''s here/$HOME/afkconvoy.js'"
            ),
            "{line}"
        );
    }

    #[test]
    fn the_extension_reads_its_facts_where_a_start_puts_them_and_is_kept_where_one_names_it() {
        let [(name, extension)] = KEPT.files else { panic!("one file is carried") };
        assert_eq!(*name, EXTENSION_FILE);
        for variable in [GUARD_ENV, WORKTREE_ENV, TEAM_ENV] {
            assert!(extension.contains(&format!("\"{variable}\"")), "it reads no {variable}");
        }
        // None of what is linked to is where pi would find the guard itself.
        assert!(KEPT.links.iter().all(|link| link.kept != EXTENSION_FILE));
    }

    #[test]
    fn a_command_is_asked_for_the_flags_pi_takes() {
        let help = "Usage:\n  pi [options]\n  --model <pattern>\n  --extension, -e <path>\n  \
                    --skill <path>\n  --no-skills, -ns\n  --approve, -a\n";
        assert_eq!(Harness::Pi.flags_question("kim"), "kim --help");
        assert_eq!(Harness::Pi.takes("kim", Some(help)), Ok(()));
        let old = help.replace("--approve, -a", "");
        let why = Harness::Pi.takes("kim", Some(&old)).unwrap_err();
        assert!(why.contains("`kim --help` names no --approve."), "{why}");
        assert!(why.contains("must end in pi taking flags"), "{why}");
    }

    /// JSON is read here as the YAML it also is.
    fn judged(described: &str) -> Result<Call, serde_yaml::Error> {
        call(serde_yaml::Deserializer::from_str(described))
    }

    #[test]
    fn a_call_of_pi_s_shell_tool_is_a_shell_line_and_any_other_is_not() {
        let bash = r#"{"toolName":"bash","input":{"command":"git commit -m x","timeout":5}}"#;
        assert_eq!(judged(bash).unwrap(), Call::Shell("git commit -m x".to_string()));
        for other in [
            r#"{"toolName":"write","input":{"path":"a","content":"git commit"}}"#,
            r#"{"toolName":"read","input":{"path":"a"}}"#,
            r#"{"toolName":"mcp__x__run","input":{"command":"git commit"}}"#,
            r#"{"toolName":"ls"}"#,
        ] {
            assert_eq!(judged(other).unwrap(), Call::Other, "{other}");
        }
        // Claude Code's description is none of pi's.
        for unread in [r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#, "{}", "[]", ""] {
            assert!(judged(unread).is_err(), "{unread}");
        }
    }
}

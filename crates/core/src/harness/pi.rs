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
use serde_json::Value;

use super::session::{self, json_lines};
use super::{Held, Kept, Link, SessionFile, SessionLayout};
use crate::guard::Call;
use crate::line::Line;
use crate::role::Start;
use crate::team::SKILLS;
use crate::turn::{Marker, Note, Turn};

pub(super) const NAME: &str = "pi";
pub(super) const KEY: &str = "pi";
pub(super) const PROGRAM: &str = "pi";
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".pi/agent/AGENTS.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("pi");
/// pi asks nothing before a tool call: it takes no flag to work with nobody
/// at its terminal.
pub(super) const STOCK: &str = "pi";

/// pi's flag that answers, for one run, the question it asks before it loads
/// what the directory it starts in holds of pi's own: a role's limits in pi's
/// form among it. Asked at the terminal, the question would take the
/// worker's brief for its answer.
const APPROVE: &str = "--approve";
pub(super) const TRUST: &str = APPROVE;
/// Its worktree trust is a flag (`APPROVE`): a launch checks the command
/// takes `FLAGS`.
pub(super) const TRUST_IS_FLAGS: bool = true;

/// Answers pi's worktree approval question on the launch line.
pub(super) fn in_worktree(command: &str) -> String {
    if command.contains(APPROVE) { command.to_string() } else { format!("{command} {APPROVE}") }
}

/// Starts one watch interactively, with pi's login and approval setup.
pub(super) fn watch(command: &str, kept: &str, model: Option<&str>) -> String {
    let line = Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(&in_worktree(command));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Loads the same extension as a role worker, without adding role rules.
pub(super) fn guarded(command: &str, worktree: &str, kept: &str, guard: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" AFKCONVOY_GUARD=")
        .word(guard)
        .fixed(" AFKCONVOY_WORKTREE=")
        .word(worktree)
        .fixed(" AFKCONVOY_TEAM_DIR=")
        .word(worktree)
        .fixed(" && ")
        .launch(command)
        .fixed(" ")
        .fixed(EXTENSION)
        .fixed(" ")
        .word(&format!("{kept}/{EXTENSION_FILE}"))
        .done()
}

/// pi's flag that leaves out every skill it would find by itself: those of
/// the host user's home among them, which it reads wherever its user
/// directory is. The host user's own are named, where a worker is given
/// them, from the user directory the host keeps.
const NO_SKILLS: &str = "--no-skills";
/// pi's flag for a directory of skills to load all the same. Of two skills
/// of one name pi keeps the one named first.
const SKILL: &str = "--skill";
/// pi's flag for an extension to load.
const EXTENSION: &str = "--extension";
/// pi's flag for the model of the session.
pub(super) const MODEL: &str = "--model";
/// Every flag the fleet adds: what a worker launch command that starts pi
/// must take.
pub(super) const FLAGS: [&str; 5] = [APPROVE, NO_SKILLS, SKILL, EXTENSION, MODEL];
/// What pi is asked for the flags it takes.
const HELP: &str = " --help";

/// What pi is asked for the models it knows.
const LIST_MODELS: &str = " --list-models";
/// What may follow a model's name and a `:`, where pi is given one: how hard
/// the model is to think.
const THINKING: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Where a role's limits for pi are written, in its directory: the
/// extensions pi loads from the directory it starts in, once its question is
/// answered (`APPROVE`).
pub(super) const LIMITS: &str = ".pi/extensions";
/// The files pi loads the rules of a directory from before the one every
/// harness loads (`team::RULES`), and in its place: it loads the first of
/// them a directory holds, and no other.
pub(super) const BEFORE_THE_RULES: [&str; 3] = ["AGENTS.override.md", "AGENTS.md", "AGENTS.MD"];

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
/// goes stale. No setting or other extension of the user's is there, and
/// their skills and rules only in the directory kept for a worker given them.
pub(super) const KEPT: Kept = Kept {
    links: &[
        Link { home: ".pi/agent/auth.json", kept: "auth.json" },
        Link { home: ".pi/agent/models.json", kept: "models.json" },
        Link {
            home: ".pi/agent/extensions/orca-agent-status.ts",
            kept: "extensions/orca-agent-status.ts",
        },
    ],
    host_user: &[
        Link { home: ".pi/agent/skills", kept: HOST_SKILLS },
        Link { home: ".pi/agent/AGENTS.override.md", kept: "AGENTS.override.md" },
        Link { home: ".pi/agent/AGENTS.md", kept: "AGENTS.md" },
        Link { home: ".pi/agent/CLAUDE.md", kept: "CLAUDE.md" },
    ],
    files: &[(EXTENSION_FILE, include_str!("pi/extension.js"))],
};
/// Where the user directory the host keeps for a worker given the host
/// user's skills and rules links to their skills. Their rules are linked to
/// where pi loads them from in its user directory, the first it finds — of
/// the names that differ in more than case: on a host that does not tell
/// case apart, one link would stand in for another.
const HOST_SKILLS: &str = "skills";

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
/// put a worker in its role: its question answered, the role's skills, then
/// the host user's where it is given them, and no others, the fleet's
/// extension, and the stage's model.
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
        .word(&format!("{}/{SKILLS}", start.role_dir));
    let line = if start.host_user {
        line.fixed(" ").fixed(SKILL).fixed(" ").word(&format!("{}/{HOST_SKILLS}", start.kept))
    } else {
        line
    };
    let line = line
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

/// Whether what a role's directory holds at `LIMITS` limits a worker: a
/// directory with an entry. What an extension refuses is not read.
pub(super) fn limited<D>(held: Held<D>) -> bool {
    match held {
        Held::Entries(entries) => entries > 0,
        Held::Nothing | Held::File(_) => false,
    }
}

/// The line that asks `command`, a worker launch command of pi's, which
/// models it knows, under the user directory `kept` a worker of it is given:
/// the logins and the models there are the ones a worker would have.
pub(super) fn models_question(command: &str, kept: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command)
        .fixed(LIST_MODELS)
        .done()
}

/// The line that sends `model` of pi's the one-line `prompt` through
/// `command`, under the user directory `kept` a worker of it is given, and
/// prints its answer.
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
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" --print ").word(prompt).done()
}

/// What pi's probe adds: its events as JSON, a line each, of which the
/// answer's names the provider and the model that answered.
const AS_JSON: &str = " --mode json";

/// The line that probes `model` with `prompt` through `command`, as
/// `prompt_question` sends it, printing its events in JSON (`answered_as`).
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
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(AS_JSON).fixed(" --print ").word(prompt).done()
}

/// What comes before the provider of a message, and between it and its
/// model, in pi's events, with no whitespace.
const PROVIDER: &str = "\"provider\":\"";
const THEN_MODEL: &str = "\",\"model\":\"";

/// The model that answered `probe_question`, by `answer`: as `provider/id`,
/// the last message's — the answer's — whatever a terminal broke its lines
/// at.
pub(super) fn answered_as(answer: &str) -> Option<String> {
    let dense: String = answer.split_whitespace().collect();
    dense
        .split(PROVIDER)
        .skip(1)
        .filter_map(|after| {
            let (provider, rest) = after.split_once(THEN_MODEL)?;
            let (id, _) = rest.split_once('"')?;
            let named = !provider.is_empty() && !provider.contains('"') && !id.is_empty();
            named.then(|| format!("{provider}/{id}"))
        })
        .last()
}

/// The models pi lists in `answer`, what it printed for `models_question`,
/// each as `provider/id`: a table under a line that heads it `provider
/// model`, whatever a shell printed above. `None` when it lists none: pi has
/// no login on this host, or said something else.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let mut listed = vec![];
    let mut headed = false;
    for line in answer.lines() {
        let mut columns = line.split_whitespace();
        let (Some(provider), Some(id)) = (columns.next(), columns.next()) else { continue };
        if headed {
            listed.push(format!("{provider}/{id}"));
        } else {
            headed = provider == "provider" && id == "model";
        }
    }
    if listed.is_empty() { None } else { Some(listed) }
}

/// Whether `model`, as a stage writes it, is one of `listed`, the models pi
/// lists on a host, as pi reads a model's name: a `provider/id`, an id, or a
/// part of an id — in one provider, where it names one — whatever the case,
/// with how hard to think after a `:` or not. pi takes more than this: a
/// name it has no login for, and any name at all after a provider it knows.
/// Neither would run a worker here, and neither is known.
pub(super) fn knows(listed: &[String], model: &str) -> bool {
    let thinks = model.rsplit_once(':').filter(|(_, level)| THINKING.contains(level));
    named(listed, model) || thinks.is_some_and(|(model, _)| named(listed, model))
}

/// Whether `model` is one of `listed` by its whole name, a `provider/id`,
/// with how hard to think after a `:` or not.
pub(super) fn names(listed: &[String], model: &str) -> bool {
    let whole = |name: &str| listed.iter().any(|full| full.eq_ignore_ascii_case(name.trim()));
    let thinks = model.rsplit_once(':').filter(|(_, level)| THINKING.contains(level));
    whole(model) || thinks.is_some_and(|(model, _)| whole(model))
}

/// Whether `name` names one of `listed`, each a `provider/id`.
fn named(listed: &[String], name: &str) -> bool {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return false;
    }
    let within = name.split_once('/').filter(|(_, part)| !part.is_empty());
    listed.iter().any(|full| {
        let full = full.to_lowercase();
        let (provider, id) = full.split_once('/').unwrap_or(("", &full));
        full == name
            || id.contains(&name)
            || within.is_some_and(|(of, part)| of == provider && id.contains(part))
    })
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

/// Z.ai's usage limit on pi's screen, reported on sunfmin/control#163.
/// An HTTP 429 whose body carries code 1308 and says the usage limit was
/// reached. The JSON may wrap across lines. The same body on another
/// harness's screen is not this quota.
pub(super) fn account_quota(output: &[String]) -> Option<String> {
    if let Some(line) = output.iter().find(|line| zai_limit(line)) {
        return Some(line.clone());
    }
    let flat = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    zai_limit(&flat).then(|| output.join("\n"))
}

/// Whether `text` is Z.ai's usage-limit error: status 429, then one JSON
/// body with code 1308 and a usage-limit message. A line the terminal
/// wrapped is not that body until the screen is joined.
fn zai_limit(text: &str) -> bool {
    let Some(start) = text.find('{') else { return false };
    let status = text[..start]
        .split(|character: char| !character.is_ascii_digit())
        .any(|digits| digits == "429");
    if !status {
        return false;
    }
    let Ok(body) = serde_json::from_str::<Value>(&text[start..]) else { return false };
    let known_code = body
        .get("code")
        .is_some_and(|code| code.as_str() == Some("1308") || code.as_u64() == Some(1308));
    let explicit_limit = body
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.to_lowercase().contains("usage limit reached"));
    known_code && explicit_limit
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

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".pi/agent";

/// Where pi keeps the session of `cwd`: one JSONL file for each session, in
/// the directory it names from the worktree path.
pub(super) fn session_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Files {
        dir: format!("sessions/{}", session::wrapped_dir(cwd)),
        extension: "jsonl",
    }
}

/// The turn `text` records for `cwd`. A session header for another worktree
/// is not this file. pi records no child session.
pub(super) fn recorded_turn(cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let mut notes = Vec::new();
    for value in json_lines(text) {
        if value["type"] == "session"
            && value["cwd"].as_str().is_some_and(|recorded| !session::same_cwd(recorded, cwd))
        {
            return None;
        }
        if let Some(note) = note(&value) {
            notes.push(note);
        }
    }
    session::turn_of(&notes, not_before)
}

fn note(value: &Value) -> Option<Note> {
    if value["type"] != "message" {
        return None;
    }
    let at = session::parse_iso(value["timestamp"].as_str()?)?;
    match value["message"]["role"].as_str()? {
        "user" => Some(Note { marker: Marker::Start, at, child: false }),
        "assistant" => match value["message"]["stopReason"].as_str()? {
            "stop" | "error" => Some(Note { marker: Marker::End, at, child: false }),
            _ => None,
        },
        _ => None,
    }
}

/// One session file a stand-in writes for this turn.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("type", "session".into()),
            ("cwd", cwd.into()),
            ("timestamp", iso.clone().into()),
        ])
    );
    body.push_str(&format!(
        "{}\n",
        session::object(vec![
            ("type", "message".into()),
            ("timestamp", iso.clone().into()),
            ("message", session::object(vec![("role", "user".into())])),
        ])
    ));
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("type", "message".into()),
                ("timestamp", iso.into()),
                (
                    "message",
                    session::object(vec![
                        ("role", "assistant".into()),
                        ("stopReason", "stop".into()),
                    ]),
                ),
            ])
        ));
    }
    vec![SessionFile {
        path: format!("sessions/{}/afkconvoy-turn.jsonl", session::wrapped_dir(cwd)),
        body,
        compressed: false,
        replaces: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::role::{Facts, On, Told};

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    #[test]
    fn zai_s_usage_limit_is_an_account_quota_and_a_wrapped_line_still_is() {
        let line = "Error: 429: {\"code\":\"1308\",\"message\":\"Usage limit reached for 5 hour. \
                    Your limit will reset at 2026-10-27 17:52:40\"}";
        assert_eq!(account_quota(&lines(&[line, "later"])).as_deref(), Some(line));
        let wrapped = lines(&[
            "Error: 429: {\"code\":\"1308\",\"message\":\"Usage limit reached for 5 hour. Your limit will reset at",
            "2026-10-27 17:52:40\"}",
        ]);
        assert_eq!(account_quota(&wrapped).as_deref(), Some(wrapped.join("\n").as_str()));
        let runtime = "429: {\"code\":\"1308\",\"message\":\"Usage limit reached for 5 hour\"}";
        assert_eq!(account_quota(&lines(&[runtime])).as_deref(), Some(runtime));
        let numeric = "429: {\"code\":1308,\"message\":\"Usage limit reached for 5 hour\"}";
        assert_eq!(account_quota(&lines(&[numeric])).as_deref(), Some(numeric));
        assert!(account_quota(&lines(&[&runtime.replacen("429", "1429", 1)])).is_none());
        assert!(account_quota(&lines(&[&runtime.replace("1308", "1309")])).is_none());
        assert!(
            account_quota(&lines(&[&runtime.replace("Usage limit reached", "slow down")]))
                .is_none()
        );
        assert!(Harness::Codex.account_quota(&lines(&[line])).is_none());
        assert!(Harness::Kimi.account_quota(&lines(&[runtime])).is_none());
        assert!(Harness::ClaudeCode.account_quota(&wrapped).is_none());
    }

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model,
            guard: "/bin/afkconvoy guard",
            kept: "/cache/harness/pi",
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
            Harness::Pi.in_role("pi", &start(Some("openai/gpt-5.5"))),
            "cd '/copies/acme/dev/abc' && export PI_CODING_AGENT_DIR='/cache/harness/pi' \
             AFKCONVOY_GUARD='/bin/afkconvoy guard' AFKCONVOY_WORKTREE='/wt/issue 7' \
             AFKCONVOY_TEAM_DIR='/copies/acme/team/def' \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' \
             AFKCONVOY_HARNESS='pi' AFKCONVOY_MODEL='openai/gpt-5.5' && \
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
        // The team rules are the file every harness loads them from.
        let team_rules = format!("const TEAM_RULES = [\"{}\"]", crate::team::RULES);
        assert!(extension.contains(&team_rules), "{extension}");
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

    #[test]
    fn a_role_holds_limits_for_pi_where_its_extensions_directory_has_an_entry() {
        let limited = |held: Held<()>| limited(held);
        assert!(limited(Held::Entries(1)));
        for none in [Held::Entries(0), Held::Nothing, Held::File(())] {
            assert!(!limited(none));
        }
    }

    const LISTED: &str = "a line an rc file printed\n\
        provider      model                context  max-out  thinking  images\n\
        openai-codex  gpt-5.5              272K     128K     yes       yes   \n\
        zai           glm-5.2              1M       131.1K   yes       no    \n\
        openrouter    moonshotai/kimi-k2.6 256K     64K      yes       no\n";

    #[test]
    fn pi_is_asked_its_models_under_the_directory_a_worker_is_given() {
        assert_eq!(
            Harness::Pi.models_question("kim  --thinking high", "/it's kept"),
            "export PI_CODING_AGENT_DIR='/it'\\''s kept' && kim  --thinking high --list-models"
        );
        assert_eq!(
            Harness::Pi.models(LISTED).unwrap(),
            ["openai-codex/gpt-5.5", "zai/glm-5.2", "openrouter/moonshotai/kimi-k2.6"]
        );
        for nothing in ["", "No models available. Use /login to sign in.\n", "provider model\n"] {
            assert_eq!(Harness::Pi.models(nothing), None, "{nothing}");
        }
    }

    #[test]
    fn pi_knows_a_model_it_lists_by_its_name_or_a_part_of_its_id() {
        let listed = Harness::Pi.models(LISTED).unwrap();
        for known in [
            "openai-codex/gpt-5.5",
            "OpenAI-Codex/GPT-5.5",
            "gpt-5.5",
            "glm",
            "zai/glm",
            "zai/glm-5.2:high",
            "glm-5.2:off",
            "moonshotai/kimi-k2.6",
            "openrouter/moonshotai/kimi-k2.6",
        ] {
            assert!(Harness::Pi.knows(&listed, known), "{known}");
        }
        for unknown in
            ["opus", "zai/gpt-5.5", "openai-codex/gpt-9", "glm-5.2:loud", "zai/", "", "codex"]
        {
            assert!(!Harness::Pi.knows(&listed, unknown), "{unknown}");
        }
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

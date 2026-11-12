//! DeepSeek Harness: an interactive TUI on the profile its role chooses. The only
//! added plugin asks the guard and reports running/idle to the worker backend.
use flux_rs::attrs::*;
use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::{Kept, Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::Start;
use crate::steps::{self, Said};
use crate::turn::{Marker, Note, Turn};

pub(super) const KEY: &str = "dsh";
pub(super) const NAME: &str = "DeepSeek Harness";
pub(super) const PROGRAM: &str = "dsh-tui";
/// The line that installs it.
pub(super) const INSTALL: &str = "npm install -g @deepseek-harness-tui/dsh-tui";
/// It is asked about no login.
pub(super) const LOGIN: Option<super::Login> = None;
/// The variable that names DSH's user directory.
pub(super) const USER_DIR: &str = "DSH_HOME";
/// The host its worker's login shell reaches without a proxy, unless the
/// host says otherwise.
pub(super) const NO_PROXY: &str = "api.deepseek.com";
/// Where, under the directory kept for it, the installed program's package
/// is linked: preferred to an editable profile copy, as the login shell a
/// worker gets finds it.
pub(super) const KEPT_PACKAGE: &str = "profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui";

/// Whether the answer to its login question says it is logged in: it is
/// asked none, so by the exit status alone.
pub(super) fn logged_in(succeeded: bool, _printed: &str) -> bool {
    succeeded
}
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".dsh/AGENTS.md";
/// `npx skills` names no agent for it: it reads the shared copies in
/// `~/.agents/skills`, which an install for any other agent writes.
pub(super) const SKILLS_AGENT: Option<&str> = None;
pub(super) const STOCK: &str = "dsh-tui";
pub(super) const TRUST: &str = "DSH_PERMISSION_MODE=danger-full-access";
/// Its worktree trust is a variable, not flags: outside a role a launch
/// adds no flag the command must take.
pub(super) const TRUST_IS_FLAGS: bool = false;
pub(super) const FLAGS: [&str; 1] = ["--patch"];

/// Starts DSH without an approval question for its worktree.
pub(super) fn in_worktree(command: &str) -> String {
    format!("export {TRUST} && {command}")
}
pub(super) const LIMITS: &str = ".dsh/cordis.patch.yml";
pub(super) const REFUSED: u8 = 2;
pub(super) const KEPT: Kept = Kept {
    links: &[
        Link { home: ".dsh/.credentials.yaml", kept: ".credentials.yaml" },
        // Only dependency resolution is shared; neither the user's profile
        // manifest nor any patch is loaded by the fleet's profile.
        Link {
            home: ".dsh/profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui",
            kept: "profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui",
        },
    ],
    host_user: &[
        Link { home: ".dsh/skills", kept: "host-skills" },
        Link { home: ".dsh/AGENTS.md", kept: "AGENTS.md" },
        Link { home: ".agents/skills", kept: "host-agents-skills" },
        Link { home: ".agents/AGENTS.md", kept: "agents/AGENTS.md" },
    ],
    files: &[
        ("profiles/dsh-tui/package.json", include_str!("dsh/profile.json")),
        ("profiles/dsh-tui/cordis.yml", "[]\n"),
        ("afkconvoy.patch.yml", include_str!("dsh/overlay.yml")),
        ("fleet.patch.yml", include_str!("dsh/fleet.yml")),
        ("afkconvoy.mjs", include_str!("dsh/plugin.mjs")),
        ("start.sh", include_str!("dsh/start.sh")),
        (PROBE_PATCH, include_str!("dsh/probe.yml")),
        ("probe.mjs", include_str!("dsh/probe.mjs")),
        (MODELS_PATCH, include_str!("dsh/models.yml")),
        ("models.mjs", include_str!("dsh/models.mjs")),
    ],
};

/// The overlay a probe boots DSH's headless profile with, kept: it loads the
/// plugin that sets the model the probe names.
const PROBE_PATCH: &str = "probe.patch.yml";

/// The line that sends `model` of DSH's the one-line `prompt` and prints its
/// answer: through `dsh`, the launcher its TUI comes with, in its headless
/// profile, with the overlay kept in `kept` that sets the model as a worker's
/// plugin does — with no model, the one the profile starts on. The worker
/// launch command starts the TUI, which answers no prompt but at a terminal.
pub(super) fn prompt_question(kept: &str, model: Option<&str>, prompt: &str) -> String {
    let line = match model {
        Some(model) => Line::new().fixed("AFKCONVOY_DSH_MODEL=").word(model).fixed(" "),
        None => Line::new(),
    };
    line.fixed("dsh headless --patch ")
        .word(&format!("{kept}/{PROBE_PATCH}"))
        .fixed(" ")
        .word(prompt)
        .own()
}

/// Starts one watch interactively on the worker profile.
pub(super) fn watch(command: &str, worktree: &str, kept: &str, model: Option<&str>) -> String {
    let mut line = Line::new()
        .fixed("export DSH_HOME=")
        .word(kept)
        .fixed(" DSH_AGENTS_HOME=")
        .word(&format!("{kept}/agents"));
    if let Some(model) = model {
        line = line.fixed(" AFKCONVOY_DSH_MODEL=").word(model);
    }
    let line = line
        .fixed(" AFKCONVOY_DSH_KEPT=")
        .word(kept)
        .fixed(" AFKCONVOY_HARNESS=dsh AFKCONVOY_GUARD=\"${AFKCONVOY_GUARD:-true}\" AFKCONVOY_WORKTREE=")
        .word(worktree)
        .fixed(" AFKCONVOY_TEAM_DIR=")
        .word(worktree)
        .fixed(" && ")
        .launch(&in_worktree(command))
        .fixed(" --patch ")
        .word(&format!("{kept}/fleet.patch.yml"))
        .fixed(" --patch ")
        .word(&format!("{kept}/afkconvoy.patch.yml"))
        .fixed(" ")
        .word(worktree);
    line.done()
}

/// Starts on the fleet's profile, whose plugin asks the supplied guard.
pub(super) fn guarded(command: &str, guard: &str) -> String {
    Line::new().fixed("export AFKCONVOY_GUARD=").word(guard).fixed(" && ").launch(command).done()
}

/// What the probe's plugin prints before the model each request is sent
/// to, on a line of its own (`dsh/probe.mjs`): DSH's headless answer says no
/// other.
const ANSWERED: &str = "AFKCONVOY_DSH_ANSWERED";

/// The model that answered `prompt_question`, by `answer`: the last request's.
pub(super) fn answered_as(answer: &str) -> Option<String> {
    let mut answered = None;
    for line in answer.lines() {
        let mut words = line.split_whitespace();
        if let (Some(ANSWERED), Some(model), None) = (words.next(), words.next(), words.next()) {
            answered = Some(model.to_string());
        }
    }
    answered
}

/// The overlay DSH's headless profile is booted with to say its models,
/// kept: it loads the plugin that prints them (`dsh/models.mjs`).
const MODELS_PATCH: &str = "models.patch.yml";

/// What that plugin prints before its JSON list or refusal, on one line.
const LISTED: &str = "AFKCONVOY_DSH_LISTED";

/// The line that asks DSH which models it runs: through `dsh`, the launcher
/// its TUI comes with, in its headless profile, with the overlay kept in
/// `kept` whose plugin asks DeepSeek's server with DSH's own credentials
/// and stops before the task is sent. The worker launch command starts the TUI, which says nothing but
/// at a terminal.
pub(super) fn models_question(kept: &str) -> String {
    Line::new()
        .fixed("dsh headless --patch ")
        .word(&format!("{kept}/{MODELS_PATCH}"))
        .fixed(" 'say your models'")
        .own()
}

/// The server's model list or refusal, carried on one marked JSON line.
#[derive(Deserialize)]
struct Listing {
    #[serde(default)]
    models: Vec<Listed>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct Listed {
    id: String,
    name: Option<String>,
}

fn listing(answer: &str) -> Option<Listing> {
    answer.lines().find_map(|line| {
        let json = line.strip_prefix(LISTED)?.strip_prefix(' ')?;
        serde_json::from_str(json).ok()
    })
}

/// The server's ids, once each in the order listed; no bundled fallback.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let answer = listing(answer)?;
    if answer.error.is_some() {
        return None;
    }
    let mut listed = vec![];
    for model in answer.models {
        if !model.id.trim().is_empty() && !listed.contains(&model.id) {
            listed.push(model.id);
        }
    }
    if listed.is_empty() { None } else { Some(listed) }
}

pub(super) fn shown(answer: &str) -> Vec<(String, String)> {
    listing(answer).map_or_else(Vec::new, |answer| {
        answer
            .models
            .into_iter()
            .filter_map(|model| model.name.map(|name| (model.id, name)))
            .collect()
    })
}

pub(super) fn listing_failure(answer: &str) -> Option<String> {
    listing(answer)?.error
}

#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    let line = match start.dsh_profile {
        crate::start::DshProfile::HostUser => line
            .fixed(" DSH_HOME=\"${DSH_HOME:-$HOME/.dsh}\"")
            .fixed(" DSH_AGENTS_HOME=\"${DSH_AGENTS_HOME:-$HOME/.agents}\""),
        crate::start::DshProfile::Fleet => line
            .fixed(" DSH_HOME=")
            .word(start.kept)
            .fixed(" DSH_AGENTS_HOME=")
            .word(&format!("{}/agents", start.kept))
            .fixed(" DSH_PERMISSION_MODE=danger-full-access"),
    };
    let line = line
        .fixed(" AFKCONVOY_DSH_KEPT=")
        .word(start.kept)
        .fixed(" DSH_TUI_PROFILE=dsh-tui")
        .fixed(" AFKCONVOY_GUARD=")
        .word(start.hook)
        .fixed(" AFKCONVOY_WORKTREE=")
        .word(start.worktree)
        .fixed(" AFKCONVOY_TEAM_DIR=")
        .word(start.added);
    match start.model {
        Some(model) => line.fixed(" AFKCONVOY_DSH_MODEL=").word(model),
        None => line,
    }
}

#[spec(fn(Line[true], &Start) -> Line[true])]
pub(super) fn added(line: Line, start: &Start) -> Line {
    let launch = line.done();
    Line::new()
        .launch("sh ")
        .word(&format!("{}/start.sh", start.kept))
        .fixed(" ")
        .word(&launch)
        .fixed(" ")
        .word(start.kept)
        .fixed(" ")
        .word(start.dir)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .word(start.dsh_profile.key())
}

#[spec(fn(Line[@l]) -> Line[l])]
pub(super) fn asked_its_flags(line: Line) -> Line {
    line.fixed(" --help")
}

#[derive(Deserialize)]
struct Described {
    name: String,
    #[serde(default)]
    arguments: Arguments,
}
#[derive(Default, Deserialize)]
struct Arguments {
    command: Option<String>,
}

pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.arguments.command {
        Some(line) if described.name == "bash" || described.name == "bash_persistent" => {
            Call::Shell(line)
        }
        _ => Call::Other,
    })
}

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".dsh";

const SESSION_FILE: &str = "session.v4.jsonl.zstd";

/// Where DeepSeek Harness keeps the session of `cwd`: one compressed file
/// in each session directory under the worktree's directory.
pub(super) fn session_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Named {
        dir: format!("sessions/{}", session::wrapped_dir(cwd)),
        name: SESSION_FILE,
        compressed: true,
        usage: None,
    }
}

/// The turn `text` records. The worktree is the directory the file was in.
/// DeepSeek Harness records no child session.
pub(super) fn recorded_turn(_cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
    session::turn_of(&notes, not_before)
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "turn/start" => Some(Note { marker: Marker::Start, at, child: false }),
        "turn/end" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

/// One session file a stand-in writes for this turn. The body is the text;
/// it is stored compressed.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let ms = at.saturating_mul(1000);
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("type", "turn/start".into()),
            ("time", ms.into()),
            ("data", session::object(vec![("turn", 1_u64.into())])),
        ])
    );
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("type", "turn/end".into()),
                ("time", ms.into()),
                (
                    "data",
                    session::object(vec![
                        ("turn", 1_u64.into()),
                        ("reason", session::object(vec![("kind", "completed".into())])),
                    ]),
                ),
            ])
        ));
    }
    vec![SessionFile {
        path: format!("sessions/{}/session-afkconvoy/{SESSION_FILE}", session::wrapped_dir(cwd)),
        body,
        compressed: true,
        replaces: None,
    }]
}

/// What one DeepSeek Harness session line says of a tool call: a
/// `tool/call` event by its `callId`, and the `tool/result` whose message
/// names it, `isError` where DeepSeek Harness marks it an error. Its shell
/// does not mark a command that exited other than 0, but ends what came back
/// with "[Command finished with exit code N]", which is an error too. A
/// session whose header says a sub-agent started it is a child's.
pub(super) fn said(value: &serde_json::Value) -> Vec<Said> {
    let data = &value["data"];
    let said = match value["type"].as_str() {
        Some("session") => {
            let delegated = value["delegationDepth"].as_u64().is_some_and(|depth| depth > 0);
            if value["origin"] == "subagent" || delegated {
                Said::Child
            } else {
                return Vec::new();
            }
        }
        Some("tool/call") => {
            let Some(id) = data["callId"].as_str() else { return Vec::new() };
            Said::Call {
                id: id.to_string(),
                action: steps::action(data["name"].as_str().unwrap_or(""), &data["arguments"]),
            }
        }
        Some("tool/result") => {
            let message = &data["message"];
            let Some(id) = message["toolCallId"].as_str() else { return Vec::new() };
            let result = steps::content(&message["content"]);
            let code = steps::exit_code(&result, "[Command finished with exit code ");
            Said::Result {
                id: id.to_string(),
                error: message["isError"] == true || code.is_some_and(|code| code != 0),
                result,
                at: value["time"].as_u64().map(session::epoch_number),
            }
        }
        _ => return Vec::new(),
    };
    vec![said]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;

    #[test]
    fn dsh_asks_the_server_through_its_credential_service() {
        assert_eq!(
            Harness::Dsh.models_question("dsh-tui --theme dark", "/it's kept"),
            "dsh headless --patch '/it'\\''s kept/models.patch.yml' 'say your models'"
        );
        assert!(KEPT.files.iter().any(|(name, _)| *name == MODELS_PATCH));
        let printed = r#"DSH startup
AFKCONVOY_DSH_LISTED {"models":[{"id":"deepseek-flash","name":"DeepSeek V4.1 Flash"},{"id":"deepseek-v4-pro"},{"id":"deepseek-flash"}]}
"#;
        assert_eq!(Harness::Dsh.models(printed).unwrap(), ["deepseek-flash", "deepseek-v4-pro"]);
        assert_eq!(
            Harness::Dsh.shown(printed),
            [("deepseek-flash".into(), "DeepSeek V4.1 Flash".into())]
        );
        for nothing in ["", "AFKCONVOY_DSH_LISTED {}", "AFKCONVOY_DSH_LISTED invalid"] {
            assert_eq!(Harness::Dsh.models(nothing), None);
        }
        let refused = r#"AFKCONVOY_DSH_LISTED {"error":"Authentication Fails"}"#;
        assert_eq!(Harness::Dsh.models(refused), None);
        assert_eq!(Harness::Dsh.listing_failure(refused).as_deref(), Some("Authentication Fails"));
    }

    #[test]
    fn dsh_installs_from_npm_needs_dsh_and_reaches_deepseek_without_a_proxy() {
        let dsh = Harness::Dsh;
        assert_eq!(dsh.install(), "npm install -g @deepseek-harness-tui/dsh-tui");
        assert_eq!(dsh.login(), None, "it is asked about no login");
        assert!(dsh.logged_in(true, ""));
        assert!(!dsh.logged_in(false, ""));
        assert_eq!(dsh.needs(), Some(crate::Needed::Dsh));
        assert_eq!(dsh.no_proxy(), Some("api.deepseek.com"));
        assert_eq!(
            dsh.kept_package(),
            Some("profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui")
        );
        assert!(!dsh.own_home_outside_role());
    }
}

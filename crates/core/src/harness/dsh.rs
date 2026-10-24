//! DeepSeek Harness: an interactive TUI on the profile its role chooses. The only
//! added plugin asks the guard and reports running/idle to the worker backend.
use flux_rs::attrs::*;
use serde::{Deserialize, Deserializer};

use super::{Kept, Link};
use crate::guard::Call;
use crate::line::Line;
use crate::role::Start;

pub(super) const KEY: &str = "dsh";
pub(super) const NAME: &str = "DeepSeek Harness";
pub(super) const PROGRAM: &str = "dsh-tui";
pub(super) const STOCK: &str = "dsh-tui";
pub(super) const FLAGS: [&str; 1] = ["--patch"];
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

/// What that plugin prints before each model's name, on a line of its own.
const LISTED: &str = "AFKCONVOY_DSH_LISTED";

/// The line that asks DSH which models it runs: through `dsh`, the launcher
/// its TUI comes with, in its headless profile, with the overlay kept in
/// `kept` whose plugin prints the models of each provider route DSH holds,
/// as DSH lists them with its own credentials, and stops before the task is
/// sent. The worker launch command starts the TUI, which says nothing but
/// at a terminal.
pub(super) fn models_question(kept: &str) -> String {
    Line::new()
        .fixed("dsh headless --patch ")
        .word(&format!("{kept}/{MODELS_PATCH}"))
        .fixed(" 'say your models'")
        .own()
}

/// The models DSH lists in `answer`, what it printed for `models_question`,
/// each by the name a stage gives it, once, in the order listed. `None`
/// when it lists none: it holds no credential, or said something else.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let mut listed: Vec<String> = vec![];
    for line in answer.lines() {
        let mut words = line.split_whitespace();
        if let (Some(LISTED), Some(model), None) = (words.next(), words.next(), words.next())
            && !listed.iter().any(|seen| seen == model)
        {
            listed.push(model.to_string());
        }
    }
    if listed.is_empty() { None } else { Some(listed) }
}

#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    let line = match start.dsh_profile {
        crate::team::DshProfile::HostUser => line
            .fixed(" DSH_HOME=\"${DSH_HOME:-$HOME/.dsh}\"")
            .fixed(" DSH_AGENTS_HOME=\"${DSH_AGENTS_HOME:-$HOME/.agents}\""),
        crate::team::DshProfile::Fleet => line
            .fixed(" DSH_HOME=")
            .word(start.kept)
            .fixed(" DSH_AGENTS_HOME=")
            .word(&format!("{}/agents", start.kept))
            .fixed(" DSH_PERMISSION_MODE=danger-full-access"),
    };
    line.fixed(" AFKCONVOY_DSH_KEPT=")
        .word(start.kept)
        .fixed(" DSH_TUI_PROFILE=dsh-tui")
        .fixed(" AFKCONVOY_GUARD=")
        .word(start.guard)
        .fixed(" AFKCONVOY_WORKTREE=")
        .word(start.worktree)
        .fixed(" AFKCONVOY_TEAM_DIR=")
        .word(start.team_dir)
        .fixed(" AFKCONVOY_DSH_MODEL=")
        .word(start.model.unwrap_or(""))
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
        .word(start.role_dir)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;

    #[test]
    fn dsh_is_asked_its_models_through_its_headless_profile_with_the_kept_overlay() {
        assert_eq!(
            Harness::Dsh.models_question("dsh-tui --theme dark", "/it's kept"),
            "dsh headless --patch '/it'\\''s kept/models.patch.yml' 'say your models'"
        );
        // The overlay loads the plugin kept beside it, which prints what is read.
        let files: Vec<&str> = KEPT.files.iter().map(|(name, _)| *name).collect();
        assert!(files.contains(&MODELS_PATCH) && files.contains(&"models.mjs"), "{files:?}");
        assert!(include_str!("dsh/models.yml").contains("@AFKCONVOY_DSH_MODELS@"));
        assert!(include_str!("dsh/models.mjs").contains(&format!("`{LISTED} ${{id}}\\n`")));
    }

    #[test]
    fn its_models_are_those_it_prints_once_each_in_order() {
        let printed = "dsh: all_proxy names a SOCKS proxy, which is not supported\n\
            AFKCONVOY_DSH_LISTED deepseek-flash\n\
            AFKCONVOY_DSH_LISTED deepseek-v4-pro\r\n\
            AFKCONVOY_DSH_LISTED deepseek-flash\n\
            said AFKCONVOY_DSH_LISTED no-model\n\
            AFKCONVOY_DSH_LISTED two words\n";
        assert_eq!(Harness::Dsh.models(printed).unwrap(), ["deepseek-flash", "deepseek-v4-pro"]);
        // No credential: it prints no model, and says why on its own.
        for nothing in ["", "dsh: MISSING_CREDENTIAL: llm-deepseek: no API key\n"] {
            assert_eq!(Harness::Dsh.models(nothing), None, "{nothing}");
        }
        let listed = Harness::Dsh.models(printed).unwrap();
        assert!(Harness::Dsh.knows(&listed, "deepseek-v4-pro"));
        assert!(!Harness::Dsh.knows(&listed, "deepseek"));
    }
}

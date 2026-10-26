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
pub(super) const TRUST: &str = "DSH_PERMISSION_MODE=danger-full-access";
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
pub(super) fn watch(command: &str, kept: &str, model: Option<&str>) -> String {
    let line = Line::new()
        .fixed("export DSH_HOME=")
        .word(kept)
        .fixed(" AFKCONVOY_DSH_MODEL=")
        .word(model.unwrap_or_default())
        .fixed(" && ")
        .launch(&in_worktree(command));
    line.done()
}

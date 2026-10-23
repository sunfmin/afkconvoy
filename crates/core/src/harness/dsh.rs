//! DeepSeek Harness: an interactive TUI on a fleet-owned profile. The only
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
        Link { home: ".dsh/profiles/dsh-tui/node_modules", kept: "profiles/dsh-tui/node_modules" },
    ],
    host_user: &[
        Link { home: ".dsh/skills", kept: "skills" },
        Link { home: ".dsh/AGENTS.md", kept: "AGENTS.md" },
    ],
    files: &[
        ("profiles/dsh-tui/package.json", include_str!("dsh/profile.json")),
        ("profiles/dsh-tui/cordis.yml", "[]\n"),
        ("afkconvoy.patch.yml", include_str!("dsh/overlay.yml")),
        ("afkconvoy.mjs", include_str!("dsh/plugin.mjs")),
        ("start.mjs", include_str!("dsh/start.mjs")),
    ],
};

#[spec(fn(Line[@l], &Start) -> Line[l])]
pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" DSH_HOME=")
        .word(start.kept)
        .fixed(" DSH_TUI_PROFILE=dsh-tui DSH_PERMISSION_MODE=danger-full-access")
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
    // The supervisor withholds terminal input until the plugin has activated.
    // Its shell command is one quoted argument, preserving host wrappers.
    let command = line.done();
    Line::new()
        .launch("node ")
        .word(&format!("{}/start.mjs", start.kept))
        .fixed(" ")
        .word(&command)
        .fixed(" ")
        .word(start.kept)
        .fixed(" ")
        .word(start.role_dir)
        .fixed(" ")
        .word(start.worktree)
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

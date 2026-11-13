//! The tmux of a Guild machine (sunfmin/control#753): Guild runs each of its
//! instances in a tmux server of its own, so every Guild machine needs one,
//! and the host kit installs it (docs/adr/0066, docs/adr/0062). What is a Guild
//! machine, where its tmux is and how one is installed is the host kit
//! crate's (`afkconvoy_host_kit::tmux`, docs/adr/0062): writing the host kit
//! installs it, and `doctor` names a Guild machine that has none, with the
//! line that installs it.

use afkconvoy_host_kit::tmux::{self, Tmux};
use serde_json::{Value, json};

use crate::agent_setup::{WorkerShell, home};

/// The line that installs tmux again: writing the host kit, which installs
/// pixi first where no line on this host can install tmux.
pub const FIX: &str = "afkconvoy kit";

/// Installs tmux on this host where it is a Guild machine with none → what
/// was done: null where nothing was to be done — not a Guild machine, or its
/// tmux already there and left alone — else `{installed, by}` or
/// `{error, fix}`. What failed fails no write of the host kit: `doctor`
/// names the Guild machine still without tmux.
pub fn give() -> Value {
    let Ok(home) = home() else { return Value::Null };
    match tmux::give(&WorkerShell, &home) {
        None => Value::Null,
        Some(Tmux::Installed { installed, by }) => {
            eprintln!("installed tmux on this Guild machine: {by}");
            json!({"installed": installed, "by": by})
        }
        Some(Tmux::Failed { error }) => {
            eprintln!("tmux was not installed on this Guild machine: {error}");
            json!({"error": error, "fix": FIX})
        }
    }
}

/// A Guild machine with no tmux, as a finding.
pub fn findings() -> Vec<crate::doctor::Finding> {
    let Ok(home) = home() else { return vec![] };
    let Some(evidence) = tmux::missing(&WorkerShell, &home) else { return vec![] };
    vec![crate::doctor::finding(
        crate::doctor::Kind::TmuxMissing,
        json!(["tmux-missing"]),
        "this Guild machine has no tmux".to_string(),
        evidence,
        FIX.to_string(),
    )]
}

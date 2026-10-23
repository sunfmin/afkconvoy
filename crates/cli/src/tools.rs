//! Finds the commands a fleet cannot work without (`afkconvoy_core::tools`):
//! `git`, `gh` and `orca` on this process's `PATH`, which runs them, a
//! harness and every login in a login shell, which is where a worker runs; and of an orca
//! that is there, whether it is running and knows this checkout. It only
//! looks: nothing is installed, started or added and nobody is logged in.

use std::os::unix::fs::PermissionsExt;

use afkconvoy_core::team::BY_DEFAULT;
use afkconvoy_core::tools::{ALWAYS, Found, Tool, refusal, remote_host};
use anyhow::{Result, bail};

use crate::git::{REMOTE, Repo};
use crate::launch::{Launches, Proposed, ask_login_shell};
use crate::orca;

/// What this host has of the commands a start needs.
pub struct Looked {
    /// Each tool needed, and what was found of it.
    pub found: Vec<(Tool, Found)>,
    /// The host `gh` was asked about: the one the repo's remote is on.
    pub host: Option<String>,
    /// Why an orca that is running could not say whether it knows this
    /// checkout: it is in `found` as not running, which is all a start can
    /// make of it.
    pub orca_unasked: Option<String>,
}

/// Looks for every command a start needs, asks each whether it is logged in,
/// and orca whether it is running and knows this checkout. `launch` is the
/// worker launch command, when the start knows it.
pub fn look(launch: Option<&Proposed>) -> Looked {
    // No git, no checkout, a remote that is a path: `gh` is asked about no host.
    let remote = Repo::here().and_then(|repo| repo.try_git(&["remote", "get-url", REMOTE]));
    let remote = remote.ok().filter(|ran| ran.ok).map(|ran| ran.stdout);
    let host = remote.as_deref().and_then(remote_host);

    let mut found = vec![];
    let mut orca_unasked = None;
    for tool in ALWAYS {
        let state = match tool.login_question(host) {
            _ if !on_path(tool.name()) => Found::Missing,
            None if tool == Tool::Orca => orca::found().unwrap_or_else(|error| {
                orca_unasked = Some(format!("{error:#}"));
                Found::NotRunning
            }),
            None => Found::Ready,
            Some(question) => asked(tool, &question),
        };
        found.push((tool, state));
    }
    found.extend(launch.and_then(harness));
    Looked { found, host: host.map(str::to_string), orca_unasked }
}

/// What this host has of the harness `launch` starts, when it runs that
/// harness by its own name (`Tool::of`): there where a worker is started,
/// and logged in there. `None`: a wrapper, which is asked nothing.
pub fn harness(launch: &Proposed) -> Option<(Tool, Found)> {
    let tool = Tool::of(launch.harness(), launch.as_str())?;
    let state = match tool.login_question(None) {
        _ if !launch.resolves() => Found::Missing,
        None => Found::Ready,
        Some(question) => asked(tool, &question),
    };
    if tool == Tool::DshTui && state == Found::Ready {
        let (ok, _) = ask_login_shell("command -v dsh").unwrap_or_default();
        if !ok {
            return Some((Tool::Dsh, Found::Missing));
        }
    }
    Some((tool, state))
}

/// What `tool` is found to be by its answer to `question`, in a login shell.
fn asked(tool: Tool, question: &str) -> Found {
    let (succeeded, printed) = ask_login_shell(question).unwrap_or_default();
    if tool.logged_in(succeeded, &printed) { Found::Ready } else { Found::LoggedOut }
}

/// Checks that this host has every command a start needs, each logged in, and
/// that orca is running and knows this checkout.
/// Of `launches`, the default harness's worker launch command is checked
/// here, and its harness looked for, when the start knows it: every other
/// harness's is `role::probe`'s, which reads the team that calls for them. An error names every fault at
/// once, each with the line a human runs to fix it.
pub fn check(launches: &Launches) -> Result<()> {
    let default = launches.default();
    let launch = launches.of(default)?;
    let looked = look(launch);
    if let Some(why) = looked.orca_unasked {
        bail!(why);
    }
    let fault = launch.and_then(Proposed::fault);
    let fault = fault.map(|fault| launches.said(&fault, default, &[BY_DEFAULT.to_string()]));
    match refusal(&looked.found, looked.host.as_deref(), fault.as_deref()) {
        Some(why) => bail!(why),
        None => Ok(()),
    }
}

/// Whether a directory of this process's `PATH` holds a program by this name.
fn on_path(name: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| {
        std::fs::metadata(dir.join(name))
            .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
    })
}

//! Finds the commands a fleet cannot work without (`afkconvoy_core::tools`):
//! `git`, `gh` and `orca` on this process's `PATH`, which runs them, and
//! every login in a login shell, which is where a worker runs; and of an orca
//! that is there, whether it is running and knows this checkout. It only
//! looks: nothing is installed, started or added and nobody is logged in.

use std::os::unix::fs::PermissionsExt;

use afkconvoy_core::tools::{Found, Tool, needed, refusal, remote_host};
use anyhow::{Result, bail};

use crate::git::{REMOTE, Repo};
use crate::launch::{Proposed, ask_login_shell};
use crate::orca;

/// Checks that this host has every command a start needs, each logged in, and
/// that orca is running and knows this checkout.
/// `launch` is the worker launch command, when the start knows it. An error
/// names every fault at once, each with the line a human runs to fix it.
pub fn check(launch: Option<&Proposed>) -> Result<()> {
    // No git, no checkout, a remote that is a path: `gh` is asked about no host.
    let remote = Repo::here().and_then(|repo| repo.try_git(&["remote", "get-url", REMOTE]));
    let remote = remote.ok().filter(|ran| ran.ok).map(|ran| ran.stdout);
    let host = remote.as_deref().and_then(remote_host);

    let mut found = vec![];
    for tool in needed(launch.map(Proposed::as_str)) {
        let present = match tool {
            // What starts a worker is looked up where a worker is started.
            Tool::Claude => launch.is_some_and(Proposed::resolves),
            Tool::Git | Tool::Gh | Tool::Orca => on_path(tool.name()),
        };
        let state = match tool.login_question(host) {
            _ if !present => Found::Missing,
            None if tool == Tool::Orca => orca::found()?,
            None => Found::Ready,
            Some(question) => {
                let (succeeded, printed) = ask_login_shell(&question).unwrap_or_default();
                if tool.logged_in(succeeded, &printed) { Found::Ready } else { Found::LoggedOut }
            }
        };
        found.push((tool, state));
    }
    match refusal(&found, host, launch.and_then(Proposed::fault).as_deref()) {
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

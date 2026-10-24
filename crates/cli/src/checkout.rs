//! The checkout a command is about: the one named after it, or with `-C`, or
//! else the one it is run in. It is found once, where the command starts, and
//! handed to everything the command does: nothing else reads the repo from the
//! process, so one process may hold the fleets of many checkouts
//! (docs/adr/0023).

use std::path::{Path, PathBuf};

use afkconvoy_core::tools::{Found, Tool, fault};
use anyhow::{Context, Result, bail};

use crate::git::Repo;
use crate::orca::Worktree;
use crate::topic::See;

/// The checkout `named` names — a checkout of the repo, or any directory in
/// one, relative to where the command is run — or, with none named, the one
/// it is run in. A worker's worktree is refused unless the command is taken
/// there (`taken`): one a worker runs there, or one that only reads. The
/// fleet cut it for that one worker, and any other command run in it would
/// act for the repo from a worker's branch. On a host with no git, which
/// finds a checkout, the refusal is git's own fault, with the line that
/// installs it: a start names every other command it lacks beside it
/// (`started`).
pub fn resolve(named: Option<&Path>, taken: bool) -> Result<Repo> {
    let dir = dir(named)?;
    if !crate::tools::on_path(Tool::Git.name()) {
        let missing = fault(Tool::Git, &Found::Missing, None, &[]).unwrap_or_default();
        return Err(See::Troubleshooting.refusal(missing.trim()));
    }
    let repo = Repo::at(&dir)?;
    if !taken && Worktree::of(&repo)?.holds_a_worker() {
        let own = repo.main_checkout().map(|own| format!(", such as {}", own.display()));
        bail!(
            "{} is a worker's worktree, where the fleet put one of its workers: only a worker's \
             own commands, and those that only read, are run there. Name another checkout of the \
             repo{}",
            repo.dir.display(),
            own.unwrap_or_default()
        );
    }
    Ok(repo)
}

/// `resolve`, for a start — `run`, `tick` or `plan`. On a host with no git
/// its refusal is a start's first check (`probe::without_git`): git, and
/// every other command the fleet needs that this host lacks, each with the
/// line that installs it, in one refusal.
pub fn started(named: Option<&Path>, taken: bool) -> Result<Repo> {
    match crate::probe::without_git(&dir(named)?) {
        Some(refused) => Err(refused),
        None => resolve(named, taken),
    }
}

/// The directory `named` names, relative to where the command is run, or
/// with none named that one.
pub fn dir(named: Option<&Path>) -> Result<PathBuf> {
    let here = std::env::current_dir().context("this process has no working directory")?;
    Ok(named.map_or_else(|| here.clone(), |named| here.join(named)))
}

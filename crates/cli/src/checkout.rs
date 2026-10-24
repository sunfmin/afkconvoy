//! The checkout a command is about: the one named after it, or with `-C`, or
//! else the one it is run in. It is found once, where the command starts, and
//! handed to everything the command does: nothing else reads the repo from the
//! process, so one process may hold the fleets of many checkouts
//! (docs/adr/0023).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::git::Repo;
use crate::orca::Worktree;

/// The checkout `named` names — a checkout of the repo, or any directory in
/// one, relative to where the command is run — or, with none named, the one
/// it is run in. A worker's worktree is refused unless the command is one a
/// worker runs there (`worker`): the fleet cut it for that one worker, and
/// any other command run in it would act for the repo from a worker's
/// branch. On a host with no git, which finds a checkout, the refusal names
/// git and every other command a fleet needs that is missing
/// (`probe::without_git`).
pub fn resolve(named: Option<&Path>, worker: bool) -> Result<Repo> {
    let dir = dir(named)?;
    if let Some(refused) = crate::probe::without_git(&dir) {
        return Err(refused);
    }
    let repo = Repo::at(&dir)?;
    if !worker && Worktree::of(&repo)?.holds_a_worker() {
        let own = repo.main_checkout().map(|own| format!(", such as {}", own.display()));
        bail!(
            "{} is a worker's worktree, where the fleet put one of its workers: only a worker's \
             own commands are run there. Name another checkout of the repo{}",
            repo.dir.display(),
            own.unwrap_or_default()
        );
    }
    Ok(repo)
}

/// The directory `named` names, relative to where the command is run, or
/// with none named that one.
pub fn dir(named: Option<&Path>) -> Result<PathBuf> {
    let here = std::env::current_dir().context("this process has no working directory")?;
    Ok(named.map_or_else(|| here.clone(), |named| here.join(named)))
}

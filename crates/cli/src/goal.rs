//! A worker's goal, where its harness keeps one (sunfmin/control#544): its
//! brief is handed as that goal, and the harness carries the worker on after
//! each turn its model ends short of it. What the store is, its query and
//! what ends a goal are the harness's (`Harness::goal_store`); here they are
//! only read and typed.

use std::path::Path;
use std::process::Command;

use afkconvoy_core::harness::{Goal, Harness};
use anyhow::{Context, Result, bail};

use crate::orca::Worktree;

/// The program that answers a goal store's query.
const SQLITE: &str = "sqlite3";

/// The goal of the worker in `worktree` and the harness it runs on. `None`:
/// its harness keeps no goal, or its store cannot be read — which is not a
/// goal that stopped.
pub(crate) fn of(worktree: &Worktree) -> Option<(Harness, Goal)> {
    let harness = worktree.recorded_harness().ok().flatten()?;
    let goal = read(&worktree.path, harness)?;
    Some((harness, goal))
}

/// The goal `harness` keeps for the worker in `cwd`.
fn read(cwd: &Path, harness: Harness) -> Option<Goal> {
    let store = harness.goal_store()?;
    let home = crate::session::kept_home(cwd, harness)?;
    let file = home.join(store.file);
    if !file.exists() {
        return Some(Goal::Unset);
    }
    let answered = Command::new(SQLITE).arg("-readonly").arg(&file).arg(store.query).output();
    let answered = answered.ok().filter(|answered| answered.status.success())?;
    harness.goal(&String::from_utf8_lossy(&answered.stdout))
}

/// Whether the worker in `worktree`, running on `harness`, has been
/// continued on its goal by its harness since `since`.
pub(crate) fn continued(worktree: &Worktree, harness: Harness, since: u64) -> Result<bool> {
    crate::session::goal_continued(&worktree.path, harness, since)
}

/// Ends the goal of the worker in `worktree`: its current turn finishes, and
/// its harness begins no other on it.
pub(crate) fn end(worktree: &Worktree, harness: Harness) -> Result<()> {
    let key = harness.goal_end().context("its harness keeps no goal")?;
    let Some(terminal) = worktree.terminal()? else {
        bail!("no live terminal in {} to end its goal", worktree.path.display());
    };
    worktree.press_key(&terminal, key)
}

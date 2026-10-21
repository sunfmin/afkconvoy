//! What a host inspection frees, and what it names for a human, as its
//! `afkconvoy inspected` is told (docs/adr/0019): each path is checked first
//! — where it is under the home folder, whether it is a git checkout, whether
//! a process uses it or a worker holds it, whether its issue is closed and
//! whether git says changes no commit holds — and freed only where the core
//! says how (`afkconvoy_core::host_inspection::reclaim`). Of a line it refuses
//! any path of, nothing is freed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use afkconvoy_core::host_inspection::{Facts, Item, Place, Reclaim, place, reclaim};
use anyhow::{Context, Result, anyhow, bail};

use crate::gh::GitHub;
use crate::git::Repo;
use crate::host_evidence::du;
use crate::orca::{OnHost, Worktree};

/// How long one path is measured for, at the most.
const MEASURED_WITHIN: Duration = Duration::from_secs(120);

/// Frees each of `paths`, once every one of them is checked → what each
/// freed. An error, freeing nothing, where any of them may not be.
pub fn reclaim_all(paths: &[PathBuf]) -> Result<Vec<Item>> {
    if paths.is_empty() {
        return Ok(vec![]);
    }
    let home = home()?;
    let on_host = Worktree::on_host().context("cannot tell which worktree a worker holds")?;
    let open = open_paths()?;
    let mut checked = vec![];
    let mut refused = vec![];
    for path in paths {
        let path = path.canonicalize().with_context(|| format!("no {}", path.display()))?;
        let facts = facts(&path, &home, &on_host, &open)?;
        match reclaim(&facts) {
            Ok(how) => checked.push((path, how)),
            Err(why) => refused.push(format!("{}: {why}", path.display())),
        }
    }
    if !refused.is_empty() {
        bail!("nothing was freed, for these may not be: {}", refused.join("; "));
    }
    let mut freed = vec![];
    for (path, how) in checked {
        let before = size(&path);
        free(&path, how).with_context(|| format!("cannot free {}", path.display()))?;
        let after = if path.exists() { size(&path) } else { 0 };
        let path = path.display().to_string();
        freed.push(Item { path, bytes: before.saturating_sub(after) });
    }
    Ok(freed)
}

/// Each of `paths`, with what it holds, for a human to free.
pub fn name_all(paths: &[PathBuf]) -> Result<Vec<Item>> {
    paths
        .iter()
        .map(|path| {
            let path = path.canonicalize().with_context(|| format!("no {}", path.display()))?;
            Ok(Item { bytes: size(&path), path: path.display().to_string() })
        })
        .collect()
}

/// The home folder, as the file system has it.
fn home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    PathBuf::from(&home).canonicalize().with_context(|| format!("no home folder {home:?}"))
}

/// What is known of `path`, under `home`, of the worktrees `on_host` and the
/// files processes hold `open`.
fn facts(path: &Path, home: &Path, on_host: &[OnHost], open: &[PathBuf]) -> Result<Facts> {
    let place = path.strip_prefix(home).ok().map(|relative| match relative.to_str() {
        Some(relative) => place(relative),
        // A name that is no UTF-8 is nothing of the core's lists.
        None => Place::Elsewhere,
    });
    let repo = Repo::new(path.to_path_buf());
    let top = repo.try_git(&["rev-parse", "--show-toplevel"])?;
    let checkout = top.ok && Path::new(top.stdout.trim()).canonicalize().ok().as_deref() == Some(path);
    let worktree = on_host.iter().find(|on| on.path.canonicalize().ok().as_deref() == Some(path));
    let held = worktree.is_some_and(|on| on.held);
    let closed = match worktree.and_then(|on| Some((on.repo.clone()?, on.issue?))) {
        Some((repo, issue)) => GitHub { dir: repo }.issue(issue)?.is_closed(),
        None => false,
    };
    let uncommitted = checkout && !repo.git(&["status", "--porcelain"])?.trim().is_empty();
    let in_use = open.iter().any(|open| open.starts_with(path));
    Ok(Facts { place, checkout, in_use, held, closed, uncommitted })
}

/// Every file and working directory a process other than this one holds
/// open, as `lsof` says them.
fn open_paths() -> Result<Vec<PathBuf>> {
    let out = Command::new("lsof")
        .args(["-n", "-P", "-w", "-F", "pn"])
        .output()
        .context("cannot run lsof: which checkout a process uses cannot be told")?;
    let me = std::process::id().to_string();
    let mut mine = false;
    let mut open: HashSet<PathBuf> = HashSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(pid) = line.strip_prefix('p') {
            mine = pid == me;
        } else if let Some(name) = line.strip_prefix('n')
            && !mine
            && name.starts_with('/')
        {
            let path = PathBuf::from(name);
            open.insert(path.canonicalize().unwrap_or(path));
        }
    }
    Ok(open.into_iter().collect())
}

/// Frees `path` as `how` says.
fn free(path: &Path, how: Reclaim) -> Result<()> {
    match how {
        Reclaim::Ignored => {
            Repo::new(path.to_path_buf()).git(&["clean", "-fdX", "-q"])?;
        }
        Reclaim::Worktree => Worktree::at(path.to_path_buf(), String::new()).remove()?,
        Reclaim::Cache { whole: true } => {
            for entry in std::fs::read_dir(path)? {
                remove(&entry?.path())?;
            }
        }
        Reclaim::Cache { whole: false } => remove(path)?,
    }
    Ok(())
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// The bytes under `path`, as `du` says them; nothing where it cannot.
fn size(path: &Path) -> u64 {
    du(path, Instant::now() + MEASURED_WITHIN).unwrap_or(0)
}

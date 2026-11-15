//! A watch's checkout goes once nothing is left in it (sunfmin/control#1063),
//! as a settled worker's worktree does: at once when `land --watch` lands its
//! change, and otherwise at a tick, once no watch round runs, no process works
//! in it and it has not moved for `GRACE_SECONDS`. Either way only where
//! nothing would be lost with it: no change that no commit holds, untracked
//! files included, and its head on the remote.

use std::path::Path;

use afkconvoy_core::worker::{is_worktree_named, short_branch};
use anyhow::Result;
use serde_json::{Value, json};

use crate::git::Repo;
use crate::host_watch::CHECKOUT;
use crate::orca::{Source, Worktree};

/// How long a watch's checkout that has not moved stays, where no watch
/// round runs and no process works in it, before a tick removes it: one a
/// watch left without landing, everything in it pushed.
pub(crate) const GRACE_SECONDS: u64 = 60 * 60;

/// Whether the directory `dir` is named as a watch's checkout is.
pub(crate) fn named(dir: &Path) -> bool {
    dir.file_name().and_then(|name| name.to_str()).is_some_and(|n| is_worktree_named(n, CHECKOUT))
}

/// Why `checkout`, of the repo whose main checkout is `main`, stays → `None`
/// where nothing would be lost with it: nothing uncommitted, untracked files
/// included, and its head on the remote — on `base` or its own branch there,
/// or in `landed`, the commit its landing put on `base`.
fn keeps(
    main: &Repo,
    base: &str,
    checkout: &Worktree,
    landed: Option<&str>,
) -> Result<Option<String>> {
    let here = Repo::new(checkout.path.clone());
    let status = here.try_git(&["status", "--porcelain", "--untracked-files=normal"])?;
    if !status.ok {
        return Ok(Some(format!("git cannot read it: {}", status.stderr.trim())));
    }
    if !status.stdout.is_empty() {
        return Ok(Some("it holds changes no commit has".into()));
    }
    let head = here.try_git(&["rev-parse", "HEAD"])?;
    if !head.ok {
        return Ok(Some(format!("git cannot read its head: {}", head.stderr.trim())));
    }
    let head = head.stdout;
    let in_landed = landed.is_some_and(|commit| {
        main.try_git(&["merge-base", "--is-ancestor", &head, commit]).is_ok_and(|ran| ran.ok)
    });
    if !in_landed && !crate::orca::commit_is_on_remote(main, base, &checkout.branch, &head) {
        return Ok(Some("it holds a commit the remote does not".into()));
    }
    Ok(None)
}

/// Removes `checkout`: orca removes one it lists, and its branch with it
/// (`Worktree::remove`); git one orca does not, and its local branch where a
/// tag or a remote branch holds its tip (`Repo::delete_held_branch`).
fn remove(main: &Repo, checkout: &Worktree) -> Result<()> {
    if crate::orca::running() && crate::orca::lists(&checkout.path)? {
        return checkout.remove();
    }
    main.remove_worktree(&checkout.path)?;
    if !checkout.branch.is_empty() {
        let _ = main.delete_held_branch(&checkout.branch);
    }
    Ok(())
}

/// Removes the watch's checkout `checkout`, of `repo`, whose change landed
/// as `commit` on `base` → what the answer of `land --watch` says of it: the
/// checkout it removed, or why it stays. The watch's shell may still be in
/// it: the watch's brief says its checkout goes once its change lands.
pub(crate) fn landed(repo: &Repo, base: &str, checkout: &Worktree, commit: &str) -> Value {
    let removed = (|| -> Result<Option<String>> {
        let main = Repo::new(repo.main_checkout()?);
        if let Some(why) = keeps(&main, base, checkout, Some(commit))? {
            return Ok(Some(why));
        }
        remove(&main, checkout)?;
        Ok(None)
    })();
    match removed {
        Ok(None) => json!({"removed": checkout.path, "branch": checkout.branch}),
        Ok(Some(why)) => json!({"kept": checkout.path, "why": why}),
        Err(error) => json!({"kept": checkout.path, "why": format!("{error:#}")}),
    }
}

/// When the checkout at `path` last moved: its head's reflog, which a
/// checkout, commit or reset writes and a status does not, or its head's
/// commit → `None` where git says neither.
fn last_moved(path: &Path) -> Option<u64> {
    let here = Repo::new(path.to_path_buf());
    let args = ["rev-parse", "--path-format=absolute", "--git-path", "logs/HEAD"];
    let reflog = here.try_git(&args).ok().filter(|ran| ran.ok).and_then(|ran| {
        let modified = std::fs::metadata(ran.stdout).ok()?.modified().ok()?;
        Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
    });
    let committed = here
        .try_git(&["log", "-1", "--format=%ct", "HEAD"])
        .ok()
        .filter(|ran| ran.ok)
        .and_then(|ran| ran.stdout.parse::<u64>().ok());
    reflog.into_iter().chain(committed).max()
}

/// Removes each watch's checkout of `repo` that a watch left: one orca lists,
/// where no watch round runs on this host, no process works in it, it has
/// not moved for `GRACE_SECONDS` before `now`, and nothing would be lost with
/// it. One that holds something stays, for doctor to name.
pub(crate) fn remove_left(repo: &Repo, base: &str, now: u64) -> Result<()> {
    if !crate::orca::running() || crate::host_watch::running()?.is_some() {
        return Ok(());
    }
    let source = Source::of(repo)?;
    let main = Repo::new(repo.main_checkout()?);
    let mut busy = None;
    for row in crate::orca::linked_rows(&source)? {
        let Some(path) = row["path"].as_str().map(std::path::PathBuf::from) else { continue };
        if !named(&path) || !path.is_dir() {
            continue;
        }
        if last_moved(&path).is_none_or(|moved| now.saturating_sub(moved) < GRACE_SECONDS) {
            continue;
        }
        if busy.is_none() {
            busy = Some(crate::hot::working_directories()?);
        }
        if crate::hot::occupies(busy.as_deref().unwrap_or_default(), &path) {
            continue;
        }
        let branch = short_branch(row["branch"].as_str().unwrap_or("")).to_string();
        let checkout = Worktree::at(path, branch);
        if keeps(&main, base, &checkout, None)?.is_some() {
            continue;
        }
        if let Err(error) = remove(&main, &checkout) {
            eprintln!(
                "warning: the watch's checkout {} will be removed on a later tick: {error:#}",
                checkout.path.display()
            );
        }
    }
    Ok(())
}

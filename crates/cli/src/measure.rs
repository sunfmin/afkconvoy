//! How much the train's worktree holds: what git ignores in it — what gate
//! runs built — and the files git tracks.

use std::path::Path;

use afkconvoy_core::landing::Built;
use anyhow::{Context, Result};

use crate::git::Repo;
use crate::worker_tmp::TMP_DIR;

/// The note of the bytes the worktree held after its first gate run, kept in
/// the worktree's own git dir: gone when the worktree is.
const FRESH_NOTE: &str = "convoy-fresh-build";

/// The note of the bytes git ignored in the worktree after its last landing's
/// gate run: what a tick reads, for it measures nothing.
const BUILT_NOTE: &str = "convoy-built";

/// The paths git ignores in the worktree, an ignored directory as one path:
/// the worker's `TMPDIR` but for, which no gate run built.
fn ignored(repo: &Repo) -> Result<Vec<String>> {
    let listed = repo.git_raw(&[
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ])?;
    let built = |path: &&str| !path.is_empty() && path.split('/').next() != Some(TMP_DIR);
    Ok(listed.split('\0').filter(built).map(str::to_string).collect())
}

/// Whether the worktree holds anything git ignores.
pub fn holds_ignored(repo: &Repo) -> Result<bool> {
    Ok(!ignored(repo)?.is_empty())
}

/// The bytes of the files under `path`, a symlink counted as itself and never
/// followed. Something gone while it is counted counts nothing.
fn bytes_under(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.map_while(|entry| entry.ok()).map(|entry| bytes_under(&entry.path())).sum()
}

/// The bytes of everything git ignores in the worktree.
pub fn built(repo: &Repo) -> Result<u64> {
    Ok(ignored(repo)?.iter().map(|path| bytes_under(&repo.dir.join(path))).sum())
}

/// The bytes of the files git tracks in the worktree's commit.
pub fn source(repo: &Repo) -> Result<u64> {
    let listed = repo.git_raw(&["ls-tree", "-r", "-l", "-z", "HEAD"])?;
    // `<mode> <type> <object> <size>\t<path>`; a submodule's size is `-`
    Ok(listed
        .split('\0')
        .filter_map(|row| row.split('\t').next()?.split_whitespace().nth(3)?.parse::<u64>().ok())
        .sum())
}

fn note(repo: &Repo, name: &str) -> Result<std::path::PathBuf> {
    Ok(repo.git_dir()?.join(name))
}

fn noted(repo: &Repo, name: &str) -> Result<Option<u64>> {
    let noted = std::fs::read_to_string(note(repo, name)?).unwrap_or_default();
    Ok(noted.trim().parse().ok())
}

fn write_note(repo: &Repo, name: &str, bytes: u64) -> Result<()> {
    let note = note(repo, name)?;
    std::fs::write(&note, format!("{bytes}\n"))
        .with_context(|| format!("cannot write {}", note.display()))
}

/// The bytes on record as the worktree's fresh build; `None` when none is.
pub fn fresh(repo: &Repo) -> Result<Option<u64>> {
    noted(repo, FRESH_NOTE)
}

/// Records `bytes` as the worktree's fresh build.
pub fn record_fresh(repo: &Repo, bytes: u64) -> Result<()> {
    write_note(repo, FRESH_NOTE, bytes)
}

/// Records `bytes` as what the worktree built, by its landing's measure.
pub fn record_built(repo: &Repo, bytes: u64) -> Result<()> {
    write_note(repo, BUILT_NOTE, bytes)
}

/// What the last landing in the worktree measured of it; `None` when no
/// landing there did.
pub fn measured(repo: &Repo) -> Result<Option<Built>> {
    let Some(built) = noted(repo, BUILT_NOTE)? else { return Ok(None) };
    Ok(Some(Built { built, fresh: fresh(repo)? }))
}

/// `bytes` as a person reads them: `1.5 GiB`, `300 MiB`, `12 B`.
pub fn said(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

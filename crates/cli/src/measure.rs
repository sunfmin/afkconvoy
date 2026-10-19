//! How much the train's worktree holds: what git ignores in it — what gate
//! runs built — and the files git tracks.

use std::path::Path;

use anyhow::{Context, Result};

use crate::git::Repo;

/// The note of the bytes the worktree held after its first gate run, kept in
/// the worktree's own git dir: gone when the worktree is.
const FRESH_NOTE: &str = "convoy-fresh-build";

/// The paths git ignores in the worktree, an ignored directory as one path.
fn ignored(repo: &Repo) -> Result<Vec<String>> {
    let listed = repo.git_raw(&[
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ])?;
    Ok(listed.split('\0').filter(|path| !path.is_empty()).map(str::to_string).collect())
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

fn fresh_note(repo: &Repo) -> Result<std::path::PathBuf> {
    let dir = repo.git(&["rev-parse", "--absolute-git-dir"])?;
    Ok(Path::new(&dir).join(FRESH_NOTE))
}

/// The bytes on record as the worktree's fresh build; `None` when none is.
pub fn fresh(repo: &Repo) -> Result<Option<u64>> {
    let noted = std::fs::read_to_string(fresh_note(repo)?).unwrap_or_default();
    Ok(noted.trim().parse().ok())
}

/// Records `bytes` as the worktree's fresh build.
pub fn record_fresh(repo: &Repo, bytes: u64) -> Result<()> {
    let note = fresh_note(repo)?;
    std::fs::write(&note, format!("{bytes}\n"))
        .with_context(|| format!("cannot write {}", note.display()))
}

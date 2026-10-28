//! A harness's session for one worktree: the turn it has open, or the one
//! it ended. The fleet reads that record. Orca's receipt and the terminal
//! screen are not a turn.
//!
//! The harness names where the session is and what its text says
//! (docs/adr/0044). This module only opens files. A missing session is no
//! turn. A file that is there and cannot be read is an error, so a tick
//! does not treat it as idle.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use afkconvoy_core::harness::{Harness, SessionLayout};
use afkconvoy_core::turn::{Turn, prefer};
use anyhow::{Context, Result};

/// The turn `harness` has recorded for `cwd`.
///
/// `since` drops session files last written before the worker started, so a
/// killed worker's open file does not keep the next one busy. `not_before`
/// is the start proof's bound: a turn that started earlier is not this
/// send. A missing session directory is no turn. A file that is there and
/// cannot be read is an error.
pub fn read_turn(
    cwd: &Path,
    harness: Harness,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Option<Turn>> {
    let place = harness.session_place(&cwd_str(cwd));
    let mut roots = Vec::new();
    if let Some(home) = user_home() {
        roots.push(home.join(place.under_home));
    }
    roots.extend(kept_roots(harness)?);
    if let Some(name) = harness.noted_home()
        && let Some(home) = noted(cwd, name)
    {
        roots.push(home);
    }
    roots.sort();
    roots.dedup();
    let mut turns = Vec::new();
    for root in &roots {
        if let Some(turn) = read_root(root, harness, cwd, since, not_before)? {
            turns.push(turn);
        }
    }
    Ok(prefer(&turns))
}

/// Writes one turn for every harness under `home`, closed unless `open`.
/// The stand-in calls this. The files are the ones the harness names, so a
/// test does not keep a second copy of where a session lives.
pub fn record(home: &Path, cwd: &Path, open: bool, at: u64) -> Result<()> {
    let cwd = cwd_str(cwd);
    for harness in Harness::ALL {
        write_harness(home, harness, &cwd, open, at)?;
    }
    Ok(())
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// Directories the fleet keeps for `harness`, including a role's own home.
/// Each is already the harness directory.
fn kept_roots(harness: Harness) -> Result<Vec<PathBuf>> {
    let Ok(cache) = crate::copies::cache() else {
        return Ok(Vec::new());
    };
    let base = cache.join("harness");
    let mut roots = Vec::new();
    for name in [harness.key().to_string(), format!("{}-host-user", harness.key())] {
        let dir = base.join(name);
        if !dir.is_dir() {
            continue;
        }
        roots.push(dir.clone());
        let Some(entries) = list_dir(&dir)? else { continue };
        for entry in entries {
            let role = entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("role-"));
            if role && entry.is_dir() {
                roots.push(entry);
            }
        }
    }
    Ok(roots)
}

/// The harness directory a launch noted beside the worktree's git directory.
fn noted(cwd: &Path, name: &str) -> Option<PathBuf> {
    let git = std::fs::read_to_string(cwd.join(".git")).ok()?;
    let dir = git.trim().strip_prefix("gitdir:")?.trim();
    let home = std::fs::read_to_string(Path::new(dir).join(name)).ok()?;
    let home = home.trim();
    if home.is_empty() { None } else { Some(PathBuf::from(home)) }
}

fn read_root(
    root: &Path,
    harness: Harness,
    cwd: &Path,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Option<Turn>> {
    if !root.exists() {
        return Ok(None);
    }
    let cwd = cwd_str(cwd);
    let place = harness.session_place(&cwd);
    let turns = match place.layout {
        SessionLayout::Files { dir, extension } => {
            read_files(&root.join(dir), extension, since, |text| {
                harness.recorded_turn(&cwd, text, not_before)
            })?
        }
        SessionLayout::Tree { dir, extension } => {
            read_tree(&root.join(dir), extension, since, |text| {
                harness.recorded_turn(&cwd, text, not_before)
            })?
        }
        SessionLayout::Named { dir, name, compressed } => {
            read_named(&root.join(dir), name, compressed, since, |text| {
                harness.recorded_turn(&cwd, text, not_before)
            })?
        }
        SessionLayout::Index { file } => read_index(root, &file, harness, &cwd, since, not_before)?,
    };
    Ok(prefer(&turns))
}

fn read_files(
    dir: &Path,
    extension: &str,
    since: Option<u64>,
    mut turn_of: impl FnMut(&str) -> Option<Turn>,
) -> Result<Vec<Turn>> {
    let Some(entries) = list_dir(dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for path in entries {
        if path.extension().and_then(|ext| ext.to_str()) != Some(extension) {
            continue;
        }
        if !fresh(&path, since)? {
            continue;
        }
        if let Some(text) = read_text(&path)?
            && let Some(turn) = turn_of(&text)
        {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn read_tree(
    dir: &Path,
    extension: &str,
    since: Option<u64>,
    mut turn_of: impl FnMut(&str) -> Option<Turn>,
) -> Result<Vec<Turn>> {
    let mut files = Vec::new();
    collect(dir, extension, tree_floor(since), &mut files)?;
    let mut turns = Vec::new();
    for path in files {
        if let Some(text) = read_text(&path)?
            && let Some(turn) = turn_of(&text)
        {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn read_named(
    dir: &Path,
    name: &str,
    compressed: bool,
    since: Option<u64>,
    mut turn_of: impl FnMut(&str) -> Option<Turn>,
) -> Result<Vec<Turn>> {
    let Some(entries) = list_dir(dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for session in entries {
        let path = session.join(name);
        if !path.is_file() || !fresh(&path, since)? {
            continue;
        }
        let text = if compressed { read_zstd(&path)? } else { read_text(&path)? };
        if let Some(text) = text
            && let Some(turn) = turn_of(&text)
        {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn read_index(
    root: &Path,
    file: &str,
    harness: Harness,
    cwd: &str,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Vec<Turn>> {
    let Some(text) = read_text(&root.join(file))? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for session in harness.indexed_sessions(cwd, &text) {
        let path = PathBuf::from(&session);
        let path = if path.is_absolute() { path } else { root.join(path) };
        if !fresh(&path, since)? {
            continue;
        }
        if let Some(body) = read_text(&path)?
            && let Some(turn) = harness.recorded_turn(cwd, &body, not_before)
        {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn write_harness(home: &Path, harness: Harness, cwd: &str, open: bool, at: u64) -> Result<()> {
    let place = harness.session_place(cwd);
    let root = home.join(place.under_home);
    let files = harness.session_files(cwd, open, at);
    for file in &files {
        if let Some(name) = &file.replaces {
            remove_named(&root, name)?;
        }
    }
    for file in &files {
        let path = root.join(&file.path);
        if file.compressed {
            write_zstd(&path, &file.body, at)?;
        } else {
            write_text(&path, &file.body, at)?;
        }
    }
    if let SessionLayout::Index { file } = place.layout {
        let path = root.join(file);
        let existing = read_text(&path)?.unwrap_or_default();
        if let Some(body) = harness.session_index(cwd, &existing) {
            write_text(&path, &body, at)?;
        }
    }
    Ok(())
}

fn cwd_str(cwd: &Path) -> String {
    cwd.to_string_lossy().into_owned()
}

/// A tree of sessions is not read whole when the caller gave no start: two
/// days covers the turn a worker is in. A given start drops a file last
/// written before it, with the same two seconds `fresh` allows.
fn tree_floor(since: Option<u64>) -> u64 {
    since.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(2 * 86_400)
    })
}

fn fresh(path: &Path, since: Option<u64>) -> Result<bool> {
    let Some(at) = modified(path)? else { return Ok(false) };
    let floor = match since {
        Some(since) => since.saturating_sub(2),
        None => 0,
    };
    Ok(at + 2 >= floor)
}

fn modified(path: &Path) -> Result<Option<u64>> {
    match path.metadata() {
        Ok(meta) => Ok(meta
            .modified()
            .ok()
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map(|at| at.as_secs())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn list_dir(path: &Path) -> Result<Option<Vec<PathBuf>>> {
    let read = match std::fs::read_dir(path) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let mut out = Vec::new();
    for entry in read {
        out.push(entry.with_context(|| format!("cannot read {}", path.display()))?.path());
    }
    Ok(Some(out))
}

fn collect(dir: &Path, extension: &str, floor: u64, out: &mut Vec<PathBuf>) -> Result<()> {
    let Some(entries) = list_dir(dir)? else { return Ok(()) };
    for entry in entries {
        if entry.is_dir() {
            collect(&entry, extension, floor, out)?;
            continue;
        }
        if entry.extension().and_then(|ext| ext.to_str()) != Some(extension) {
            continue;
        }
        if modified(&entry)?.is_some_and(|at| at + 2 >= floor) {
            out.push(entry);
        }
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn read_zstd(path: &Path) -> Result<Option<String>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let mut decoder = zstd::stream::read::Decoder::new(file)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let mut text = String::new();
    decoder.read_to_string(&mut text).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(Some(text))
}

fn write_text(path: &Path, text: &str, at: u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    dated(path, at)
}

fn write_zstd(path: &Path, text: &str, at: u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    let file =
        std::fs::File::create(path).with_context(|| format!("cannot write {}", path.display()))?;
    let mut encoder = zstd::stream::write::Encoder::new(file, 0)
        .with_context(|| format!("cannot write {}", path.display()))?;
    encoder
        .write_all(text.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))?;
    encoder.finish().with_context(|| format!("cannot write {}", path.display()))?;
    dated(path, at)
}

/// A stamp ahead of the wall clock is the fleet's own clock. The start
/// proof skips a file last written before the handoff, so the file has to
/// carry that stamp as its mtime or the turn is never read.
fn dated(path: &Path, at: u64) -> Result<()> {
    let wall = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if at <= wall {
        return Ok(());
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("cannot date {}", path.display()))?;
    file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(at))
        .with_context(|| format!("cannot date {}", path.display()))
}

fn remove_named(dir: &Path, name: &str) -> Result<()> {
    let Some(entries) = list_dir(dir)? else { return Ok(()) };
    for entry in entries {
        if entry.is_dir() {
            remove_named(&entry, name)?;
            continue;
        }
        if entry.file_name().and_then(|file| file.to_str()) == Some(name) {
            std::fs::remove_file(&entry)
                .with_context(|| format!("cannot replace {}", entry.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_793_106_373;

    #[test]
    fn every_harness_round_trips_an_open_and_a_closed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        record(&home, &cwd, false, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &cwd, None, Some(AT)).unwrap();
            assert!(
                matches!(turn, Some(Turn::Closed { since: AT })),
                "{} closed: {turn:?}",
                harness.key()
            );
        }
        record(&home, &cwd, true, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &cwd, None, Some(AT - 2)).unwrap();
            assert!(
                matches!(turn, Some(Turn::Open { since: AT })),
                "{} open: {turn:?}",
                harness.key()
            );
            let earlier = read_root(&root, harness, &cwd, None, Some(AT + 1)).unwrap();
            assert_eq!(earlier, None, "{} must not prove a start before the bound", harness.key());
        }
    }

    #[test]
    fn a_long_worktree_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut cwd = dir.path().to_path_buf();
        for i in 0..40 {
            cwd.push(format!("segment-{i:02}"));
        }
        std::fs::create_dir_all(&cwd).unwrap();
        let home = dir.path().join("home");
        record(&home, &cwd, false, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &cwd, None, None).unwrap();
            assert_eq!(turn, Some(Turn::Closed { since: AT }), "{}", harness.key());
        }
    }

    #[test]
    fn a_noted_home_is_the_session_for_its_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let stored = dir.path().join("stored");
        record(&stored, &cwd, true, AT).unwrap();
        let place = Harness::Codex.session_place(&cwd_str(&cwd));
        let gitdir = dir.path().join("gitdir");
        std::fs::create_dir(&gitdir).unwrap();
        std::fs::write(cwd.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let note = Harness::Codex.noted_home().unwrap();
        std::fs::write(gitdir.join(note), stored.join(place.under_home).display().to_string())
            .unwrap();
        let turn = read_turn(&cwd, Harness::Codex, None, Some(AT - 2)).unwrap();
        assert_eq!(turn, Some(Turn::Open { since: AT }));
    }

    #[test]
    fn a_missing_session_directory_is_no_turn() {
        let dir = tempfile::tempdir().unwrap();
        for harness in Harness::ALL {
            let turn = read_root(dir.path(), harness, Path::new("/nowhere"), None, None).unwrap();
            assert_eq!(turn, None, "{}", harness.key());
        }
    }

    #[test]
    fn a_session_written_before_the_worker_started_is_not_this_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        record(&home, &cwd, true, AT).unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let stale = read_root(&root, harness, &cwd, Some(now + 60), Some(AT - 2)).unwrap();
            assert_eq!(stale, None, "{}", harness.key());
            let kept = read_root(&root, harness, &cwd, Some(now.saturating_sub(60)), Some(AT - 2))
                .unwrap();
            assert!(matches!(kept, Some(Turn::Open { since: AT })), "{}: {kept:?}", harness.key());
        }
    }

    #[test]
    fn another_worktree_is_not_this_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        let other = dir.path().join("other");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::create_dir(&other).unwrap();
        record(&home, &cwd, true, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &other, None, None).unwrap();
            assert_eq!(turn, None, "{}", harness.key());
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_session_file_that_cannot_be_read_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        record(&home, &cwd, true, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            deny_files(&root);
            let err = read_root(&root, harness, &cwd, None, None).unwrap_err();
            assert!(err.to_string().contains("cannot read"), "{}: {err}", harness.key());
            allow_files(&root);
        }
    }

    #[cfg(unix)]
    fn deny_files(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                deny_files(&path);
            } else {
                let mut perms = std::fs::metadata(&path).unwrap().permissions();
                perms.set_mode(0o0);
                std::fs::set_permissions(&path, perms).unwrap();
            }
        }
    }

    #[cfg(unix)]
    fn allow_files(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                allow_files(&path);
            } else {
                let mut perms = std::fs::metadata(&path).unwrap().permissions();
                perms.set_mode(0o644);
                std::fs::set_permissions(&path, perms).unwrap();
            }
        }
    }
}

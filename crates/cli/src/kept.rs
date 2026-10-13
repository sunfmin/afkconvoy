//! What this host keeps for a harness before it starts a worker of it
//! (`Harness::kept`): a directory of the fleet's own, beside its copies,
//! holding the files the tool carries for that harness and links to the few
//! files of the host user's the harness is to have. Which files is the
//! harness's to say; here they are only put there.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use afkconvoy_core::harness::Harness;
use anyhow::{Context, Result};

use crate::copies::cache;

/// Puts what this host keeps for `harness` in its directory as it stands
/// now → the directory. Every start does: a file the tool carries is as this
/// build has it, and a link follows what the host user has today — made when
/// the file is there, taken away when it no longer is. A harness nothing is
/// kept for gets no directory.
pub fn keep(harness: Harness) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from);
    keep_under(&cache()?, home.as_deref(), harness)
}

/// `keep`, on a host whose fleets keep what they keep under `cache` and whose
/// user's home is `home`.
pub fn keep_under(cache: &Path, home: Option<&Path>, harness: Harness) -> Result<PathBuf> {
    let dir = cache.join("harness").join(harness.key());
    let kept = harness.kept();
    let put =
        |what: &str| format!("cannot keep {what} for {} in {}", harness.name(), dir.display());
    for (name, text) in kept.files {
        carry(&dir.join(name), text).with_context(|| put(name))?;
    }
    for link in kept.links {
        let theirs = home.map(|home| home.join(link.home)).filter(|file| file.is_file());
        point(&dir.join(link.kept), theirs.as_deref()).with_context(|| put(link.kept))?;
    }
    Ok(dir)
}

/// Has `file` hold `text`, replaced in one step: a worker starting beside
/// this one never loads half of it.
fn carry(file: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(file).is_ok_and(|held| held == text) {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let draft = file.with_extension(format!("{}.draft", std::process::id()));
    std::fs::write(&draft, text)?;
    std::fs::rename(&draft, file)
}

/// Has `link` point at `theirs`, or be gone when there is none.
fn point(link: &Path, theirs: Option<&Path>) -> std::io::Result<()> {
    if std::fs::read_link(link).ok().as_deref() == theirs && link.is_symlink() == theirs.is_some() {
        return Ok(());
    }
    match std::fs::remove_file(link) {
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let Some(theirs) = theirs else { return Ok(()) };
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::os::unix::fs::symlink(theirs, link) {
        // Another start of this host put it there first: the same link.
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        made => made,
    }
}

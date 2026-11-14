//! Discarding a directory without deleting its files on the caller's thread
//! (sunfmin/control#987). A settled worktree carries its `target/`: deleting
//! its tens of thousands of files is seconds of kernel time, which on the host
//! runner's tick thread holds the tick and counts against its own budget.
//!
//! The directory is renamed into a trash directory beside it, on the same
//! filesystem: the rename is instant and frees the path at once. A detached
//! child under `nice` then deletes it, and whoever asked waits on none of it.
//! Trash a child did not finish — a host that restarted mid-delete — is swept
//! when the host runner next starts (`sweep`).

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

/// The trash directory beside each discarded directory.
pub const TRASH_DIR: &str = ".afkconvoy-trash";

/// The file, in the cache directory, naming each trash directory ever used:
/// what `sweep` looks in.
const KNOWN: &str = "trash-dirs";

/// What the child runs on the trash entry `$1`: the owner is given write on
/// each directory it cannot write (as `copies::removable`: a role copy is
/// read-only, directories included), and the entry is deleted.
const DELETE: &str =
    r#"find "$1" -type d ! -perm -u+w -exec chmod u+w {} + 2>/dev/null; exec rm -rf "$1""#;

/// Moves `path` into the trash beside it and has a detached child delete it
/// there → the entry it was moved to. On success `path` is free at once. An
/// error leaves `path` where it was: a different filesystem, or a parent the
/// caller cannot write.
pub fn discard(path: &Path) -> Result<PathBuf> {
    let entry = set_aside(path)?;
    delete_detached(&entry);
    Ok(entry)
}

/// Moves `path` into the trash beside it → the entry it is now at, which
/// nothing deletes yet: the caller deletes it (`delete_detached`) or puts it
/// back (`put_back`).
pub fn set_aside(path: &Path) -> Result<PathBuf> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        bail!("{} has no directory to be set aside in", path.display());
    };
    let trash = parent.join(TRASH_DIR);
    std::fs::create_dir_all(&trash)
        .with_context(|| format!("cannot make the trash {}", trash.display()))?;
    remember(&trash);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
    let entry = trash.join(format!("{}.{}.{nanos}", name.to_string_lossy(), std::process::id()));
    // Moving a directory to another parent rewrites its `..`: it needs its
    // owner's write, which a read-only role copy left at the top lacks.
    give_write(path);
    std::fs::rename(path, &entry).with_context(|| {
        format!("cannot move {} into the trash {}", path.display(), trash.display())
    })?;
    Ok(entry)
}

/// Puts an entry `set_aside` made back at `path`, which must not be there.
pub fn put_back(entry: &Path, path: &Path) -> Result<()> {
    std::fs::rename(entry, path)
        .with_context(|| format!("cannot put {} back at {}", entry.display(), path.display()))
}

/// Starts a detached, low-priority child that deletes `entry`. Nothing waits
/// on it but a thread reaping it: the deletion is the child's own CPU. A
/// child that cannot start leaves the entry to the next `sweep`.
pub fn delete_detached(entry: &Path) {
    let child = Command::new("nice")
        .args(["-n", "20", "/bin/sh", "-c", DELETE, "sh"])
        .arg(entry)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    if let Ok(mut child) = child {
        // A child nobody waits on stays a zombie while the host runner lives.
        std::thread::spawn(move || child.wait());
    }
}

/// Deletes what is left in every trash directory a discard has used: entries
/// a child did not finish, because the host stopped mid-delete. One a child
/// is still deleting is deleted twice over, which `rm -rf` allows.
pub fn sweep() {
    for trash in known() {
        for entry in std::fs::read_dir(&trash).into_iter().flatten().flatten() {
            delete_detached(&entry.path());
        }
    }
}

fn give_write(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(found) = std::fs::symlink_metadata(path) else { return };
    let mode = found.permissions().mode();
    if found.is_dir() && (mode & 0o200) == 0 {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o200));
    }
}

fn known_file() -> Option<PathBuf> {
    crate::copies::cache().ok().map(|cache| cache.join(KNOWN))
}

/// The trash directories named in the known file that are still there.
fn known() -> Vec<PathBuf> {
    let Some(file) = known_file() else { return Vec::new() };
    let read = std::fs::read_to_string(file).unwrap_or_default();
    let mut dirs: Vec<PathBuf> =
        read.lines().filter(|line| !line.is_empty()).map(PathBuf::from).collect();
    dirs.sort();
    dirs.dedup();
    dirs.retain(|dir| dir.is_dir());
    dirs
}

/// Names `trash` in the known file, where it is not yet. Best effort: a trash
/// not named there is still deleted by its own child, and is only missed by
/// a sweep.
fn remember(trash: &Path) {
    use std::io::Write;
    let Some(file) = known_file() else { return };
    let line = trash.display().to_string();
    if std::fs::read_to_string(&file).is_ok_and(|read| read.lines().any(|known| known == line)) {
        return;
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut known) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(known, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use super::*;

    fn gone_soon(path: &Path) -> bool {
        let since = Instant::now();
        while since.elapsed() < Duration::from_secs(20) {
            if !path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_discarded_directory_frees_its_path_at_once_and_a_child_deletes_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("worktree");
        let copy = path.join("target/role/inner");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("file"), "x").unwrap();
        // A read-only role copy, directories included, and the top too.
        for dir in [&copy, &path.join("target/role"), &path] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }

        let entry = discard(&path).unwrap();

        assert!(!path.exists(), "the path is free once discard answers");
        assert_eq!(entry.parent(), Some(root.path().join(TRASH_DIR).as_path()));
        std::fs::create_dir(&path).unwrap();
        assert!(gone_soon(&entry), "the child deletes {}", entry.display());
        assert!(path.is_dir(), "the reused path is not touched");
    }

    #[test]
    fn an_entry_set_aside_can_be_put_back() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("worktree");
        std::fs::create_dir_all(path.join("src")).unwrap();

        let entry = set_aside(&path).unwrap();
        put_back(&entry, &path).unwrap();

        assert!(path.join("src").is_dir());
        assert!(!entry.exists());
    }
}

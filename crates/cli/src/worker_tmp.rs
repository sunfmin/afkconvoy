//! A worker's `TMPDIR` (docs/adr/0019): its own, short enough for a socket's
//! path however long its worktree's is, and what is written there its
//! worktree's.
//!
//! It is a link, `/tmp/afkconvoy-<uid>/<the worktree's key>`, to `TMP_DIR` in
//! the worktree: a directory git ignores, by a `.gitignore` of its own, and
//! which goes with the worktree. A worktree is named for its issue's title,
//! and macOS holds a socket's path in 104 bytes: a tool that binds a socket
//! under a `TMPDIR` in the worktree — sccache, starting its server — cannot.
//! The link is named for the worktree's whole path, so it is unique on the
//! host, across its fleets, and the same after `run` starts again: `hot`
//! finds a worker's processes by it.
//!
//! A process started with it can outlive the worktree: a server shared by
//! the host that the worker started, which keeps the `TMPDIR` it was started
//! with — sccache's makes a directory under it for every compile, whoever
//! asks for it. So before a worktree is removed its link is turned, in one
//! step, to the fleet's own temporary directory, and a link no worktree
//! holds is removed once no process started with it runs.

use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::hot;

/// A worker's `TMPDIR` in its worktree, where its link points: what it writes
/// there is its own, and goes when the worktree does.
pub const TMP_DIR: &str = ".afkconvoy-tmp";

/// The worker's `TMPDIR` for the worktree at `worktree`: its link, made or
/// not. This is the one place it is said. The worktree's path is keyed as its
/// parts: `/w/a/` is `/w/a`.
pub fn of(worktree: &Path) -> PathBuf {
    let worktree: PathBuf = worktree.components().collect();
    let key = afkconvoy_core::gate::key(&worktree.to_string_lossy());
    links().join(format!("{key:016x}"))
}

/// The worker's `TMPDIR` for the worktree at `worktree`, made if it was not:
/// `TMP_DIR` in it, and the link to that. Links no worktree holds are
/// reclaimed on the way (`reclaim`).
pub fn make(worktree: &Path) -> Result<PathBuf> {
    let dir = worktree.join(TMP_DIR);
    let ignore = dir.join(".gitignore");
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&ignore, "*\n"))
        .with_context(|| format!("cannot make the worker's TMPDIR {}", dir.display()))?;
    let link = of(worktree);
    own_links()?;
    if std::fs::read_link(&link).ok().as_deref() != Some(&dir) {
        turn(&link, &dir)
            .with_context(|| format!("cannot link the worker's TMPDIR {}", link.display()))?;
    }
    // One that cannot be reclaimed now is at the next.
    let _ = reclaim();
    Ok(link)
}

/// Turns the link of the worktree at `worktree` to the fleet's own temporary
/// directory, before the worktree is removed: a process started with it that
/// outlives the worktree still has a directory there. A worktree no worker
/// was started in has no link, and one turned already is left as it is.
pub fn retire(worktree: &Path) -> Result<()> {
    let link = of(worktree);
    if std::fs::read_link(&link).is_ok_and(|to| to == worktree.join(TMP_DIR)) {
        turn(&link, &std::env::temp_dir())
            .with_context(|| format!("cannot turn the worker's TMPDIR {}", link.display()))?;
    }
    Ok(())
}

/// Reclaims the links no worktree holds — turned at its removal, or left
/// pointing at nothing by one removed some other way: each no running process
/// was started with is removed; one a process was — a server shared by the
/// host — is kept, and turned to the fleet's own temporary directory where
/// it points at nothing.
pub fn reclaim() -> Result<()> {
    let Ok(entries) = std::fs::read_dir(links()) else { return Ok(()) };
    // A worktree holds its own link while it is there: turned, the link
    // points at a directory of the fleet's, which may be a worker's own.
    let held = |link: &Path, to: &Path| {
        to.file_name() == Some(TMP_DIR.as_ref())
            && to.parent().is_some_and(|worktree| of(worktree) == link)
            && to.is_dir()
    };
    let unheld: Vec<(PathBuf, PathBuf)> = entries
        .map_while(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|link| link.file_name().is_some_and(|name| is_key(&name.to_string_lossy())))
        .filter_map(|link| Some((std::fs::read_link(&link).ok()?, link)))
        .filter(|(to, link)| !held(link, to))
        .map(|(to, link)| (link, to))
        .collect();
    if unheld.is_empty() {
        return Ok(());
    }
    let procs = hot::listed();
    for (link, to) in unheld {
        let tmp = link.display().to_string();
        if procs.iter().any(|proc| hot::started_with(proc, &tmp)) {
            if !to.is_dir() {
                turn(&link, &std::env::temp_dir())?;
            }
            continue;
        }
        // Made again for a worktree since it was read: its worker's now.
        if std::fs::read_link(&link).ok().as_ref() != Some(&to) {
            continue;
        }
        match std::fs::remove_file(&link) {
            Err(error) if error.kind() != ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(())
}

/// Whether `dir`, a process's `TMPDIR`, is a worker's link (`of`) or under
/// one: a fleet of this host started that process, or one that did, whether
/// the worktree is still there or not.
pub fn is_workers(dir: &str) -> bool {
    let Ok(rest) = Path::new(dir).strip_prefix(links()) else { return false };
    rest.components().next().is_some_and(|key| is_key(&key.as_os_str().to_string_lossy()))
}

/// The directory of this user's links, `/tmp/afkconvoy-<uid>`: as short as a
/// path that is the user's alone can be, whoever is logged in.
fn links() -> PathBuf {
    // SAFETY: getuid takes nothing, cannot fail and changes nothing.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/afkconvoy-{uid}"))
}

/// The directory of this user's links, made if it was not; an error where
/// it is not this user's own, as a link put in it would not be either.
fn own_links() -> Result<PathBuf> {
    let dir = links();
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Err(error) if error.kind() != ErrorKind::AlreadyExists => {
            return Err(error).with_context(|| format!("cannot make {}", dir.display()));
        }
        _ => {}
    }
    let meta = std::fs::symlink_metadata(&dir)
        .with_context(|| format!("cannot read {}", dir.display()))?;
    // SAFETY: as in `links`.
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        bail!(
            "{} is not a directory of this user's own: no worker's TMPDIR is linked in it",
            dir.display()
        );
    }
    Ok(dir)
}

/// Whether `name` is one a link is given (`of`): a worktree's key, and not a
/// link being made (`turn`).
fn is_key(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Has `link` point at `to`, replaced in one step: a process started with it
/// never finds it gone.
fn turn(link: &Path, to: &Path) -> std::io::Result<()> {
    let draft = link.with_extension(format!("{}.draft", crate::unique()));
    std::os::unix::fs::symlink(to, &draft)?;
    std::fs::rename(&draft, link).inspect_err(|_| {
        let _ = std::fs::remove_file(&draft);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};

    /// What macOS gives a user for a temporary directory,
    /// `getconf DARWIN_USER_TEMP_DIR`: `/var/folders/xx/<28 characters>/T/`.
    const MACOS_TMPDIR: usize = 49;

    /// A worktree whose path is `long` characters at the least, made under a
    /// directory of the test's own.
    fn worktree(under: &Path, long: usize) -> PathBuf {
        let mut path = under.join("issue-434-unit-tests-write-tick-logs-into-the-host");
        while path.as_os_str().len() < long {
            path = path.join("and-deeper");
        }
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_workers_tmpdir_holds_a_socket_however_long_its_worktrees_path() {
        let under = tempfile::tempdir().unwrap();
        // as long as #434's: its `.afkconvoy-tmp` was 106 characters
        let worktree = worktree(under.path(), 106 - "/.afkconvoy-tmp".len());
        assert!(worktree.join(TMP_DIR).as_os_str().len() >= 106);

        let tmp = make(&worktree).unwrap();
        assert_eq!(tmp, of(&worktree));
        assert!(tmp.as_os_str().len() <= MACOS_TMPDIR, "{}", tmp.display());
        let socket = tmp.join(format!("{:-<50}", "a-socket-as-sccache-binds-under-its-own-tmpdir"));
        assert_eq!(socket.as_os_str().len() - tmp.as_os_str().len() - 1, 50);
        let bound = UnixListener::bind(&socket);
        assert!(bound.is_ok(), "{}: {bound:?}", socket.display());

        // what is written there is the worktree's
        assert!(worktree.join(TMP_DIR).join(socket.file_name().unwrap()).exists());
        retire(&worktree).unwrap();
        drop(bound);
        reclaim().unwrap();
    }

    #[test]
    fn a_workers_tmpdir_is_one_place_for_its_worktree_alone() {
        let a = of(Path::new("/w/issue-7-a"));
        assert_eq!(a, of(Path::new("/w/issue-7-a")), "the same after a restart");
        assert_eq!(a, of(Path::new("/w/issue-7-a/")));
        assert_ne!(a, of(Path::new("/w/issue-7-b")));
        assert_ne!(a, of(Path::new("/v/issue-7-a")), "another fleet's");
        assert_eq!(a.parent(), Some(links().as_path()));
    }

    #[test]
    fn a_link_is_removed_with_its_worktree_once_no_process_was_started_with_it() {
        let under = tempfile::tempdir().unwrap();
        let worktree = worktree(under.path(), 0);
        let tmp = make(&worktree).unwrap();

        retire(&worktree).unwrap();
        // turned, or reclaimed already by another start on this host
        assert_ne!(std::fs::read_link(&tmp).ok(), Some(worktree.join(TMP_DIR)));
        std::fs::remove_dir_all(&worktree).unwrap();
        reclaim().unwrap();
        assert!(tmp.symlink_metadata().is_err(), "{} is left", tmp.display());
    }

    /// Stands in for a server a worker started: it keeps the `TMPDIR` it was
    /// started with, and is a program whose environment `ps` reads, as it
    /// reads no system program's.
    #[test]
    #[ignore = "a_link_outlives_its_worktree_while_a_process_started_with_it_runs runs it"]
    fn a_server() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[test]
    fn a_link_outlives_its_worktree_while_a_process_started_with_it_runs() {
        let under = tempfile::tempdir().unwrap();
        let worktree = worktree(under.path(), 0);
        let tmp = make(&worktree).unwrap();
        let mut server = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "worker_tmp::tests::a_server", "--ignored"])
            .env("TMPDIR", &tmp)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let reclaimed = || {
            reclaim().unwrap();
            std::fs::read_link(&tmp).ok()
        };

        // removed some other way than the fleet's: it points at nothing, and
        // is turned
        std::fs::remove_dir_all(&worktree).unwrap();
        assert_eq!(reclaimed(), Some(std::env::temp_dir()));
        assert!(tmp.is_dir(), "the server still has a directory there");

        server.kill().unwrap();
        server.wait().unwrap();
        assert_eq!(reclaimed(), None);
    }
}

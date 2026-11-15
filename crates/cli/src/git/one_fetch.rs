//! One fetch at a time into an object store (sunfmin/control#1102).
//!
//! Every worktree of a checkout shares its `.git`: the workers landing at
//! once, the fleet's own reads, the candidates' gates. A fetch into that
//! store takes its fetch lock, so N lands that need the same commit make one
//! fetch: the first fetches, and each after it finds the commit there.
//!
//! A fetch whose pack came in corrupt — `fatal: pack has 4 unresolved
//! deltas` — is fetched again once with `--refetch`, which asks the remote
//! for everything anew instead of building on the store. One that fails
//! corrupt again is noted in the store (`Failing`): the next fetches wait
//! out a growing backoff instead of fetching every tick, and `doctor` names
//! the error until a fetch succeeds.
//!
//! The fleet keeps git's own housekeeping off its checkout
//! (`maintenance_off`) and repacks under the same lock (`housekeep`), so no
//! repack rewrites the packs a fetch is reading.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{Ran, Repo};

/// The file in the git common dir every fetch into it locks.
const LOCK: &str = "convoy-fetch.lock";

/// The note in the git common dir of the fetch that keeps failing corrupt.
pub(crate) const FAILING: &str = "convoy-fetch-failing.json";

/// The first wait after a fetch failed corrupt twice; each failure after
/// doubles it, up to `MOST_BACKOFF`.
const FIRST_BACKOFF: u64 = 60;
const MOST_BACKOFF: u64 = 30 * 60;

/// How long housekeeping may run under the fetch lock before its process
/// group is ended.
const HOUSEKEEP_LIMIT: Duration = Duration::from_secs(600);

/// What git says of a pack it could not take in: the store, or the pack the
/// remote sent, is corrupt, and fetching it the same way fails the same way.
const CORRUPT: [&str; 6] = [
    "unresolved deltas",
    "invalid index-pack output",
    "index-pack failed",
    "did not receive expected object",
    "inflate: data stream error",
    "bad object",
];

/// Whether the fetch that said `stderr` failed on a corrupt pack.
pub(crate) fn corrupt(stderr: &str) -> bool {
    CORRUPT.iter().any(|said| stderr.contains(said))
}

/// The fetch of one ref that failed corrupt even with `--refetch`, as the
/// store notes it until a fetch succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Failing {
    /// The ref whose fetch failed last.
    pub(crate) refname: String,
    /// What git said, on one line.
    pub(crate) error: String,
    /// When the first of these failures was.
    pub(crate) since: u64,
    /// How many fetches in a row failed so.
    pub(crate) failures: u32,
    /// When the last one was.
    pub(crate) last: u64,
}

impl Failing {
    /// When the next fetch may be made: the backoff doubles with each failure.
    pub(crate) fn next(&self) -> u64 {
        let doubled = FIRST_BACKOFF.saturating_mul(1 << self.failures.saturating_sub(1).min(16));
        self.last.saturating_add(doubled.min(MOST_BACKOFF))
    }
}

/// The fetch lock of the store at `common`, held until it is dropped. With
/// `wait` unset, `None` when another holds it.
pub(super) fn take(common: &Path, wait: bool) -> Result<Option<File>> {
    let path = common.join(LOCK);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let mode = if wait { libc::LOCK_EX } else { libc::LOCK_EX | libc::LOCK_NB };
    // SAFETY: a lock of a live descriptor, released as it closes.
    if unsafe { libc::flock(file.as_raw_fd(), mode) } == 0 {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    ensure!(
        !wait && error.kind() == std::io::ErrorKind::WouldBlock,
        "cannot lock {}: {error}",
        path.display()
    );
    Ok(None)
}

/// The fetch that keeps failing corrupt in the store at `common`, if one does.
pub(crate) fn failing_at(common: &Path) -> Option<Failing> {
    let text = std::fs::read_to_string(common.join(FAILING)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Fetches `refname` into the store at `common` unless `here` finds what is
/// wanted there once the lock is held — another fetched it meanwhile. `run`
/// makes the fetch, `--refetch` added when it is told so. A fetch that
/// failed corrupt twice is noted, and none is made until its backoff ends.
pub(crate) fn guarded(
    common: &Path,
    refname: &str,
    now: u64,
    mut here: impl FnMut() -> Result<bool>,
    mut run: impl FnMut(bool) -> Result<Ran>,
) -> Result<()> {
    let _lock = take(common, true)?;
    if here()? {
        return Ok(());
    }
    let noted = failing_at(common);
    if let Some(failing) = noted.as_ref().filter(|failing| now < failing.next()) {
        anyhow::bail!(
            "fetching {refname} waits until {} after {} fetch(es) of {} failed corrupt since {}: {} \
             — `afkconvoy doctor` names it",
            failing.next(),
            failing.failures,
            failing.refname,
            failing.since,
            failing.error
        );
    }
    let mut ran = run(false)?;
    if !ran.ok && corrupt(&ran.stderr) {
        ran = run(true)?;
    }
    let file = common.join(FAILING);
    if ran.ok {
        if noted.is_some() {
            let _ = std::fs::remove_file(&file);
        }
        return Ok(());
    }
    let error = ran.stderr.split_whitespace().collect::<Vec<_>>().join(" ");
    if corrupt(&ran.stderr) {
        let failing = match noted {
            Some(was) => Failing {
                refname: refname.to_string(),
                error: error.clone(),
                failures: was.failures.saturating_add(1),
                last: now,
                ..was
            },
            None => Failing {
                refname: refname.to_string(),
                error: error.clone(),
                since: now,
                failures: 1,
                last: now,
            },
        };
        std::fs::write(&file, serde_json::to_string(&failing)?)
            .with_context(|| format!("cannot write {}", file.display()))?;
    }
    anyhow::bail!("git fetch {refname} failed: {error}")
}

/// The git config that keeps git's own housekeeping off a fleet's checkout:
/// no auto-gc after a fetch, no scheduled maintenance task that repacks.
pub(crate) const MAINTENANCE_OFF: [(&str, &str); 6] = [
    ("maintenance.auto", "false"),
    ("gc.auto", "0"),
    ("maintenance.gc.enabled", "false"),
    ("maintenance.incremental-repack.enabled", "false"),
    ("maintenance.loose-objects.enabled", "false"),
    ("maintenance.geometric-repack.enabled", "false"),
];

impl Repo {
    /// Keeps git's own housekeeping off this repository (`MAINTENANCE_OFF`):
    /// each key is written only where it says otherwise.
    pub fn maintenance_off(&self) -> Result<()> {
        for (key, value) in MAINTENANCE_OFF {
            let set = self.try_git(&["config", "--local", "--get", key])?;
            if set.stdout != value {
                self.git(&["config", "--local", key, value])?;
            }
        }
        Ok(())
    }

    /// Repacks this repository as git's auto-gc would, under the fetch lock
    /// and only while nothing fetches: one that holds the lock leaves it to
    /// a later call. → whether it ran.
    pub fn housekeep(&self) -> Result<bool> {
        let Some(_lock) = take(self.common_dir()?, false)? else {
            return Ok(false);
        };
        let args = [
            "-c",
            "gc.auto=6700",
            "-c",
            "gc.autoPackLimit=50",
            "-c",
            "gc.autoDetach=false",
            "gc",
            "--auto",
            "--quiet",
        ];
        let ran = super::run_within(&self.dir, &args, &[], HOUSEKEEP_LIMIT)?;
        ensure!(ran.ok, "git gc --auto failed: {}", ran.stderr.trim());
        Ok(true)
    }

    /// The fetch that keeps failing corrupt in this repository, if one does.
    pub(crate) fn failing_fetch(&self) -> Option<Failing> {
        failing_at(self.common_dir().ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn ran(ok: bool, stderr: &str) -> Ran {
        Ran { ok, stdout: String::new(), stderr: stderr.to_string() }
    }

    const DELTAS: &str = "fatal: pack has 4 unresolved deltas\nfatal: fetch-pack: invalid \
                          index-pack output\n";

    /// Eight workers landing at once in one checkout, each in its own
    /// worktree, each bringing the candidate chain's tip: one fetch reaches
    /// the remote, and every one of them has the commit.
    #[test]
    fn eight_lands_at_once_make_one_fetch_of_the_candidates_ref() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let (bare, seed, clone) = (root.join("bare.git"), root.join("seed"), root.join("clone"));
        git(root, &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
        git(root, &["init", "-q", "-b", "main", seed.to_str().unwrap()]);
        let commit = |dir: &Path, message: &str| {
            git(
                dir,
                &[
                    "-c",
                    "user.name=a",
                    "-c",
                    "user.email=a@b",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    message,
                ],
            );
            git(dir, &["rev-parse", "HEAD"])
        };
        commit(&seed, "base");
        git(&seed, &["push", "-q", bare.to_str().unwrap(), "HEAD:refs/heads/main"]);
        git(root, &["clone", "-q", bare.to_str().unwrap(), clone.to_str().unwrap()]);
        let tip = commit(&seed, "candidate");
        git(&seed, &["push", "-q", bare.to_str().unwrap(), "HEAD:refs/convoy/candidates/main"]);
        // Each fetch the remote serves is counted, and is slow enough that
        // the eight meet while the first is in flight.
        let log = root.join("served");
        let pack = root.join("upload-pack");
        std::fs::write(
            &pack,
            format!(
                "#!/bin/sh\necho served >> '{}'\nsleep 1\nexec git-upload-pack \"$@\"\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&pack, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(&clone, &["config", "remote.origin.uploadpack", pack.to_str().unwrap()]);
        let worktrees: Vec<PathBuf> = (0..8)
            .map(|n| {
                let wt = root.join(format!("wt{n}"));
                git(&clone, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "HEAD"]);
                wt
            })
            .collect();
        let start = Arc::new(std::sync::Barrier::new(worktrees.len()));
        let lands: Vec<_> = worktrees
            .into_iter()
            .map(|wt| {
                let (start, tip) = (Arc::clone(&start), tip.clone());
                std::thread::spawn(move || {
                    let repo = Repo::new(wt);
                    start.wait();
                    repo.bring(&tip, "refs/convoy/candidates/main")
                })
            })
            .collect();
        for land in lands {
            land.join().unwrap().unwrap();
        }
        let served = std::fs::read_to_string(&log).unwrap();
        assert_eq!(served.lines().count(), 1, "{served}");
    }

    /// A fetch that says "unresolved deltas" is made again with `--refetch`;
    /// one that succeeds so leaves nothing noted.
    #[test]
    fn a_corrupt_fetch_is_made_again_with_refetch() {
        let store = tempfile::tempdir().unwrap();
        let mut asked = vec![];
        guarded(
            store.path(),
            "refs/convoy/candidates/main",
            100,
            || Ok(false),
            |refetch| {
                asked.push(refetch);
                Ok(if refetch { ran(true, "") } else { ran(false, DELTAS) })
            },
        )
        .unwrap();
        assert_eq!(asked, [false, true]);
        assert_eq!(failing_at(store.path()), None);
    }

    /// A second corrupt failure, with `--refetch`, is noted for `doctor`, and
    /// the fetches after it wait out a backoff that grows with each failure;
    /// one that succeeds clears the note.
    #[test]
    fn a_second_corrupt_failure_is_noted_and_backs_off() {
        let store = tempfile::tempdir().unwrap();
        let name = "refs/convoy/candidates/main";
        let mut asked = vec![];
        let error = guarded(
            store.path(),
            name,
            100,
            || Ok(false),
            |refetch| {
                asked.push(refetch);
                Ok(ran(false, DELTAS))
            },
        )
        .unwrap_err();
        assert_eq!(asked, [false, true]);
        assert!(format!("{error:#}").contains("unresolved deltas"), "{error:#}");
        let noted = failing_at(store.path()).unwrap();
        assert_eq!((noted.failures, noted.since, noted.next()), (1, 100, 160));
        assert!(noted.error.contains("unresolved deltas"));
        // Within the backoff no fetch is made.
        let error = guarded(
            store.path(),
            name,
            150,
            || Ok(false),
            |_| -> Result<Ran> { panic!("a fetch was made within its backoff") },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("waits until 160"), "{error:#}");
        // Past it, a fetch is made; failing again doubles the wait.
        let _ = guarded(store.path(), name, 160, || Ok(false), |_| Ok(ran(false, DELTAS)));
        let noted = failing_at(store.path()).unwrap();
        assert_eq!((noted.failures, noted.since, noted.next()), (2, 100, 280));
        guarded(store.path(), name, 280, || Ok(false), |_| Ok(ran(true, ""))).unwrap();
        assert_eq!(failing_at(store.path()), None);
    }

    /// A failure that is not a corrupt pack is not made again, nor noted.
    #[test]
    fn a_fetch_failing_otherwise_is_not_made_again() {
        let store = tempfile::tempdir().unwrap();
        let mut asked = vec![];
        let error = guarded(
            store.path(),
            "refs/heads/x",
            100,
            || Ok(false),
            |refetch| {
                asked.push(refetch);
                Ok(ran(false, "fatal: couldn't find remote ref refs/heads/x"))
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("couldn't find"), "{error:#}");
        assert_eq!(asked, [false]);
        assert_eq!(failing_at(store.path()), None);
    }

    /// Housekeeping does not run while a fetch holds the lock.
    #[test]
    fn housekeeping_waits_for_no_fetch() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q", "-b", "main", "."]);
        let repo = Repo::new(root.path().to_path_buf());
        repo.maintenance_off().unwrap();
        for (key, value) in MAINTENANCE_OFF {
            assert_eq!(git(root.path(), &["config", "--local", "--get", key]), value);
        }
        let common = repo.common_dir().unwrap().to_path_buf();
        let held = take(&common, true).unwrap();
        assert!(!repo.housekeep().unwrap());
        drop(held);
        assert!(repo.housekeep().unwrap());
    }
}

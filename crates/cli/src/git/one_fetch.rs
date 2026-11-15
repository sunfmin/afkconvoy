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
//! (`maintenance_off`) and packs the store itself (`housekeep`), under the
//! same lock and while the host's exclusive queue holds the repository whole,
//! so no repack rewrites the packs a fetch, a gate or a land is reading.

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
const CORRUPT: [&str; 8] = [
    "in the commit graph file but not in the object database",
    "could not load pack",
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

/// The fetch lock of the store at `common`, held until it is dropped, and
/// whether it had to wait for another to let it go: one that waited asks
/// first whether the fetch it waited for brought what it is for.
pub(super) fn take_after(common: &Path) -> Result<(File, bool)> {
    if let Some(file) = take(common, false)? {
        return Ok((file, false));
    }
    Ok((take(common, true)?.expect("a lock waited for is taken"), true))
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
        drop_indexes(common)?;
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

/// Removes the commit-graph and multi-pack-index of the store at `common`
/// before a corrupt fetch is made again (sunfmin/control#1090). A repack cut
/// short leaves them naming commits the store lost (`in the commit graph
/// file but not in the object database`) or a pack that is gone (`could not
/// load pack`), and every read then trusts them. Both only speed up reads:
/// git reads each pack's own `.idx` without them, and housekeeping writes
/// them again from what is there.
fn drop_indexes(common: &Path) -> Result<()> {
    let objects = common.join("objects");
    let (info, pack) = (objects.join("info"), objects.join("pack"));
    for index in [
        info.join("commit-graph"),
        info.join("commit-graphs"),
        pack.join("multi-pack-index"),
        pack.join("multi-pack-index.d"),
    ] {
        let gone = if index.is_dir() {
            std::fs::remove_dir_all(&index)
        } else {
            std::fs::remove_file(&index)
        };
        match gone {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                return Err(err).with_context(|| format!("cannot remove {}", index.display()));
            }
            _ => {}
        }
    }
    Ok(())
}

/// How many packs a store holds before it is packed, where its
/// `gc.autoPackLimit` says none: git's own default.
const PACK_LIMIT: u64 = 50;

/// How many loose objects a store holds before it is packed: git's own
/// `gc.auto` default.
const LOOSE: u64 = 6700;

/// The names of the files in `dir`; none where it cannot be read.
fn names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
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

    /// Packs this repository's object store as git's auto-gc would, and
    /// writes its commit-graph again, at a quiet moment: once it holds more
    /// packs than `gc.autoPackLimit` or too many loose objects (`packing`),
    /// under the fetch lock, taken only where no fetch holds it, and while the
    /// host's exclusive queue holds the repository whole for it — no gate,
    /// turn or land of it running or waiting (sunfmin/control#1101). One that
    /// finds any of them leaves it to a later call. → whether it packed.
    pub fn housekeep(&self) -> Result<bool> {
        self.housekeep_asking(&crate::copies::cache()?)
    }

    /// `housekeep`, asking the host runner whose socket is in `cache`.
    pub(crate) fn housekeep_asking(&self, cache: &Path) -> Result<bool> {
        let Some(limit) = self.packing()? else {
            return Ok(false);
        };
        let Some(_lock) = take(self.common_dir()?, false)? else {
            return Ok(false);
        };
        let (loose, packs) = (format!("gc.auto={LOOSE}"), format!("gc.autoPackLimit={limit}"));
        let args = [
            "-c",
            &loose,
            "-c",
            &packs,
            "-c",
            "gc.autoDetach=false",
            "-c",
            "gc.writeCommitGraph=true",
            "gc",
            "--auto",
            "--quiet",
        ];
        let packed = crate::exclusive::whole_repo(cache, self, "git gc --auto", || {
            super::run_within(&self.dir, &args, &[], HOUSEKEEP_LIMIT)
        })?;
        let Some(ran) = packed else {
            return Ok(false);
        };
        let ran = ran?;
        ensure!(ran.ok, "git gc --auto failed: {}", ran.stderr.trim());
        Ok(true)
    }

    /// The pack limit this store is packed by, where it is past it or holds
    /// more loose objects than git's auto-gc lets be (`LOOSE`, counted as git
    /// counts them, in `objects/17`); `None`: it need not be packed. The
    /// limit is the repository's `gc.autoPackLimit`, 50 where it says none;
    /// 0 packs no store for its packs alone, as git's auto-gc does not.
    fn packing(&self) -> Result<Option<u64>> {
        let said = self.try_git(&["config", "--get", "gc.autoPackLimit"])?;
        let limit = said.stdout.trim().parse().unwrap_or(PACK_LIMIT);
        let objects = self.common_dir()?.join("objects");
        let packs = names(&objects.join("pack"))
            .iter()
            .filter_map(|name| name.strip_suffix(".pack"))
            .filter(|stem| !objects.join("pack").join(format!("{stem}.keep")).exists())
            .count() as u64;
        let loose = names(&objects.join("17"))
            .iter()
            .filter(|name| name.len() == 38 && name.bytes().all(|b| b.is_ascii_hexdigit()))
            .count() as u64;
        let past = (limit > 0 && packs > limit) || loose > LOOSE.div_ceil(256);
        Ok(past.then_some(limit))
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

    /// A fetch that says the store's multi-pack-index names a pack that is
    /// gone is made again with `--refetch` once that index and the
    /// commit-graph are dropped (sunfmin/control#1090).
    #[test]
    fn a_stale_index_is_dropped_before_the_refetch() {
        let store = tempfile::tempdir().unwrap();
        let (info, pack) = (store.path().join("objects/info"), store.path().join("objects/pack"));
        std::fs::create_dir_all(info.join("commit-graphs")).unwrap();
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(info.join("commit-graph"), "").unwrap();
        std::fs::write(pack.join("multi-pack-index"), "").unwrap();
        let mut asked = vec![];
        guarded(
            store.path(),
            "refs/convoy/candidates/main",
            100,
            || Ok(false),
            |refetch| {
                asked.push(refetch);
                Ok(if refetch { ran(true, "") } else { ran(false, "error: could not load pack 0") })
            },
        )
        .unwrap();
        assert_eq!(asked, [false, true]);
        assert!(!info.join("commit-graph").exists());
        assert!(!info.join("commit-graphs").exists());
        assert!(!pack.join("multi-pack-index").exists());
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

    /// The packs of the store at `common`.
    fn packs(common: &Path) -> usize {
        names(&common.join("objects/pack")).iter().filter(|name| name.ends_with(".pack")).count()
    }

    /// Once a checkout holds more packs than its `gc.autoPackLimit`, it is
    /// packed only while the host's exclusive queue holds the repository
    /// whole, never while a gate of it runs or a fetch of it holds the
    /// lock, and the pack leaves the store whole, with its commit-graph
    /// written again (sunfmin/control#1101).
    #[test]
    fn a_store_past_its_pack_limit_is_packed_only_while_held_whole_and_no_fetch_runs() {
        use crate::exclusive::Queue;
        use crate::runner_socket::{self, Reply, Request};
        use afkconvoy_core::exclusive::{Gate, Note, Ticket};

        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q", "-b", "main", "."]);
        let repo = Repo::new(root.path().to_path_buf());
        repo.maintenance_off().unwrap();
        for (key, value) in MAINTENANCE_OFF {
            assert_eq!(git(root.path(), &["config", "--local", "--get", key]), value);
        }
        git(root.path(), &["config", "--local", "gc.autoPackLimit", "3"]);
        for at in 0..5 {
            std::fs::write(root.path().join("file"), format!("{at}")).unwrap();
            git(root.path(), &["add", "file"]);
            let named = ["-c", "user.name=a", "-c", "user.email=a@example.com"];
            git(root.path(), &[&named[..], &["commit", "-q", "-m", "a change"]].concat());
            git(root.path(), &["repack", "-d", "-q"]);
        }
        let common = std::fs::canonicalize(repo.common_dir().unwrap()).unwrap();
        assert_eq!(packs(&common), 5);

        // No host runner holds the queue: nothing keeps the repo's gates off.
        let nobody = tempfile::tempdir().unwrap();
        assert!(!repo.housekeep_asking(nobody.path()).unwrap());

        let cache = tempfile::tempdir().unwrap();
        let queue = Queue::with_leases(false);
        let listener = runner_socket::reached(cache.path(), runner_socket::SOCKET, |path| {
            std::os::unix::net::UnixListener::bind(path)
        });
        let serving = queue.clone();
        runner_socket::serve(
            listener.unwrap(),
            serde_json::json!({}),
            move |request| match request {
                Request::Exclusive { ticket } => serving.answer(ticket),
                Request::WholeRepo { ticket } => serving.whole_repo(ticket),
                _ => Reply::Line(serde_json::json!({})),
            },
        );
        let settled = |count: usize| {
            let until = std::time::Instant::now() + Duration::from_secs(5);
            while queue.standing().len() != count && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(queue.standing().len(), count, "{:?}", queue.standing());
        };

        // A fast gate of the repo runs: the store is not packed.
        let gate = Ticket {
            number: 0,
            pid: std::process::id(),
            command: "./gate.sh fast".into(),
            dir: root.path().to_string_lossy().into_owned(),
            cut: false,
            repo_lane: None,
            repo: Some(common.to_string_lossy().into_owned()),
            is_turn: false,
            candidate: false,
            whole_repo: false,
            note: Note::default(),
            gate: Some(Gate::Fast),
            asked: 0,
            ran: None,
        };
        let asked = Request::Exclusive { ticket: gate };
        let (_, running) =
            runner_socket::keep_asking_in(cache.path(), &asked, Duration::from_secs(5)).unwrap();
        let mut running = running.expect("kept on the line");
        assert!(running.heard().unwrap().expect("its turn")["runs"].is_u64());
        assert!(!repo.housekeep_asking(cache.path()).unwrap());
        assert_eq!(packs(&common), 5, "not packed beside a gate of its repo");
        settled(1);
        drop(running);
        settled(0);

        // A fetch holds the lock: the store is not packed.
        let held = take(&common, true).unwrap();
        assert!(!repo.housekeep_asking(cache.path()).unwrap());
        assert_eq!(packs(&common), 5, "not packed beside a fetch");
        settled(0);
        drop(held);

        // A quiet moment: packed, whole, its commit-graph written again.
        assert!(repo.housekeep_asking(cache.path()).unwrap());
        assert_eq!(packs(&common), 1);
        git(root.path(), &["fsck", "--connectivity-only"]);
        let info = common.join("objects/info");
        assert!(info.join("commit-graph").exists() || info.join("commit-graphs").exists());
        settled(0);
        assert!(!repo.housekeep_asking(cache.path()).unwrap(), "within its limit now");
    }
}

//! The checkout list: which checkouts this host's runner works. One file on
//! the host, never pushed, which holds the host settings beside them
//! (`host_settings`, docs/adr/0023).
//!
//! It is read whole by anyone, never under its lock: a change is written
//! whole beside it and moved over it, so a reader sees one list or the
//! next. Changes take the lock beside it, which keeps out a change made by
//! another thread of the same process as well as by another process.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use afkconvoy_core::checkouts;
use afkconvoy_core::config::RepoName;
use afkconvoy_core::harness::Harness;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::copies::cache;
use crate::gh::Tracker;
use crate::git::{REMOTE, Repo};
use crate::host_settings::Settings;
use crate::instance::Named;
use crate::orca::Worktree;

/// The file's name in the cache directory.
const FILE: &str = "checkouts.json";

/// What the checkout list's file holds: the checkouts, and the host settings
/// beside them.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Listed {
    #[serde(default)]
    pub checkouts: Vec<Entry>,
    #[serde(default)]
    pub settings: Settings,
    /// What another afkconvoy keeps beside them, kept as it is.
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// One checkout on the list: its root, what the `run` that started it named
/// for its fleet instance alone, the concurrency it says on this host
/// (`concurrency`), which its fleet instance reads at each tick, its
/// tracker (`tracker_of`), and the harness its workers start on
/// (`named_harness`), where it names one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub checkout: PathBuf,
    #[serde(default, skip_serializing_if = "Named::is_empty")]
    pub named: Named,
    /// `--concurrency`: none, and the default holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<NonZeroUsize>,
    /// The repo its fleet reads and writes issues in, where that is not the
    /// checkout's own: none, and it is its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<RepoName>,
    /// `--harness`: the harness this checkout's workers start on. None, and
    /// they start on the host kit's first eligible harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<Harness>,
    /// The key the last successful converge landed this checkout's workers
    /// on. `Some("")` is the host kit. Absent means a converge has not
    /// landed yet, which is not the same as having landed on the host kit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_applied: Option<String>,
}

/// What the `run` of a checkout the list holds no entry of yet asked of it,
/// in this process: what its first tick puts on the list with it — a
/// checkout whose first tick has not ended, or that a `run --foreground`
/// holds, goes by it until then.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    /// `--concurrency` (`concurrency`).
    pub concurrency: Option<NonZeroUsize>,
    /// `--tracker` (`crate::tracker`).
    pub tracker: Option<RepoName>,
    /// `--harness` (`named_harness`).
    pub harness: Option<Harness>,
}

/// What each `run` asked in this process, by its checkout's root.
static ASKED: Mutex<BTreeMap<PathBuf, Asked>> = Mutex::new(BTreeMap::new());

/// What each `run` asked in this process. Whoever reads it beside the list
/// holds it while they read the list, and a first tick forgets what was
/// asked (`forget`) only once the list holds it: a reader finds it in one or
/// the other.
pub fn asked() -> MutexGuard<'static, BTreeMap<PathBuf, Asked>> {
    ASKED.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the `run` of the checkout at `root` asked in this process.
pub fn asked_of(root: &Path) -> Asked {
    asked().get(root).cloned().unwrap_or_default()
}

/// Forgets what the `run` of the checkout at `root` asked in this process:
/// once its entry on the list holds it, or as a new start asks anew.
pub fn forget(root: &Path) {
    asked().remove(root);
}

/// A host's checkout list: its file, and the lock beside it.
#[derive(Clone)]
pub struct List {
    file: PathBuf,
}

impl List {
    /// This host's: `checkouts.json` in the cache directory.
    pub fn of_host() -> Result<List> {
        Ok(List::at(cache()?.join(FILE)))
    }

    /// The list kept in `file`.
    pub fn at(file: PathBuf) -> List {
        List { file }
    }

    /// The file it is kept in.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// What it lists. None yet is an empty list; a file that cannot be read
    /// is an error naming it.
    pub fn read(&self) -> Result<Listed> {
        self.read_with_settings().map(|(listed, _)| listed)
    }

    /// The list, and whether its file holds host settings, even defaults.
    /// A first `run` must distinguish settings never said from defaults
    /// already accepted; reading this makes no file or lock.
    pub fn read_with_settings(&self) -> Result<(Listed, bool)> {
        let bytes = match std::fs::read(&self.file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Listed::default(), false));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", self.file.display()));
            }
        };
        let read = || -> serde_json::Result<(Listed, bool)> {
            let listed = serde_json::from_slice(&bytes)?;
            let value: Value = serde_json::from_slice(&bytes)?;
            let settings = value.get("settings").is_some();
            Ok((listed, settings))
        };
        read().with_context(|| {
            format!(
                "{} cannot be read as the checkout list: mend it, or remove it and run \
                 `afkconvoy run` again in each checkout to be worked",
                self.file.display()
            )
        })
    }

    /// Changes it under its lock → what `change` answered: read, `change`d,
    /// written whole beside it (`checkouts.json.<unique()>.tmp`) and moved
    /// over it. A reader sees the old list or the new one, whole.
    pub fn change<T>(&self, change: impl FnOnce(&mut Listed) -> T) -> Result<T> {
        let unwritten = || format!("the checkout list {} cannot be written", self.file.display());
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).with_context(unwritten)?;
        }
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.file.with_extension("lock"))
            .with_context(unwritten)?;
        lock.lock().with_context(unwritten)?;
        let mut listed = self.read()?;
        let changed = change(&mut listed);
        let temporary = self.file.with_extension(format!("json.{}.tmp", crate::unique()));
        let written = serde_json::to_vec_pretty(&listed)
            .map_err(std::io::Error::other)
            .and_then(|text| std::fs::write(&temporary, text))
            .and_then(|()| std::fs::rename(&temporary, &self.file));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written.with_context(unwritten)?;
        Ok(changed)
    }
}

/// The tracker of the checkout `repo` on this host, as its entry on the
/// list says it (`tracker_on`).
pub fn tracker_of(repo: &Repo) -> Result<Tracker> {
    tracker_on(&List::of_host()?, repo)
}

/// The tracker of the checkout `repo` on the host of `list`, read once for
/// what is done next: as the entry of `repo` itself says it, or of its main
/// checkout, which every worktree of it shares — a worker's command acts on
/// the issues its fleet does. One the list holds no entry of, or whose entry
/// says none, is its own repo's. Another repo's issues are the checkout's
/// by its repo label, which names the repo its remote names (`named_repo`).
pub fn tracker_on(list: &List, repo: &Repo) -> Result<Tracker> {
    match said_tracker(list, repo)? {
        None => Ok(Tracker::Own),
        Some(tracker) => Ok(Tracker::Other { tracker, repo: named_repo(repo)? }),
    }
}

/// The tracker the checkout `repo` says on the host of `list`, with its
/// entry or its main checkout's — where the list holds neither, what its
/// `run` asked in this process: none, and it is its own repo.
pub fn said_tracker(list: &List, repo: &Repo) -> Result<Option<RepoName>> {
    Ok(said(list, repo)?.flatten())
}

/// What the checkout `repo` says of its tracker on the host of `list`, as
/// `said_tracker` reads it; `None` where it says nothing at all — the list
/// holds no entry of it or its main checkout, and no `run` of it asked in
/// this process — so the tracker is not said, which is not the same as its
/// own repo said. A worker's worktree says the tracker its fleet kept there
/// as it started the worker (`Worktree::keep_tracker`), which holds while
/// the list holds no entry of its checkout yet — a fleet's first tick, or a
/// `run --foreground` — and that kept nothing is read off the list as
/// another checkout is; where neither says it, it is an error: a worker's
/// command never acts on the issues of a tracker its fleet does not hold.
pub fn said(list: &List, repo: &Repo) -> Result<Option<Option<RepoName>>> {
    let worktree = Worktree::at(repo.dir.clone(), String::new());
    let worker = worktree.holds_a_worker();
    if worker && let Some(kept) = worktree.tracker()? {
        return Ok(Some(kept));
    }
    let main = repo.main_checkout()?;
    let of = |root: &Path| root == repo.dir || root == main;
    let asked = asked();
    let listed = list.read()?;
    let said = match listed.checkouts.into_iter().find(|entry| of(&entry.checkout)) {
        Some(entry) => Some(entry.tracker),
        None => asked.iter().find(|(root, _)| of(root)).map(|(_, asked)| asked.tracker.clone()),
    };
    if worker && said.is_none() {
        bail!(
            "the tracker of the worker in {} cannot be read: its fleet kept none in its \
             worktree, and the checkout list {} holds no entry of {}: a worker's command acts \
             on its fleet's tracker alone, which its fleet keeps there as it starts the worker",
            repo.dir.display(),
            list.file().display(),
            main.display()
        );
    }
    Ok(said)
}

/// The repo the checkout `repo` is of, as GitHub names it, `owner/name`:
/// what its remote `origin` is set to (`checkouts::repo_of`), in lower case,
/// before any `insteadOf` of git's config rewrites it. An error where that
/// names no repo of a host of git — a path on this one.
pub fn named_repo(repo: &Repo) -> Result<RepoName> {
    let set = repo.try_git(&["config", "--get", &format!("remote.{REMOTE}.url")])?;
    let url = set.stdout.trim();
    let named = checkouts::repo_of(url).and_then(|repo| {
        let (_, path) = repo.split_once('/')?;
        RepoName::read(path)
    });
    named.ok_or_else(|| {
        anyhow::anyhow!(
            "the remote `{REMOTE}` of {} is {url:?}, which names no repo on GitHub: in another \
             repo its issues carry its repo label, `repo:<owner>/<repo>`, which names the repo \
             its remote does",
            repo.dir.display()
        )
    })
}

/// The repo the checkout `repo` is of, as its remote `origin` names it
/// (`checkouts::repo_of`); a remote that is a path is that directory, made
/// canonical against the checkout. A checkout with no `origin` is an error
/// saying a fleet works its repo through it.
pub fn repo_of(repo: &Repo) -> Result<String> {
    let ran = repo.try_git(&["remote", "get-url", REMOTE])?;
    if !ran.ok {
        bail!(
            "{} has no remote `{REMOTE}`, and a fleet works its repo through that remote: add \
             it with `git remote add {REMOTE} <url>`",
            repo.dir.display()
        );
    }
    let url = ran.stdout.as_str();
    if let Some(repo) = checkouts::repo_of(url) {
        return Ok(repo);
    }
    let path = url.strip_prefix("file://").unwrap_or(url);
    let dir = repo.dir.join(path).canonicalize().with_context(|| {
        format!("the remote `{REMOTE}` of {}, {url}, is no directory here", repo.dir.display())
    })?;
    Ok(dir.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn entry(checkout: &str) -> Entry {
        let checkout = PathBuf::from(checkout);
        Entry {
            checkout,
            named: Named::default(),
            concurrency: None,
            tracker: None,
            harness: None,
            harness_applied: None,
        }
    }

    #[test]
    fn none_yet_is_an_empty_list_and_one_that_cannot_be_read_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join(FILE));
        assert!(list.read().unwrap().checkouts.is_empty());
        std::fs::write(list.file(), "{\"checkouts\": [").unwrap();
        let error = format!("{:#}", list.read().unwrap_err());
        assert!(error.contains(&list.file().display().to_string()), "{error}");
        assert!(list.change(|_| ()).is_err(), "a list that cannot be read is not written over");
        assert_eq!(std::fs::read_to_string(list.file()).unwrap(), "{\"checkouts\": [");
    }

    #[test]
    fn a_change_keeps_what_it_does_not_know_and_names_only_what_was_named() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join(FILE));
        let kept = r#"{"settings": {"no_host_sleep": true}, "kept_by_another": {"a": 1},
            "checkouts": [{"checkout": "/a"}]}"#;
        std::fs::write(list.file(), kept).unwrap();
        let named = Named { host: Some("h1".into()), ..Named::default() };
        let concurrency = NonZeroUsize::new(6);
        let added = list.change(|listed| {
            let checkout = PathBuf::from("/b");
            let named = named.clone();
            listed.checkouts.push(Entry {
                checkout,
                named,
                concurrency,
                tracker: None,
                harness: None,
                harness_applied: None,
            });
            listed.checkouts.len()
        });
        assert_eq!(added.unwrap(), 2);
        let text: Value =
            serde_json::from_str(&std::fs::read_to_string(list.file()).unwrap()).unwrap();
        assert_eq!(text["kept_by_another"], serde_json::json!({"a": 1}), "{text}");
        assert_eq!(text["settings"]["no_host_sleep"], true, "{text}");
        assert_eq!(text["settings"]["hot_cores"], Value::Null, "every setting, said or not");
        assert_eq!(text["checkouts"][0], serde_json::json!({"checkout": "/a"}), "{text}");
        assert_eq!(
            text["checkouts"][1],
            serde_json::json!({"checkout": "/b", "named": {"host": "h1"}, "concurrency": 6})
        );
        assert_eq!(
            list.read().unwrap().checkouts,
            [
                entry("/a"),
                Entry {
                    checkout: "/b".into(),
                    named,
                    concurrency,
                    tracker: None,
                    harness: None,
                    harness_applied: None,
                }
            ]
        );
        // a concurrency of no slot is none: the list cannot be read
        std::fs::write(list.file(), r#"{"checkouts": [{"checkout": "/a", "concurrency": 0}]}"#)
            .unwrap();
        assert!(list.read().is_err());
        // nothing is left beside it
        let left: Vec<_> =
            std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 2, "the list and its lock: {left:?}");
    }

    #[test]
    fn changes_made_at_once_are_each_kept() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let file = file.clone();
                std::thread::spawn(move || {
                    let list = List::at(file);
                    for m in 0..10 {
                        let checkout = format!("/{n}/{m}");
                        list.change(|listed| listed.checkouts.push(entry(&checkout))).unwrap();
                        // a reader never waits, and never sees half a list
                        assert!(list.read().is_ok());
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let listed = List::at(file).read().unwrap();
        assert_eq!(listed.checkouts.len(), 80);
    }

    #[test]
    fn a_checkout_and_each_worktree_of_it_go_by_the_tracker_its_entry_says() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let ran =
                crate::process::output(Command::new("git").args(args).current_dir(&root)).unwrap();
            assert!(ran.status.success(), "{ran:?}");
        };
        git(&["init", "--quiet", "w"]);
        git(&["-C", "w", "commit", "--quiet", "--allow-empty", "-m", "initial"]);
        git(&["-C", "w", "remote", "add", "origin", "git@github.com:Acme/Dev.git"]);
        git(&["-C", "w", "worktree", "add", "--quiet", "../wt"]);
        let (checkout, worktree) = (Repo::new(root.join("w")), Repo::new(root.join("wt")));
        let list = List::at(root.join(FILE));
        let other = Tracker::Other {
            tracker: RepoName::read("acme/tracker").unwrap(),
            repo: RepoName::read("acme/dev").unwrap(),
        };

        // none on the list, or none said: its own repo
        assert_eq!(tracker_on(&list, &checkout).unwrap(), Tracker::Own);
        list.change(|listed| listed.checkouts.push(entry(&checkout.dir.display().to_string())))
            .unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), Tracker::Own);

        let tracker = RepoName::read("acme/tracker");
        list.change(|listed| listed.checkouts[0].tracker = tracker).unwrap();
        assert_eq!(tracker_on(&list, &checkout).unwrap(), other);
        assert_eq!(
            tracker_on(&list, &worktree).unwrap(),
            other,
            "a worktree goes by its checkout's"
        );
        let text = std::fs::read_to_string(list.file()).unwrap();
        assert!(text.contains("\"tracker\": \"acme/tracker\""), "{text}");

        // a tracker that is no repo's name: the list cannot be read
        std::fs::write(list.file(), r#"{"checkouts": [{"checkout": "/a", "tracker": "acme"}]}"#)
            .unwrap();
        let error = format!("{:#}", list.read().unwrap_err());
        assert!(error.contains("`acme` is no repo's name"), "{error}");
    }

    #[test]
    fn a_workers_worktree_goes_by_the_tracker_its_fleet_kept_there() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let ran =
                crate::process::output(Command::new("git").args(args).current_dir(&root)).unwrap();
            assert!(ran.status.success(), "{ran:?}");
        };
        git(&["init", "--quiet", "w"]);
        git(&["-C", "w", "commit", "--quiet", "--allow-empty", "-m", "initial"]);
        git(&["-C", "w", "remote", "add", "origin", "git@github.com:Acme/Dev.git"]);
        git(&["-C", "w", "worktree", "add", "--quiet", "../wt"]);
        let (checkout, worktree) = (Repo::new(root.join("w")), Repo::new(root.join("wt")));
        let list = List::at(root.join(FILE));
        let held = Worktree::at(worktree.dir.clone(), String::new());
        let other = Tracker::Other {
            tracker: RepoName::read("acme/tracker").unwrap(),
            repo: RepoName::read("acme/dev").unwrap(),
        };

        // a worker's worktree whose tracker nothing says: an error, not its own repo
        std::fs::write(held.brief().unwrap(), "the brief").unwrap();
        let error = format!("{:#}", tracker_on(&list, &worktree).unwrap_err());
        assert!(error.contains("cannot be read"), "{error}");
        assert_eq!(tracker_on(&list, &checkout).unwrap(), Tracker::Own, "the checkout's is said");

        // what its fleet kept there holds while the list holds no entry
        held.keep_tracker(&other).unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), other);
        assert_eq!(tracker_on(&list, &checkout).unwrap(), Tracker::Own, "kept in the worktree");
        held.keep_tracker(&Tracker::Own).unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), Tracker::Own);

        // a worker's worktree that kept none goes by its checkout's entry
        std::fs::remove_file(held.note(crate::orca::TRACKER_FILE).unwrap()).unwrap();
        let tracker = RepoName::read("acme/tracker");
        list.change(|listed| {
            let mut kept = entry(&checkout.dir.display().to_string());
            kept.tracker = tracker;
            listed.checkouts.push(kept);
        })
        .unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), other);

        // a kept tracker that names no repo is an error
        std::fs::write(held.note(crate::orca::TRACKER_FILE).unwrap(), "acme").unwrap();
        let error = format!("{:#}", tracker_on(&list, &worktree).unwrap_err());
        assert!(error.contains("names no repo"), "{error}");
    }

    #[test]
    fn a_checkout_is_of_the_repo_its_origin_names() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("w");
        let git = |args: &[&str]| {
            let ran =
                crate::process::output(Command::new("git").args(args).current_dir(dir.path()))
                    .unwrap();
            assert!(ran.status.success(), "{ran:?}");
        };
        git(&["init", "--quiet", "--bare", "remote.git"]);
        git(&["init", "--quiet", "w"]);
        let repo = Repo::new(checkout.clone());
        let error = format!("{:#}", repo_of(&repo).unwrap_err());
        assert!(error.contains("has no remote `origin`"), "{error}");
        let set = |url: &str| git(&["-C", "w", "remote", "add", "origin", url]);
        set("git@github.com:Me/W.git");
        assert_eq!(repo_of(&repo).unwrap(), "github.com/me/w");
        git(&["-C", "w", "remote", "remove", "origin"]);
        // a path, however it is written, is the directory it names
        let bare = dir.path().join("remote.git").canonicalize().unwrap().display().to_string();
        for url in ["../remote.git", &bare, &format!("file://{bare}")] {
            set(url);
            assert_eq!(repo_of(&repo).unwrap(), bare, "{url}");
            git(&["-C", "w", "remote", "remove", "origin"]);
        }
        set("../gone.git");
        let error = format!("{:#}", repo_of(&repo).unwrap_err());
        assert!(error.contains("is no directory here"), "{error}");
    }
}

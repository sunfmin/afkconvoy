//! The checkout list: which checkouts this host's runner works, kept in the
//! host's database (`host_db`, docs/adr/0119), never pushed, with the host
//! settings beside them (`host_settings`, docs/adr/0023).
//!
//! It is read whole by anyone, and a reader sees one list or the next. A
//! change is one transaction of the database, which keeps out a change made
//! by another thread of the same process as well as by another process.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use afkconvoy_core::checkouts;
use afkconvoy_core::config::RepoName;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::tracker::Said;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::gh::Tracker;
use crate::git::{REMOTE, Repo};
use crate::host_db::{Host, Layout, Old, Store};
use crate::host_settings::Settings;
use crate::instance::Named;
use crate::orca::Worktree;

/// The checkout list in the host's database: each checkout a row of
/// `checkouts`, by its root, in the list's order; and what the list holds
/// beside them, the host settings and what another afkconvoy kept there,
/// the one row of `checkout_list`. It was `checkouts.json` in the cache
/// directory, with `checkouts.lock` beside it.
pub(crate) static STORE: Store = Store {
    file: "checkouts.json",
    old: Old::Meant(pinned),
    layout: Layout::Tables { export, import },
};

/// What an older release's list `listed` meant: there, an entry that says
/// no tracker worked its checkout's own repo, which this release's says
/// by naming it — no tracker said is the host's local tracker here. Each
/// such entry names the repo its checkout's remote names; one whose repo
/// cannot be read says none still, and is named in `UNPINNED` beside the
/// checkouts, which `doctor` finds (`findings`).
fn pinned(mut listed: Value) -> Value {
    let mut unpinned = vec![];
    for entry in listed["checkouts"].as_array_mut().into_iter().flatten() {
        if !entry["tracker"].is_null() {
            continue;
        }
        let Some(root) = entry["checkout"].as_str().map(PathBuf::from) else { continue };
        match Repo::at(&root).and_then(|repo| named_repo(&repo)) {
            Ok(own) => entry["tracker"] = Value::from(own.to_string()),
            Err(_) => unpinned.push(Value::from(root.display().to_string())),
        }
    }
    if !unpinned.is_empty()
        && let Some(listed) = listed.as_object_mut()
    {
        listed.insert(UNPINNED.into(), Value::Array(unpinned));
    }
    listed
}

/// Beside the checkouts: those an older release's list said no tracker of,
/// whose own repo could not be read as it was taken in (`pinned`).
const UNPINNED: &str = "tracker_unpinned";

/// What `doctor` finds of the checkouts whose tracker an older release's
/// list meant to be their own repo, which could not be read as it was taken
/// in: one finding each that still says none.
pub fn findings() -> Vec<crate::doctor::Finding> {
    let Ok(listed) = List::of_host().and_then(|list| list.read()) else { return vec![] };
    let unpinned = listed.rest.get(UNPINNED).and_then(Value::as_array).cloned().unwrap_or_default();
    let says_none = |root: &str| {
        listed
            .checkouts
            .iter()
            .any(|entry| entry.checkout == Path::new(root) && entry.tracker.is_none())
    };
    unpinned
        .iter()
        .filter_map(Value::as_str)
        .filter(|root| says_none(root))
        .map(|root| {
            crate::doctor::finding(
                crate::doctor::Kind::HostDatabase,
                serde_json::json!(["tracker-unpinned", root]),
                format!(
                    "the checkout {root} says no tracker since it was taken into the host database"
                ),
                "an older release worked its own repo's issues there, and its repo could not be \
                 read from its remote `origin` as the list was taken in: it now works the \
                 host's local tracker"
                    .to_string(),
                format!("afkconvoy run --tracker <owner>/<repo>   # in {root}"),
            )
        })
        .collect()
}

fn export(db: &rusqlite::Connection) -> Result<Option<Value>> {
    let beside = crate::host_db::rows(db, "checkout_list")?;
    let Some((_, mut listed)) = beside.into_iter().next() else { return Ok(None) };
    let checkouts = crate::host_db::rows(db, "checkouts")?;
    let checkouts = checkouts.into_iter().map(|(_, entry)| entry).collect();
    let object = listed.as_object_mut().context("the checkout list is no object")?;
    object.insert("checkouts".into(), Value::Array(checkouts));
    Ok(Some(listed))
}

fn import(db: &rusqlite::Connection, listed: &Value) -> Result<()> {
    let mut beside = listed.as_object().context("the checkout list is no object")?.clone();
    let checkouts = match beside.remove("checkouts") {
        Some(Value::Array(checkouts)) => checkouts,
        None => vec![],
        Some(_) => bail!("the checkout list's `checkouts` is no list"),
    };
    let root = |entry: &Value| entry["checkout"].as_str().unwrap_or_default().to_string();
    crate::host_db::put_rows(db, "checkouts", checkouts.iter().map(|entry| (root(entry), entry)))?;
    let beside = Value::Object(beside);
    crate::host_db::put_rows(db, "checkout_list", [(String::new(), &beside)])
}

/// What the checkout list holds: the checkouts, and the host settings
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
    /// The tracker its fleet reads and writes issues in, where that is not
    /// the checkout's own repo — another repo, or the host's local tracker:
    /// none, and it is its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<Said>,
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
    pub tracker: Option<Said>,
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

/// A host's checkout list: the database it is kept in.
#[derive(Clone)]
pub struct List {
    host: Host,
}

impl List {
    /// This host's: in its database (`host_db`).
    pub fn of_host() -> Result<List> {
        Ok(List { host: Host::of_host()? })
    }

    /// The list kept in the host database `file`.
    pub fn at(file: PathBuf) -> List {
        List { host: Host::at(file) }
    }

    /// The database file it is kept in.
    pub fn file(&self) -> &Path {
        self.host.file()
    }

    /// What it lists. None yet is an empty list; a file that cannot be read
    /// is an error naming it.
    pub fn read(&self) -> Result<Listed> {
        self.read_with_settings().map(|(listed, _)| listed)
    }

    /// The list, and whether it holds host settings, even defaults. A first
    /// `run` must distinguish settings never said from defaults already
    /// accepted.
    pub fn read_with_settings(&self) -> Result<(Listed, bool)> {
        let unread = || {
            format!(
                "the checkout list in {} cannot be read: mend it, or remove it and run \
                 `afkconvoy run` again in each checkout to be worked",
                self.file().display()
            )
        };
        let Some(value) = self.host.value(&STORE).with_context(unread)? else {
            return Ok((Listed::default(), false));
        };
        let settings = value.get("settings").is_some();
        Ok((serde_json::from_value(value).with_context(unread)?, settings))
    }

    /// Changes it in one transaction → what `change` answered: read,
    /// `change`d and written whole. A reader sees the old list or the new
    /// one, whole.
    pub fn change<T>(&self, change: impl FnOnce(&mut Listed) -> T) -> Result<T> {
        let unchanged =
            || format!("the checkout list in {} cannot be changed", self.file().display());
        self.host.change(&STORE, change).with_context(unchanged)
    }

    /// What it holds as its file held it, `None` where nothing: what a test
    /// reads, as another afkconvoy would.
    #[cfg(test)]
    pub(crate) fn kept(&self) -> Option<Value> {
        self.host.value(&STORE).unwrap()
    }

    /// Makes it hold `value`, as its file held it: what a test writes, as
    /// another afkconvoy would.
    #[cfg(test)]
    pub(crate) fn keep(&self, value: Value) {
        self.host.replace(&STORE, &value).unwrap()
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
/// says none, works the host's local tracker (`tracker_for`).
pub fn tracker_on(list: &List, repo: &Repo) -> Result<Tracker> {
    tracker_for(said_tracker(list, repo)?, repo)
}

/// The tracker of the checkout `repo` that says `said` of it on this host:
/// its host's local tracker where it says none (`Said::DEFAULT`), its own
/// repo where it names that. A checkout whose remote names no repo on GitHub
/// — a path — and says none works its own repo, as every checkout that said
/// none did before the local tracker was the default: the local tracker has
/// no repo name to file its issues under. Another repo's issues, or the local
/// tracker's, are the checkout's by the repo its remote names
/// (`named_repo`) — as a repo label in another repo, as a field in the local
/// tracker, which is named for this host (`host_settings::host_name`), as
/// the claims of its fleets are stamped.
pub fn tracker_for(said: Option<Said>, repo: &Repo) -> Result<Tracker> {
    match (said, named_repo(repo)) {
        (None, Err(_)) => Ok(Tracker::Own),
        (said, named) => {
            Ok(Said::or_default(said).tracker(&crate::host_settings::host_name(), named?))
        }
    }
}

/// The tracker the checkout `repo` says on the host of `list`, with its
/// entry or its main checkout's — where the list holds neither, what its
/// `run` asked in this process: none, and it is its own repo.
pub fn said_tracker(list: &List, repo: &Repo) -> Result<Option<Said>> {
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
pub fn said(list: &List, repo: &Repo) -> Result<Option<Option<Said>>> {
    let worktree = Worktree::at(repo.dir.clone(), String::new());
    let worker = worktree.holds_a_worker();
    if worker && let Some(kept) = worktree.tracker()? {
        // An empty note is the checkout's own repo, as an older fleet kept
        // it: by name where its remote names one — says none, where it does
        // not, which is its own repo there (`tracker_for`).
        let kept = match kept {
            Some(kept) => Some(kept),
            None => named_repo(repo).ok().map(Said::Repo),
        };
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

    /// The host database a test keeps its list in.
    const FILE: &str = crate::host_db::FILE;

    #[test]
    fn none_yet_is_an_empty_list_and_one_that_cannot_be_read_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join(FILE));
        assert!(list.read().unwrap().checkouts.is_empty());
        assert!(!list.read_with_settings().unwrap().1, "nothing read, nothing said");
        let unreadable = serde_json::json!({"checkouts": [{"checkout": "/a", "concurrency": 0}]});
        list.keep(unreadable.clone());
        let error = format!("{:#}", list.read().unwrap_err());
        assert!(error.contains(&list.file().display().to_string()), "{error}");
        assert!(list.change(|_| ()).is_err(), "a list that cannot be read is not written over");
        assert_eq!(list.kept(), Some(unreadable));
    }

    #[test]
    fn an_older_hosts_checkouts_json_is_taken_in_and_moved_aside_and_taken_in_once() {
        use crate::host_db::tests::{host, runner};
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let old = cache.join("checkouts.json");
        let list = List { host: host(dir.path()) };
        let older = runner(&cache, "0.0.1");
        std::fs::write(
            &old,
            r#"{"settings": {"admit_load": 400}, "kept": 1,
            "checkouts": [{"checkout": "/a"}]}"#,
        )
        .unwrap();
        let (listed, settings) = list.read_with_settings().unwrap();
        assert_eq!((listed.checkouts, settings), (vec![entry("/a")], true));
        assert_eq!(listed.settings.admit_load, Some(400));
        assert!(old.exists(), "the older host runner reads it still");
        // what that runner writes there is taken in again
        std::fs::write(&old, r#"{"checkouts": [{"checkout": "/a"}, {"checkout": "/b"}]}"#).unwrap();
        assert_eq!(list.read().unwrap().checkouts.len(), 2);
        drop(older);
        list.change(|listed| listed.checkouts.push(entry("/c"))).unwrap();
        assert!(!old.exists(), "moved aside once no older runner holds the host");
        let migrated = std::fs::read_to_string(cache.join("checkouts.json.migrated")).unwrap();
        assert!(migrated.contains("/b") && !migrated.contains("/c"), "{migrated}");
        assert_eq!(list.read().unwrap().checkouts, [entry("/a"), entry("/b"), entry("/c")]);
        assert_eq!(list.read().unwrap().checkouts.len(), 3, "a second read takes in nothing");
        // one that cannot be read is no list: an error naming it, and left
        std::fs::write(&old, "{\"checkouts\": [").unwrap();
        let error = format!("{:#}", list.read().unwrap_err());
        assert!(error.contains("checkouts.json cannot be read"), "{error}");
        assert!(old.exists(), "{error}");
    }

    #[test]
    fn an_older_hosts_entry_saying_no_tracker_is_taken_in_naming_its_own_repo() {
        use crate::host_db::tests::host;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let ran =
                crate::process::output(crate::census::command("git").args(args).current_dir(&root))
                    .unwrap();
            assert!(ran.status.success(), "{ran:?}");
        };
        git(&["init", "--quiet", "w"]);
        git(&["-C", "w", "remote", "add", "origin", "git@github.com:Acme/Dev.git"]);
        git(&["init", "--quiet", "nameless"]);
        let (own, nameless) = (root.join("w"), root.join("nameless"));
        let cache = root.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let old = serde_json::json!({"checkouts": [
            {"checkout": own}, {"checkout": "/elsewhere", "tracker": "acme/tracker"},
            {"checkout": nameless}]});
        std::fs::write(cache.join("checkouts.json"), old.to_string()).unwrap();
        let list = List { host: host(&root) };
        let listed = list.read().unwrap();
        let trackers: Vec<_> = listed.checkouts.iter().map(|entry| entry.tracker.clone()).collect();
        assert_eq!(
            trackers,
            [Said::read("acme/dev"), Said::read("acme/tracker"), None],
            "its own repo named; a tracker named as it was; one unread says none"
        );
        let unpinned = &list.kept().unwrap()[UNPINNED];
        assert_eq!(*unpinned, serde_json::json!([nameless]), "named for doctor");
        // taken in once: an entry said with no tracker from now on stays so
        list.change(|listed| listed.checkouts.push(entry("/new"))).unwrap();
        assert_eq!(list.read().unwrap().checkouts[3].tracker, None);
    }

    #[test]
    fn a_change_keeps_what_it_does_not_know_and_names_only_what_was_named() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join(FILE));
        let kept = r#"{"settings": {"no_host_sleep": true}, "kept_by_another": {"a": 1},
            "checkouts": [{"checkout": "/a"}]}"#;
        list.keep(serde_json::from_str(kept).unwrap());
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
        let text = list.kept().unwrap();
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
        // nothing is left beside it
        let left: Vec<_> =
            std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, [FILE], "the database alone: {left:?}");
    }

    /// A process's changes, as `two_processes_changing_the_list_at_once_each_keep_theirs`
    /// starts it: none where it is not started so.
    #[test]
    #[ignore = "a process of another test"]
    fn a_process_changing_the_list() {
        let Some(file) = std::env::var_os("AFKCONVOY_TEST_LIST") else { return };
        let list = List::at(PathBuf::from(file));
        let me = std::process::id();
        for m in 0..20 {
            let checkout = format!("/{me}/{m}");
            list.change(|listed| listed.checkouts.push(entry(&checkout))).unwrap();
            assert!(list.read().is_ok(), "a reader never sees half a list");
        }
    }

    #[test]
    fn two_processes_changing_the_list_at_once_each_keep_theirs() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let me = std::env::current_exe().unwrap();
        let name = "checkout_list::tests::a_process_changing_the_list";
        let children: Vec<_> = (0..4)
            .map(|_| {
                std::process::Command::new(&me)
                    .args([name, "--exact", "--ignored", "--test-threads=1"])
                    .env("AFKCONVOY_TEST_LIST", &file)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let listed = List::at(file).read().unwrap();
        assert_eq!(listed.checkouts.len(), 80, "each process's twenty");
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
                crate::process::output(crate::census::command("git").args(args).current_dir(&root))
                    .unwrap();
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

        // none on the list, or none said: the host's local tracker
        let local = Said::Local
            .tracker(&crate::host_settings::host_name(), RepoName::read("acme/dev").unwrap());
        assert_eq!(tracker_on(&list, &checkout).unwrap(), local);
        list.change(|listed| listed.checkouts.push(entry(&checkout.dir.display().to_string())))
            .unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), local);
        // its own repo, named in whatever case: its own
        let own = Said::read("Acme/Dev");
        list.change(|listed| listed.checkouts[0].tracker = own).unwrap();
        assert_eq!(tracker_on(&list, &checkout).unwrap(), Tracker::Own);
        assert_eq!(tracker_on(&list, &worktree).unwrap(), Tracker::Own);

        let tracker = Said::read("acme/tracker");
        list.change(|listed| listed.checkouts[0].tracker = tracker).unwrap();
        assert_eq!(tracker_on(&list, &checkout).unwrap(), other);
        assert_eq!(
            tracker_on(&list, &worktree).unwrap(),
            other,
            "a worktree goes by its checkout's"
        );
        let text = list.kept().unwrap();
        assert_eq!(text["checkouts"][0]["tracker"], "acme/tracker", "{text}");

        // the host's local tracker, named for the host
        list.change(|listed| listed.checkouts[0].tracker = Some(Said::Local)).unwrap();
        let local = Tracker::Local {
            tracker: afkconvoy_core::tracker::local_name(&crate::host_settings::host_name()),
            repo: RepoName::read("acme/dev").unwrap(),
        };
        assert_eq!(tracker_on(&list, &worktree).unwrap(), local);
        let text = list.kept().unwrap();
        assert_eq!(text["checkouts"][0]["tracker"], "local", "{text}");

        // its own repo named — as an older release's list is taken in — is its own
        list.change(|listed| listed.checkouts[0].tracker = Said::read("acme/dev")).unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), Tracker::Own);

        // a tracker that is no repo's name nor `local`: the list cannot be read
        list.keep(serde_json::json!({"checkouts": [{"checkout": "/a", "tracker": "acme"}]}));
        let error = format!("{:#}", list.read().unwrap_err());
        assert!(error.contains("`acme` is no tracker"), "{error}");
    }

    #[test]
    fn a_workers_worktree_goes_by_the_tracker_its_fleet_kept_there() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let ran =
                crate::process::output(crate::census::command("git").args(args).current_dir(&root))
                    .unwrap();
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
        let local = Said::Local
            .tracker(&crate::host_settings::host_name(), RepoName::read("acme/dev").unwrap());
        assert_eq!(tracker_on(&list, &checkout).unwrap(), local, "one saying none works the local");

        // what its fleet kept there holds while the list holds no entry
        held.keep_tracker(&other).unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), other);
        assert_eq!(tracker_on(&list, &checkout).unwrap(), local, "kept in the worktree");
        held.keep_tracker(&Tracker::Own).unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), Tracker::Own);
        let kept = std::fs::read_to_string(held.note(crate::orca::TRACKER_FILE).unwrap()).unwrap();
        assert_eq!(kept, "acme/dev", "its own repo is kept named");
        // an empty note, as an older fleet kept its own repo, is its own repo
        std::fs::write(held.note(crate::orca::TRACKER_FILE).unwrap(), "").unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), Tracker::Own);

        // a worker's worktree that kept none goes by its checkout's entry
        std::fs::remove_file(held.note(crate::orca::TRACKER_FILE).unwrap()).unwrap();
        let tracker = Said::read("acme/tracker");
        list.change(|listed| {
            let mut kept = entry(&checkout.dir.display().to_string());
            kept.tracker = tracker;
            listed.checkouts.push(kept);
        })
        .unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), other);

        // a kept local tracker is the host's
        held.keep_tracker(&local).unwrap();
        assert_eq!(tracker_on(&list, &worktree).unwrap(), local);

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
            let ran = crate::process::output(
                crate::census::command("git").args(args).current_dir(dir.path()),
            )
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

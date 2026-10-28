//! git, as a subprocess: the checkout a command runs in and its one remote.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use afkconvoy_core::layout::{Layout, Note};
use anyhow::{Context, Result, bail};

/// The remote every ref and branch is pushed to and fetched from.
pub const REMOTE: &str = "origin";

/// The note of the ref layout a start settled on (`Repo::note`).
const LAYOUT_NOTE: &str = "convoy-layout";

/// What one git call came to. Never an error by itself: the caller reads `ok`.
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// A git checkout: the fleet's own, or a worker's worktree.
#[derive(Debug, Clone)]
pub struct Repo {
    pub dir: PathBuf,
    /// What git said of this checkout that holds for as long as it is there.
    /// Copies of this value share those facts. Remote reads are shared by
    /// every checkout of the same git common dir, because a worktree's push
    /// must drop the ref the parent checkout cached (`turn::certify` opens
    /// the worktree with `Repo::new`).
    fixed: Arc<Fixed>,
}

#[derive(Debug, Default)]
struct Fixed {
    /// The repo's git dir (`common_dir`).
    common: OnceLock<PathBuf>,
    /// The empty tree's id in the repo's object format (`record_commit`).
    empty_tree: OnceLock<String>,
    /// Remote reads of this checkout's repository, resolved on the first one.
    reads: OnceLock<Arc<RemoteReads>>,
}

/// Remote refs and commit subjects this process has already read of one
/// repository. Every worktree of that repository shares one, so a push from
/// any of them drops the ref the others cached.
#[derive(Debug, Default)]
struct RemoteReads {
    /// Remote refs already read. A push of a ref, or a raw `git ls-remote`
    /// outside `listed`, drops what a later read must see again.
    seen: Mutex<Seen>,
    /// A commit's subject, by sha. The sha names the subject, so it stays.
    subjects: Mutex<HashMap<String, String>>,
    /// Shas `bring` has already found present. Objects live in the common store.
    present: Mutex<HashSet<String>>,
}

/// One process's remote reads. An exact ref already covered by a listing is
/// not asked again. Pushing the ref drops it, so the next read sees the
/// remote — including a peer that won the push. A raw `ls-remote` drops the
/// refs it named, because that call saw them and this map did not.
#[derive(Debug, Default)]
struct Seen {
    /// `Some` is the sha the remote had; `None` is a read that found it absent.
    sha: HashMap<String, Option<String>>,
    /// Glob prefixes (`refs/heads/`) for which every ref is in `sha` or absent.
    covered: Vec<String>,
    /// Pushed by this process since the read that filled `sha`.
    dirty: HashSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum SeenAnswer {
    Miss,
    Absent,
    Sha(String),
}

impl Seen {
    fn answer(&self, name: &str) -> SeenAnswer {
        if self.dirty.contains(name) {
            return SeenAnswer::Miss;
        }
        if let Some(found) = self.sha.get(name) {
            return match found {
                Some(sha) => SeenAnswer::Sha(sha.clone()),
                None => SeenAnswer::Absent,
            };
        }
        if self.covered.iter().any(|prefix| name.starts_with(prefix)) {
            return SeenAnswer::Absent;
        }
        SeenAnswer::Miss
    }

    fn clear(&mut self) {
        self.sha.clear();
        self.covered.clear();
        self.dirty.clear();
    }

    /// The next read of `name` asks the remote. Other refs stay.
    fn dirty(&mut self, name: &str) {
        self.sha.remove(name);
        self.dirty.insert(name.to_string());
    }

    /// `records` is what `patterns` just listed. A glob prefix becomes complete:
    /// a name under it that was not listed is absent.
    fn remember(&mut self, records: &[RemoteRecord], patterns: &[&str]) {
        if patterns.iter().any(|pattern| glob_prefix(pattern).is_none() && !is_exact(pattern)) {
            self.clear();
            return;
        }
        for pattern in patterns {
            if let Some(prefix) = glob_prefix(pattern) {
                self.sha.retain(|name, _| !name.starts_with(&prefix));
                self.dirty.retain(|name| !name.starts_with(&prefix));
                if !self.covered.iter().any(|had| had == &prefix) {
                    self.covered.push(prefix);
                }
            } else {
                self.sha.remove(*pattern);
                self.dirty.remove(*pattern);
            }
        }
        for record in records {
            self.sha.insert(record.name.clone(), Some(record.sha.clone()));
            self.dirty.remove(&record.name);
        }
        for pattern in patterns {
            if is_exact(pattern) && !records.iter().any(|record| record.name == *pattern) {
                self.sha.insert((*pattern).to_string(), None);
            }
        }
    }

    /// Drops the refs a raw `ls-remote` just read, so the next read asks again.
    /// A pattern this cache cannot name drops every ref.
    fn forget(&mut self, patterns: &[String]) {
        for pattern in patterns {
            if let Some(prefix) = glob_prefix(pattern) {
                self.sha.retain(|name, _| !name.starts_with(&prefix));
                self.dirty.retain(|name| !name.starts_with(&prefix));
                self.covered.retain(|had| had != &prefix);
            } else if is_exact(pattern) {
                self.dirty(pattern);
            } else {
                self.clear();
                return;
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The reads of the repository `key` names. `key` is its absolute git common
/// dir, so every worktree of it in this process is one entry.
fn shared_reads(key: PathBuf) -> Arc<RemoteReads> {
    static REPOS: LazyLock<Mutex<HashMap<PathBuf, Arc<RemoteReads>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    lock(&REPOS).entry(key).or_default().clone()
}

/// The prefix a trailing-star pattern covers (`refs/heads/*` → `refs/heads/`).
/// A pattern with any other glob is not a prefix.
fn glob_prefix(pattern: &str) -> Option<String> {
    let prefix = pattern.strip_suffix('*')?;
    if prefix.is_empty() || prefix.contains('*') || prefix.contains('?') {
        return None;
    }
    Some(prefix.to_string())
}

fn is_exact(pattern: &str) -> bool {
    !pattern.contains('*') && !pattern.contains('?')
}

/// The refs a `git push` writes. `None` when a destination is not a named ref
/// (`HEAD`, a tag, a branch other than `main`, `--all`): the caller drops
/// every cached ref, because this cannot say which one changed.
fn refs_pushed(args: &[&str]) -> Option<Vec<String>> {
    let mut refs = Vec::new();
    let mut saw_remote = false;
    let mut i = 1;
    while i < args.len() {
        let arg = args[i];
        if arg == "--all" || arg == "--mirror" || arg == "--tags" {
            return None;
        }
        if arg == "--delete" {
            i += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--force-with-lease=") {
            refs.push(lease_name(value)?);
            i += 1;
            continue;
        }
        if arg == "--force-with-lease" {
            if let Some(value) = args.get(i + 1).copied().filter(|value| !value.starts_with('-')) {
                refs.push(lease_name(value)?);
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        if !saw_remote {
            saw_remote = true;
            i += 1;
            continue;
        }
        refs.push(pushed_ref(arg)?);
        i += 1;
    }
    if refs.is_empty() { None } else { Some(refs) }
}

fn lease_name(value: &str) -> Option<String> {
    let name = value.split_once(':').map(|(name, _)| name).unwrap_or(value);
    name.starts_with("refs/").then(|| name.to_string())
}

fn pushed_ref(arg: &str) -> Option<String> {
    if let Some((_, dst)) = arg.split_once(':') {
        return dst.starts_with("refs/").then(|| dst.to_string());
    }
    if arg.starts_with("refs/") {
        return Some(arg.to_string());
    }
    if arg == "main" {
        return Some("refs/heads/main".to_string());
    }
    None
}

/// The patterns a raw `git ls-remote` asked for. `None` when it listed a class
/// this cache does not name (`--heads`, no pattern): the caller drops every ref.
fn refs_asked(args: &[&str]) -> Option<Vec<String>> {
    let mut patterns = Vec::new();
    let mut saw_remote = false;
    for arg in args.iter().skip(1) {
        if *arg == "--heads" || *arg == "--tags" || *arg == "--refs" {
            return None;
        }
        if arg.starts_with('-') {
            continue;
        }
        if !saw_remote {
            saw_remote = true;
            continue;
        }
        patterns.push((*arg).to_string());
    }
    if patterns.is_empty() { None } else { Some(patterns) }
}

impl Repo {
    /// The checkout that contains `dir`, by its top directory: a directory
    /// anywhere in a checkout names that checkout.
    pub fn at(dir: &Path) -> Result<Repo> {
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }
        let ran = run(dir, &["rev-parse", "--show-toplevel"], &[])?;
        if !ran.ok {
            bail!("{} is not in a git checkout", dir.display());
        }
        Ok(Repo::new(PathBuf::from(ran.stdout)))
    }

    /// The checkout whose top directory is `dir`.
    pub fn new(dir: PathBuf) -> Repo {
        Repo { dir, fixed: Arc::default() }
    }

    /// The repo's git dir, which every worktree of it shares. git is asked
    /// once: a checkout's repo is the same one for as long as it is there.
    pub fn common_dir(&self) -> Result<&Path> {
        if self.fixed.common.get().is_none() {
            self.resolve_common()?;
        }
        Ok(self.fixed.common.get().expect("set above"))
    }

    /// The absolute git common dir. Asked through `run`, not `git`: `git`
    /// notes a remote read, and resolving the reads would call this again.
    fn resolve_common(&self) -> Result<PathBuf> {
        if let Some(common) = self.fixed.common.get() {
            return Ok(common.clone());
        }
        let args = ["rev-parse", "--path-format=absolute", "--git-common-dir"];
        let ran = run(&self.dir, &args, &[])?;
        if !ran.ok || ran.stdout.is_empty() {
            bail!("git {} failed: {}", args.join(" "), ran.stderr.trim());
        }
        let dir = PathBuf::from(&ran.stdout);
        let _ = self.fixed.common.set(dir);
        Ok(self.fixed.common.get().expect("set above").clone())
    }

    /// This checkout's remote reads, shared with every worktree of its
    /// repository. A checkout whose common dir cannot be read keeps its own.
    fn reads(&self) -> Arc<RemoteReads> {
        if let Some(reads) = self.fixed.reads.get() {
            return Arc::clone(reads);
        }
        let key = self.resolve_common().unwrap_or_else(|_| self.dir.clone());
        let reads = shared_reads(key);
        let _ = self.fixed.reads.set(Arc::clone(&reads));
        Arc::clone(self.fixed.reads.get().unwrap_or(&reads))
    }

    /// The main worktree of this checkout: where its `.git` directory is.
    pub fn main_checkout(&self) -> Result<PathBuf> {
        let common = self.common_dir()?;
        Ok(common.parent().unwrap_or(common).to_path_buf())
    }

    /// Where the last start found the remote keeps the fleet's refs
    /// (`probe::startup`): a note in the repo's git dir, shared by every
    /// worktree, so a worker's command names the refs the fleet does. A repo
    /// no start has probed uses the first layout.
    pub fn layout(&self) -> Layout {
        self.noted().map(|note| note.layout).unwrap_or(Layout::ALL[0])
    }

    /// The note of the ref layout, including how many probes in a row have
    /// seen it rejected; `None` when no start has probed, or the note is not one.
    pub fn noted(&self) -> Option<Note> {
        let text = std::fs::read_to_string(self.note(LAYOUT_NOTE).ok()?).ok()?;
        Note::read(&text)
    }

    /// Notes the layout a start settled on, and the streak against it.
    pub fn note_settled(&self, layout: Layout, streak: u32) -> Result<()> {
        let next = Note { layout, streak };
        if self.noted() == Some(next) {
            return Ok(());
        }
        let note = self.note(LAYOUT_NOTE)?;
        std::fs::write(&note, next.write())
            .with_context(|| format!("cannot write {}", note.display()))?;
        Ok(())
    }

    /// Notes the layout a start settled on, keeping a streak already counted
    /// against that same layout.
    pub fn note_layout(&self, layout: Layout) -> Result<()> {
        let streak = self.noted().filter(|note| note.layout == layout).map(|note| note.streak);
        self.note_settled(layout, streak.unwrap_or(0))
    }

    /// Where a note called `name` is kept on this host: in the repo's git
    /// dir, which every worktree of it shares.
    pub fn note(&self, name: &str) -> Result<PathBuf> {
        Ok(self.common_dir()?.join(name))
    }

    /// One git call whose failure the caller reads.
    pub fn try_git(&self, args: &[&str]) -> Result<Ran> {
        let ran = run(&self.dir, args, &[])?;
        self.note_remote(args);
        Ok(ran)
    }

    /// A push drops the ref it wrote. A raw `ls-remote` drops the refs it
    /// named: the caller read them itself, and the saved reads would be older.
    /// A call whose refs this cache cannot name drops every saved ref.
    fn note_remote(&self, args: &[&str]) {
        let Some(cmd) = args.first().copied() else { return };
        if cmd != "ls-remote" && cmd != "push" {
            return;
        }
        // Resolve the shared reads before taking `seen`. Resolving runs git,
        // and that must not happen while `seen` is held.
        let reads = self.reads();
        let mut seen = lock(&reads.seen);
        if cmd == "ls-remote" {
            match refs_asked(args) {
                None => seen.clear(),
                Some(patterns) => seen.forget(&patterns),
            }
        } else if cmd == "push" {
            match refs_pushed(args) {
                None => seen.clear(),
                Some(refs) => {
                    for name in refs {
                        seen.dirty(&name);
                    }
                }
            }
        }
    }

    /// One git call under extra environment, whose failure the caller reads.
    pub fn try_git_with(&self, args: &[&str], env: &[(&str, &str)]) -> Result<Ran> {
        let ran = run(&self.dir, args, env)?;
        self.note_remote(args);
        Ok(ran)
    }

    /// Is `commit` on `tip`'s history (or `tip` itself)?
    pub fn within(&self, commit: &str, tip: &str) -> Result<bool> {
        Ok(self.try_git(&["merge-base", "--is-ancestor", commit, tip])?.ok)
    }

    /// The sha the remote has at one ref, with its commits brought into this
    /// repo; `None` when the remote has no such ref. Like every read of the
    /// remote it writes no ref here and no FETCH_HEAD.
    pub fn fetched(&self, refname: &str) -> Result<Option<String>> {
        let Some(sha) = self.remote_sha(refname)? else { return Ok(None) };
        self.bring(&sha, refname)?;
        Ok(Some(sha))
    }

    /// Brings into this repo the commit `sha` the remote was seen to have at
    /// `refname`, when it is not here yet. An error when the remote no longer
    /// has it there.
    pub fn bring(&self, sha: &str, refname: &str) -> Result<()> {
        if !self.object_here(sha)? {
            self.git(&["fetch", "--quiet", "--no-write-fetch-head", "--no-tags", REMOTE, refname])?;
            if !self.object_here(sha)? {
                bail!("{refname} moved on the remote while it was read — run this again");
            }
        }
        Ok(())
    }

    /// Whether `sha` is a commit already in this repo. A sha that is here stays
    /// here, so a later bring does not ask again.
    fn object_here(&self, sha: &str) -> Result<bool> {
        let reads = self.reads();
        if lock(&reads.present).contains(sha) {
            return Ok(true);
        }
        let ok = self.try_git(&["cat-file", "-e", &format!("{sha}^{{commit}}")])?.ok;
        if ok {
            lock(&reads.present).insert(sha.to_string());
        }
        Ok(ok)
    }

    /// One git call that must succeed → its stdout as it came: for output whose
    /// leading spaces mean something.
    pub fn git_raw(&self, args: &[&str]) -> Result<String> {
        let out = crate::process::output(
            Command::new("git").current_dir(&self.dir).args(args).env("LC_ALL", "C"),
        )
        .context("git could not run")?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// One git call that must succeed → its stdout, trimmed.
    pub fn git(&self, args: &[&str]) -> Result<String> {
        let ran = self.try_git(args)?;
        if !ran.ok {
            bail!("git {} failed: {}", args.join(" "), ran.stderr.trim());
        }
        Ok(ran.stdout)
    }

    /// A parentless commit of the empty tree whose subject is `record` → its
    /// sha, which is what gets pushed to a ref. It drags no history along, and
    /// is written under the fleet's identity, whoever's machine this is.
    pub fn record_commit(&self, record: &str) -> Result<String> {
        if self.fixed.empty_tree.get().is_none() {
            let tree = self.git(&["hash-object", "-t", "tree", "/dev/null"])?;
            let _ = self.fixed.empty_tree.set(tree);
        }
        self.record_commit_of(self.fixed.empty_tree.get().expect("set above"), record)
    }

    /// A parentless commit of `tree` whose subject is `record`: a record that
    /// is about a tree, and keeps it.
    pub fn record_commit_of(&self, tree: &str, record: &str) -> Result<String> {
        let ran = run(&self.dir, &["commit-tree", tree, "-m", record], &RECORD_IDENTITY)?;
        if !ran.ok {
            bail!("git commit-tree failed: {}", ran.stderr.trim());
        }
        Ok(ran.stdout)
    }

    /// The sha the remote has for one ref, `None` if it has none. A ref this
    /// process already listed is not asked again until this process pushes it
    /// or a raw `ls-remote` reads it. An error when the remote cannot be read.
    pub fn remote_sha(&self, refname: &str) -> Result<Option<String>> {
        let found = self.listed(&[refname])?.into_iter().find(|listed| listed.name == refname);
        Ok(found.map(|listed| listed.sha))
    }

    /// The sha the remote has for one ref now, even when this process listed
    /// it. A check that another process can move the ref — a source branch
    /// while a candidate is validating — uses this. The saved listing would
    /// still say what it said when this process read it.
    pub(crate) fn remote_sha_now(&self, refname: &str) -> Result<Option<String>> {
        self.note_remote(&["ls-remote", REMOTE, refname]);
        self.remote_sha(refname)
    }

    /// Every ref the remote has that matches one of `patterns` — a ref's
    /// name, or `<namespace>/*` — in one call, with no record read yet. An
    /// exact ref already listed is answered from that listing. An error when
    /// the remote cannot be read.
    fn listed(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        if !patterns.is_empty() && patterns.iter().copied().all(is_exact) {
            let reads = self.reads();
            let seen = lock(&reads.seen);
            let mut records = Vec::new();
            for pattern in patterns {
                match seen.answer(pattern) {
                    SeenAnswer::Miss => {
                        drop(seen);
                        return self.ask(patterns);
                    }
                    SeenAnswer::Absent => {}
                    SeenAnswer::Sha(sha) => records.push(RemoteRecord {
                        name: (*pattern).to_string(),
                        sha,
                        subject: String::new(),
                    }),
                }
            }
            return Ok(records);
        }
        self.ask(patterns)
    }

    /// Lists `patterns` and keeps the answer. Goes through `run`, not `git`:
    /// `git` would drop the answer as a raw `ls-remote`.
    fn ask(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        let mut args = Vec::with_capacity(patterns.len() + 2);
        args.push("ls-remote");
        args.push(REMOTE);
        args.extend_from_slice(patterns);
        let ran = run(&self.dir, &args, &[])?;
        if !ran.ok {
            bail!("git {} failed: {}", args.join(" "), ran.stderr.trim());
        }
        let records = ran
            .stdout
            .lines()
            .filter_map(|row| row.split_once('\t'))
            .map(|(sha, name)| RemoteRecord {
                name: name.to_string(),
                sha: sha.to_string(),
                subject: String::new(),
            })
            .collect::<Vec<_>>();
        lock(&self.reads().seen).remember(&records, patterns);
        Ok(records)
    }

    /// Every branch tip, including `base_branch`, every version tag and every ref
    /// the fleet keeps, as ONE listing of the remote saw them: what a rebuild
    /// reads of the remote, all of it from one moment.
    pub fn listing(&self, base_branch: &str) -> Result<Listing> {
        let (every, base_ref) = (self.layout().every(""), format!("refs/heads/{base_branch}"));
        let mut records = self.listed(&[&every, "refs/heads/*", TAGS])?;
        let base = records.iter().find(|record| record.name == base_ref);
        let base = base.map(|record| record.sha.clone());
        let tags = tags_of(&records);
        let heads = records
            .iter()
            .filter(|r| r.name.starts_with("refs/heads/"))
            .map(|r| (r.name.clone(), r.sha.clone()))
            .collect();
        records.retain(|record| record.name.starts_with(every.trim_end_matches('*')));
        self.read_records(&mut records)?;
        Ok(Listing { base_ref, base, records, tags, heads })
    }

    /// Every version tag the remote has, each with the commit it is on.
    pub fn remote_tags(&self) -> Result<Vec<(String, String)>> {
        Ok(tags_of(&self.listed(&[TAGS])?))
    }

    /// Every branch the remote has, by its short name. An error when the
    /// remote cannot be read.
    pub fn remote_heads(&self) -> Result<Vec<String>> {
        Ok(self
            .listed(&["refs/heads/*"])?
            .into_iter()
            .filter_map(|record| record.name.strip_prefix("refs/heads/").map(str::to_string))
            .collect())
    }

    /// Every ref the remote has that matches `pattern` (one ref's name, or
    /// `<namespace>/*`), each with the subject of the commit it points at. An
    /// error when the remote cannot be read.
    ///
    /// Nothing is written to this checkout's refs or its FETCH_HEAD: worktrees
    /// share both, and a worker's command may be reading the remote meanwhile.
    pub fn remote_records(&self, pattern: &str) -> Result<Vec<RemoteRecord>> {
        self.remote_records_in(&[pattern])
    }

    /// `remote_records` for several patterns, in one listing of the remote.
    pub fn remote_records_in(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        let mut records = self.listed_refs(patterns)?;
        self.read_record_subjects(&mut records)?;
        Ok(records)
    }

    /// The refs named by `patterns`, in one read of the remote, with no record
    /// read yet. A ref the remote does not have is absent, not an error.
    pub(crate) fn listed_refs(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        self.listed(patterns)
    }

    /// The subject of each of `records`, fetched by ref name when its commit
    /// is not here yet. Nothing is fetched when `records` is empty.
    pub(crate) fn read_record_subjects(&self, records: &mut [RemoteRecord]) -> Result<()> {
        self.read_records(records)
    }

    /// Reads the record each of `records` carries: the subject of its commit,
    /// fetched by the name of each ref when one is not here yet — by name,
    /// for git takes a pattern only with a ref of this repo to write.
    fn read_records(&self, records: &mut [RemoteRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        // The commits are usually here already: fetched by an earlier read.
        let subjects = match self.subjects(records)? {
            Some(subjects) => subjects,
            None => {
                let mut fetch =
                    vec!["fetch", "--quiet", "--no-write-fetch-head", "--no-tags", REMOTE];
                fetch.extend(records.iter().map(|record| record.name.as_str()));
                self.git(&fetch)?;
                self.subjects(records)?.unwrap_or_default()
            }
        };
        for record in records {
            // A ref that moved between the listing and the fetch reads as a
            // record nobody can read; the next read sees it where it is.
            record.subject = subjects.get(&record.sha).cloned().unwrap_or_default();
        }
        Ok(())
    }

    /// The subject of each record's commit by sha; `None` when a commit is not
    /// in this repo yet. A sha already read is not asked again: the sha names
    /// the subject.
    fn subjects(&self, records: &[RemoteRecord]) -> Result<Option<HashMap<String, String>>> {
        let mut missing = Vec::new();
        let reads = self.reads();
        {
            let known = lock(&reads.subjects);
            for record in records {
                if !known.contains_key(&record.sha) && !missing.contains(&record.sha.as_str()) {
                    missing.push(record.sha.as_str());
                }
            }
            if missing.is_empty() {
                return Ok(Some(subjects_of(&known, records)));
            }
        }
        let mut args = vec!["log", "--no-walk=unsorted", "--format=%H%x09%s"];
        args.extend(missing);
        let ran = self.try_git(&args)?;
        if !ran.ok {
            return Ok(None);
        }
        let found: HashMap<String, String> = ran
            .stdout
            .lines()
            .filter_map(|row| row.split_once('\t'))
            .map(|(sha, subject)| (sha.to_string(), subject.to_string()))
            .collect();
        let mut known = lock(&reads.subjects);
        for (sha, subject) in &found {
            known.entry(sha.clone()).or_insert_with(|| subject.clone());
        }
        Ok(Some(subjects_of(&known, records)))
    }
}

fn subjects_of(
    known: &HashMap<String, String>,
    records: &[RemoteRecord],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for record in records {
        if let Some(subject) = known.get(&record.sha) {
            out.insert(record.sha.clone(), subject.clone());
        }
    }
    out
}

/// One ref of the remote and the record its commit carries.
#[derive(Debug, Clone)]
pub struct RemoteRecord {
    pub name: String,
    pub sha: String,
    pub subject: String,
}

/// Every branch tip and ref of the fleet, as one listing of the
/// remote saw them (`Repo::listing`).
#[derive(Debug, Clone)]
pub struct Listing {
    /// Branch identities and tips, from the same observation as fleet state.
    pub heads: Vec<(String, String)>,
    /// The base branch's ref.
    pub base_ref: String,
    /// Its tip; `None` when the remote has no such branch.
    pub base: Option<String>,
    /// Every ref of the fleet, with the record its commit carries.
    pub records: Vec<RemoteRecord>,
    /// Every tag whose name starts `v` — a version tag, where it names one
    /// (`cut::Version::tagged`) — each with the commit it is on.
    pub tags: Vec<(String, String)>,
}

impl Listing {
    /// The sha a fleet ref or branch had in this listing, `None` if absent.
    pub fn sha(&self, refname: &str) -> Option<&str> {
        let found = self.records.iter().find(|record| record.name == refname);
        found.map(|record| record.sha.as_str()).or_else(|| {
            self.heads.iter().find(|(name, _)| name == refname).map(|(_, sha)| sha.as_str())
        })
    }
}

/// What a listing of the remote's version tags asks for: every tag whose
/// name starts `v`.
const TAGS: &str = "refs/tags/v*";

/// The tags among `listed`, each by its name with the commit it is on: an
/// annotated tag's is listed once more, peeled (`<tag>^{}`), and that one is
/// taken.
fn tags_of(listed: &[RemoteRecord]) -> Vec<(String, String)> {
    let tag = |name: &str| name.strip_prefix("refs/tags/").map(str::to_string);
    let mut tags: Vec<(String, String)> = listed
        .iter()
        .filter(|listed| !listed.name.ends_with("^{}"))
        .filter_map(|listed| Some((tag(&listed.name)?, listed.sha.clone())))
        .collect();
    for (name, commit) in &mut tags {
        let peeled = format!("refs/tags/{name}^{{}}");
        if let Some(peeled) = listed.iter().find(|listed| listed.name == peeled) {
            commit.clone_from(&peeled.sha);
        }
    }
    tags
}

const RECORD_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "afkconvoy"),
    ("GIT_AUTHOR_EMAIL", "afkconvoy@localhost"),
    ("GIT_COMMITTER_NAME", "afkconvoy"),
    ("GIT_COMMITTER_EMAIL", "afkconvoy@localhost"),
];

/// git's messages are read — a ref the remote lacks, a push the server turned
/// down — so they are asked for in one language whatever the machine's locale.
fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Ran> {
    let out = crate::process::output(
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("LC_ALL", "C")
            .envs(env.iter().copied()),
    )
    .context("git could not run")?;
    Ok(Ran {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_annotated_tag_is_on_the_commit_it_peels_to() {
        let listed = |name: &str, sha: &str| RemoteRecord {
            name: name.to_string(),
            sha: sha.to_string(),
            subject: String::new(),
        };
        let tags = tags_of(&[
            listed("refs/tags/v0.5.15", "c15"),
            listed("refs/tags/v0.5.16", "t16"),
            listed("refs/tags/v0.5.16^{}", "c16"),
            listed("refs/heads/main", "m"),
        ]);
        let named = |name: &str, commit: &str| (name.to_string(), commit.to_string());
        assert_eq!(tags, [named("v0.5.15", "c15"), named("v0.5.16", "c16")]);
    }

    fn record(name: &str, sha: &str) -> RemoteRecord {
        RemoteRecord { name: name.to_string(), sha: sha.to_string(), subject: String::new() }
    }

    #[test]
    fn a_listing_answers_an_absent_ref_under_its_prefix() {
        let mut seen = Seen::default();
        seen.remember(&[record("refs/heads/main", "abc")], &["refs/heads/*"]);
        assert_eq!(seen.answer("refs/heads/main"), SeenAnswer::Sha("abc".into()));
        assert_eq!(seen.answer("refs/heads/other"), SeenAnswer::Absent);
        assert_eq!(seen.answer("refs/convoy/target/7"), SeenAnswer::Miss);
    }

    #[test]
    fn a_push_drops_only_the_ref_it_wrote() {
        let mut seen = Seen::default();
        seen.remember(
            &[record("refs/heads/main", "abc"), record("refs/heads/other", "def")],
            &["refs/heads/*"],
        );
        seen.dirty("refs/heads/main");
        assert_eq!(seen.answer("refs/heads/main"), SeenAnswer::Miss);
        assert_eq!(seen.answer("refs/heads/other"), SeenAnswer::Sha("def".into()));
        assert_eq!(seen.answer("refs/heads/gone"), SeenAnswer::Absent);
    }

    #[test]
    fn an_exact_miss_stays_absent_until_the_ref_is_listed() {
        let mut seen = Seen::default();
        seen.remember(&[], &["refs/convoy/target/7"]);
        assert_eq!(seen.answer("refs/convoy/target/7"), SeenAnswer::Absent);
        seen.remember(&[record("refs/convoy/target/7", "abc")], &["refs/convoy/target/7"]);
        assert_eq!(seen.answer("refs/convoy/target/7"), SeenAnswer::Sha("abc".into()));
    }

    #[test]
    fn a_raw_read_drops_only_the_refs_it_named() {
        let mut seen = Seen::default();
        seen.remember(
            &[record("refs/heads/main", "abc"), record("refs/convoy/target/7", "def")],
            &["refs/heads/*", "refs/convoy/*"],
        );
        seen.forget(&["refs/convoy/candidates/main".into(), "refs/heads/main".into()]);
        assert_eq!(seen.answer("refs/heads/main"), SeenAnswer::Miss);
        assert_eq!(seen.answer("refs/heads/other"), SeenAnswer::Absent);
        assert_eq!(seen.answer("refs/convoy/target/7"), SeenAnswer::Sha("def".into()));
    }

    #[test]
    fn a_push_names_the_ref_it_writes() {
        let names = |args: &[&str]| refs_pushed(args);
        assert_eq!(names(&["push", "-q", "origin", "main"]), Some(vec!["refs/heads/main".into()]));
        assert_eq!(
            names(&["push", "--quiet", "origin", "abc:refs/convoy/target/7"]),
            Some(vec!["refs/convoy/target/7".into()])
        );
        assert_eq!(
            names(&["push", "--quiet", "origin", "--delete", "refs/convoy/gate/x"]),
            Some(vec!["refs/convoy/gate/x".into()])
        );
        assert_eq!(
            names(&[
                "push",
                "--quiet",
                "origin",
                "--force-with-lease=refs/convoy/claim/7:abc",
                ":refs/convoy/claim/7",
            ]),
            Some(vec!["refs/convoy/claim/7".into(), "refs/convoy/claim/7".into()])
        );
        assert_eq!(names(&["push", "-q", "origin", "HEAD"]), None);
        assert_eq!(names(&["push", "-q", "origin", "v0.1.0"]), None);
        assert_eq!(names(&["push", "--force-with-lease=main:abc", "origin", "main"]), None);
        assert_eq!(
            names(&["push", "--quiet", "--force", "origin", "abc:refs/convoy/heartbeat/alpha"]),
            Some(vec!["refs/convoy/heartbeat/alpha".into()])
        );
        assert_eq!(
            names(&[
                "push",
                "--quiet",
                "--force-with-lease=refs/convoy/train/line/main:old",
                "origin",
                ":refs/convoy/train/line/main",
            ]),
            Some(vec!["refs/convoy/train/line/main".into(), "refs/convoy/train/line/main".into()])
        );
        assert_eq!(
            names(&["push", "--quiet", "origin", "HEAD:refs/heads/cut"]),
            Some(vec!["refs/heads/cut".into()])
        );
        assert_eq!(
            names(&["push", "--quiet", "origin", "refs/tags/v0.1.0"]),
            Some(vec!["refs/tags/v0.1.0".into()])
        );
        assert_eq!(names(&["push", "--all", "origin"]), None);
        assert_eq!(refs_asked(&["ls-remote", "--heads", "origin"]), None);
        assert_eq!(
            refs_asked(&["ls-remote", "origin", "refs/convoy/candidates/main", "refs/heads/main"]),
            Some(vec!["refs/convoy/candidates/main".into(), "refs/heads/main".into()])
        );
    }

    /// `turn::certify` pushes the gate record from a worktree `Repo::new` opened
    /// beside the checkout that cached the ref's absence. Both are one repository,
    /// so the parent sees the push.
    #[test]
    fn a_worktree_push_drops_the_ref_the_parent_cached() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("remote.git");
        let main = root.path().join("main");
        let wt = root.path().join("wt");
        let git = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .env("GIT_AUTHOR_NAME", "afkconvoy")
                .env("GIT_AUTHOR_EMAIL", "afkconvoy@localhost")
                .env("GIT_COMMITTER_NAME", "afkconvoy")
                .env("GIT_COMMITTER_EMAIL", "afkconvoy@localhost")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {} in {}: {}",
                args.join(" "),
                dir.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(root.path(), &["init", "--bare", bare.to_str().unwrap()]);
        git(root.path(), &["init", "-b", "main", main.to_str().unwrap()]);
        git(&main, &["config", "maintenance.auto", "false"]);
        git(&main, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&main, &["commit", "--allow-empty", "-m", "init"]);
        git(&main, &["worktree", "add", "--detach", wt.to_str().unwrap(), "HEAD"]);

        let parent = Repo::new(main);
        let child = Repo::new(wt);
        assert_eq!(parent.common_dir().unwrap(), child.common_dir().unwrap());
        let gate = "refs/convoy/gate/x";
        assert_eq!(parent.remote_sha(gate).unwrap(), None);

        let sha = child.record_commit("green").unwrap();
        let spec = format!("{sha}:{gate}");
        let pushed = child.try_git(&["push", "--quiet", "--force", "origin", &spec]).unwrap();
        assert!(pushed.ok, "{}", pushed.stderr);
        assert_eq!(parent.remote_sha(gate).unwrap().as_deref(), Some(sha.as_str()));
    }
}

//! git, as a subprocess: the checkout a command runs in and its one remote.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use afkconvoy_core::layout::{Layout, Note};
use anyhow::{Context, Result, bail};

use crate::remote::{Remote, RemoteRecord, Seen, Wire};

/// The remote every ref and branch is pushed to and fetched from.
pub const REMOTE: &str = "origin";

/// How long one listing of the remote may take before its process group is ended.
pub(crate) const LS_REMOTE_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long one fetch may take before its process group is ended. This covers
/// the largest fetches the fleets make, including zhiyuanwiki's tens of MB.
pub(crate) const FETCH_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

/// What a git call past its limit says of itself: `git <args> did not end
/// within <n>s; its process group was ended`.
pub(crate) const PAST_LIMIT: &str = "did not end within";

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
    /// Remote reads of this checkout's repository, resolved on the first one.
    reads: OnceLock<Arc<RemoteReads>>,
}

/// What this process has read of one repository's remote, and the wire it
/// reads it over. Every worktree of that repository shares one, so a write
/// from any of them drops the ref the others read (`remote::Remote`).
#[derive(Debug)]
pub(crate) struct RemoteReads {
    pub(crate) wire: Arc<dyn Wire>,
    /// Remote refs already read.
    pub(crate) seen: Mutex<Seen>,
    /// A commit's subject, by sha. The sha names the subject, so it stays.
    subjects: Mutex<HashMap<String, String>>,
    /// A commit's whole message, by sha (`Repo::message`): it stays too.
    messages: Mutex<HashMap<String, String>>,
    /// The empty tree's id in the repository's object format
    /// (`record_commit`), which it keeps for as long as it is there.
    empty_tree: OnceLock<String>,
    /// Shas `bring` has already found present. Objects live in the common store.
    present: Mutex<HashSet<String>>,
}

impl RemoteReads {
    fn over(wire: Arc<dyn Wire>) -> RemoteReads {
        RemoteReads {
            wire,
            seen: Mutex::default(),
            subjects: Mutex::default(),
            messages: Mutex::default(),
            empty_tree: OnceLock::new(),
            present: Mutex::default(),
        }
    }
}

impl Default for RemoteReads {
    fn default() -> RemoteReads {
        RemoteReads::over(Arc::new(crate::remote::Git))
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

/// Refuses a git call that reads or writes the remote: refs go through the
/// remote view (`Repo::remote`), and fetches through their bounded operations.
fn local(args: &[&str]) -> Result<()> {
    if let Some(cmd) = args.first().filter(|cmd| ["push", "ls-remote"].contains(*cmd)) {
        bail!("git {cmd} goes through the remote view (`Repo::remote`), never around it");
    }
    if args.first() == Some(&"fetch") {
        bail!("git fetch goes through `Repo::fetch` or `Repo::try_fetch`, never around them");
    }
    Ok(())
}

impl Repo {
    /// The checkout that contains `dir`, by its top directory: a directory
    /// anywhere in a checkout names that checkout.
    pub fn at(dir: &Path) -> Result<Repo> {
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }
        let ran = run(dir, &["rev-parse", "--show-toplevel"], &[])?;
        if ran.ok {
            return Ok(Repo::new(PathBuf::from(ran.stdout)));
        }
        // Only that answer means no checkout: any other is git refusing one
        // (sunfmin/control#304), which it alone can say why.
        if ran.stderr.starts_with("fatal: not a git repository") {
            bail!("{} is not in a git checkout", dir.display());
        }
        bail!("git refused {}: {}", dir.display(), ran.stderr.trim())
    }

    /// The checkout whose top directory is `dir`.
    pub fn new(dir: PathBuf) -> Repo {
        Repo { dir, fixed: Arc::default() }
    }

    /// The checkout `dir`, whose remote is `wire` alone: a fake remote.
    #[cfg(test)]
    pub(crate) fn on_wire(dir: PathBuf, wire: Arc<dyn Wire>) -> Repo {
        let repo = Repo::new(dir);
        let _ = repo.fixed.reads.set(Arc::new(RemoteReads::over(wire)));
        repo
    }

    /// This checkout's remote: every read and write of its refs.
    pub fn remote(&self) -> Remote {
        Remote::of(self)
    }

    /// The repo's git dir, which every worktree of it shares. git is asked
    /// once: a checkout's repo is the same one for as long as it is there.
    pub fn common_dir(&self) -> Result<&Path> {
        if self.fixed.common.get().is_none() {
            self.resolve_common()?;
        }
        Ok(self.fixed.common.get().expect("set above"))
    }

    /// The absolute git common dir, asked once.
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
    pub(crate) fn reads(&self) -> Arc<RemoteReads> {
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

    /// This checkout's own git dir, not the one its worktrees share: a linked
    /// worktree's is named by its `.git` file, read with no git started, as a
    /// tick asks of every worktree it looks at (sunfmin/control#311). git is
    /// asked where there is no such file or it names no directory.
    pub fn git_dir(&self) -> Result<PathBuf> {
        let linked = std::fs::read_to_string(self.dir.join(".git")).ok().and_then(|file| {
            let dir = self.dir.join(file.strip_prefix("gitdir: ")?.trim_end_matches(['\r', '\n']));
            dir.is_dir().then_some(dir)
        });
        match linked {
            Some(dir) => Ok(dir),
            None => Ok(PathBuf::from(self.git(&["rev-parse", "--absolute-git-dir"])?)),
        }
    }

    /// Where a note called `name` is kept on this host: in the repo's git
    /// dir, which every worktree of it shares.
    pub fn note(&self, name: &str) -> Result<PathBuf> {
        Ok(self.common_dir()?.join(name))
    }

    /// One git call whose failure the caller reads.
    pub fn try_git(&self, args: &[&str]) -> Result<Ran> {
        local(args)?;
        run(&self.dir, args, &[])
    }

    /// One git call under extra environment, whose failure the caller reads.
    pub fn try_git_with(&self, args: &[&str], env: &[(&str, &str)]) -> Result<Ran> {
        local(args)?;
        run(&self.dir, args, env)
    }

    /// Removes the linked worktree at `path`, its directory and its
    /// registration, including when a directory in it is not writable by its
    /// owner, as a cancelled test leaves one (sunfmin/control#610).
    ///
    /// Directories are opened for removal first (`copies::removable`): git
    /// deletes the gitdir before the worktree, and stops on a directory the
    /// owner cannot write, leaving the registration gone and the directory
    /// behind. What git leaves is removed here, and the registration pruned.
    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        crate::copies::removable(path);
        let text = path.to_string_lossy();
        let ran = self.try_git(&["worktree", "remove", "--force", text.as_ref()])?;
        if path.exists() {
            crate::copies::removable(path);
            let _ = std::fs::remove_dir_all(path);
        }
        // A removal that deleted the directory and left the registration, or
        // the reverse, converges here. A worktree still on disk stays listed.
        let _ = self.try_git(&["worktree", "prune"]);
        if path.exists() {
            bail!(
                "cannot remove worktree {}: {}",
                path.display(),
                if ran.ok { "it is still there" } else { ran.stderr.trim() }
            );
        }
        Ok(())
    }

    /// One fetch whose failure the caller reads. At `FETCH_LIMIT` its process
    /// group is ended and the call is a failure.
    pub fn try_fetch(&self, args: &[&str]) -> Result<Ran> {
        if args.first() != Some(&"fetch") {
            bail!("Repo::try_fetch takes a git fetch command");
        }
        run_within(&self.dir, args, &[], FETCH_LIMIT)
    }

    /// One fetch that must succeed → its stdout, trimmed.
    pub fn fetch(&self, args: &[&str]) -> Result<String> {
        let ran = self.try_fetch(args)?;
        if !ran.ok {
            bail!("git {} failed: {}", args.join(" "), ran.stderr.trim());
        }
        Ok(ran.stdout)
    }

    /// Is `name` a ref name git takes (`git check-ref-format`)? What git says
    /// depends on the name alone, so it is asked once a name in a process: a
    /// tick checks its base branch's name for every issue it reads.
    pub fn ref_name_ok(&self, name: &str) -> Result<bool> {
        static SAID: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Mutex::default);
        if let Some(&ok) = lock(&SAID).get(name) {
            return Ok(ok);
        }
        let ok = self.try_git(&["check-ref-format", name])?.ok;
        lock(&SAID).insert(name.to_string(), ok);
        Ok(ok)
    }

    /// Is `commit` on `tip`'s history (or `tip` itself)?
    pub fn within(&self, commit: &str, tip: &str) -> Result<bool> {
        Ok(self.try_git(&["merge-base", "--is-ancestor", commit, tip])?.ok)
    }

    /// Brings into this repo the commit `sha` the remote was seen to have at
    /// `refname`, when it is not here yet. An error when the remote no longer
    /// has it there.
    pub fn bring(&self, sha: &str, refname: &str) -> Result<()> {
        if !self.object_here(sha)? {
            self.fetch(&[
                "fetch",
                "--quiet",
                "--no-write-fetch-head",
                "--no-tags",
                REMOTE,
                refname,
            ])?;
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
        local(args)?;
        let out = crate::process::output(
            Command::new("git").current_dir(&self.dir).args(args).env("LC_ALL", "C"),
        )
        .with_context(|| could_not_run(args, &self.dir))?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// One git call that must succeed, `input` on its stdin → its stdout,
    /// trimmed: for text no argument may carry, past the OS's `ARG_MAX`.
    pub fn git_with_input(&self, args: &[&str], input: &str) -> Result<String> {
        local(args)?;
        let out = crate::process::output_with_input(
            Command::new("git").current_dir(&self.dir).args(args).env("LC_ALL", "C"),
            input.as_bytes(),
        )
        .with_context(|| could_not_run(args, &self.dir))?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
        let reads = self.reads();
        if reads.empty_tree.get().is_none() {
            let tree = self.git(&["hash-object", "-t", "tree", "/dev/null"])?;
            let _ = reads.empty_tree.set(tree);
        }
        self.record_commit_of(reads.empty_tree.get().expect("set above"), record)
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

    /// A commit of `tree` on `parent`, whose subject is `record`. The parent
    /// gives a push the edge of the tested commit, so a remote holding it is
    /// sent only the record and objects it still lacks.
    pub fn record_commit_on(&self, tree: &str, parent: &str, record: &str) -> Result<String> {
        let ran =
            run(&self.dir, &["commit-tree", tree, "-p", parent, "-m", record], &RECORD_IDENTITY)?;
        if !ran.ok {
            bail!("git commit-tree failed: {}", ran.stderr.trim());
        }
        Ok(ran.stdout)
    }

    /// The whole message of the commit `sha`, which is here. A sha already
    /// read is not asked again: the sha names the message, and a turn waiting
    /// on a record reads the same one many times.
    pub(crate) fn message(&self, sha: &str) -> Result<String> {
        let reads = self.reads();
        if let Some(message) = lock(&reads.messages).get(sha) {
            return Ok(message.clone());
        }
        let message = self.git(&["show", "-s", "--format=%B", sha])?;
        lock(&reads.messages).insert(sha.to_string(), message.clone());
        Ok(message)
    }

    /// The subject of each of `records`, fetched by ref name when its commit
    /// is not here yet. Nothing is fetched when `records` is empty. A ref the
    /// remote deleted since it was listed is absent: it leaves `records`.
    pub(crate) fn read_record_subjects(&self, records: &mut Vec<RemoteRecord>) -> Result<()> {
        self.read_records(records)
    }

    /// Reads the record each of `records` carries: the subject of its commit,
    /// fetched by the name of each ref when one is not here yet — by name,
    /// for git takes a pattern only with a ref of this repo to write.
    fn read_records(&self, records: &mut Vec<RemoteRecord>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        // The commits are usually here already: fetched by an earlier read.
        let subjects = match self.subjects(records)? {
            Some(subjects) => subjects,
            None => {
                self.fetch_records(records)?;
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

    /// Fetches every one of `records` by name. git fails the whole fetch on
    /// one ref the remote no longer has, and names it: that ref was deleted
    /// after the listing (a publication's scratch ref, a settled claim), so it
    /// leaves `records`, reads as absent from now on, and the rest is fetched
    /// again. Any other failure fails the read.
    fn fetch_records(&self, records: &mut Vec<RemoteRecord>) -> Result<()> {
        while !records.is_empty() {
            let began = lock(&self.reads().seen).began();
            let mut fetch = vec!["fetch", "--quiet", "--no-write-fetch-head", "--no-tags", REMOTE];
            fetch.extend(records.iter().map(|record| record.name.as_str()));
            let ran = self.try_fetch(&fetch)?;
            if ran.ok {
                return Ok(());
            }
            let gone = vanished(&ran.stderr)
                .filter(|gone| records.iter().any(|record| record.name == *gone))
                .map(str::to_string);
            let Some(gone) = gone else {
                bail!("git {} failed: {}", fetch.join(" "), ran.stderr.trim());
            };
            records.retain(|record| record.name != gone);
            lock(&self.reads().seen).vanished(&gone, began);
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

/// The ref a failed fetch says the remote does not have, if that is why
/// it failed.
fn vanished(stderr: &str) -> Option<&str> {
    stderr.lines().find_map(|line| line.trim().strip_prefix("fatal: couldn't find remote ref "))
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

const RECORD_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "afkconvoy"),
    ("GIT_AUTHOR_EMAIL", "afkconvoy@localhost"),
    ("GIT_COMMITTER_NAME", "afkconvoy"),
    ("GIT_COMMITTER_EMAIL", "afkconvoy@localhost"),
];

/// What a git call that could not start says: which call, and the directory
/// it was to run in, which a cancelled candidate's removal may have taken.
fn could_not_run(args: &[&str], dir: &Path) -> String {
    format!("git {} could not run in {}", args.join(" "), dir.display())
}

/// git's messages are read — a ref the remote lacks, a push the server turned
/// down — so they are asked for in one language whatever the machine's locale.
pub(crate) fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Ran> {
    let out = crate::process::output(
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("LC_ALL", "C")
            .envs(env.iter().copied()),
    )
    .with_context(|| could_not_run(args, dir))?;
    Ok(ran(out))
}

/// One git call that must end within `limit`. On the bound its process group
/// is ended and the call is a failure the caller can report, not an error that
/// holds its lane while waiting indefinitely.
pub(crate) fn run_within(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    limit: std::time::Duration,
) -> Result<Ran> {
    let (out, timed_out) = crate::process::output_within_with_timeout(
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("LC_ALL", "C")
            .envs(env.iter().copied()),
        limit,
    )
    .with_context(|| could_not_run(args, dir))?;
    if !timed_out {
        return Ok(ran(out));
    }
    let mut stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !stderr.is_empty() && !stderr.ends_with('\n') {
        stderr.push('\n');
    }
    stderr.push_str(&format!(
        "git {} {PAST_LIMIT} {}s; its process group was ended",
        args.join(" "),
        limit.as_secs()
    ));
    Ok(Ran { ok: false, stdout: String::new(), stderr })
}

fn ran(out: std::process::Output) -> Ran {
    Ran {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `turn::certify` pushes the gate record from a worktree `Repo::new` opened
    /// beside the checkout that read the ref. Both are one repository, so they
    /// share one remote view's reads, and the parent sees the push.
    #[test]
    fn every_worktree_of_a_repository_shares_its_remote_reads() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let wt = root.path().join("wt");
        git(root.path(), &["init", "-q", "-b", "main", main.to_str().unwrap()]);
        git(
            &main,
            &[
                "-c",
                "user.name=a",
                "-c",
                "user.email=a@b",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        git(&main, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "HEAD"]);
        let (parent, child) = (Repo::new(main), Repo::new(wt));
        assert!(Arc::ptr_eq(&parent.reads(), &child.reads()));
    }

    /// A worktree's own git dir is read from its `.git` file, and is the one
    /// git names; the main checkout's is git's answer.
    #[test]
    fn a_checkouts_own_git_dir_is_the_one_git_names() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let wt = root.path().join("wt");
        git(root.path(), &["init", "-q", "-b", "main", main.to_str().unwrap()]);
        git(
            &main,
            &[
                "-c",
                "user.name=a",
                "-c",
                "user.email=a@b",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        git(&main, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "HEAD"]);
        for dir in [main, wt] {
            let repo = Repo::new(dir);
            let said = repo.git(&["rev-parse", "--absolute-git-dir"]).unwrap();
            let read = repo.git_dir().unwrap();
            assert_eq!(read.canonicalize().unwrap(), Path::new(&said).canonicalize().unwrap());
        }
    }

    #[test]
    fn a_raw_push_listing_or_fetch_of_the_remote_is_refused() {
        let repo = Repo::new(PathBuf::from("/nowhere"));
        assert!(repo.try_git(&["push", "origin", "main"]).is_err());
        assert!(repo.git(&["ls-remote", "origin"]).is_err());
        assert!(repo.git_raw(&["push", "origin", "HEAD"]).is_err());
        assert!(repo.try_git(&["fetch", "origin", "main"]).is_err());
    }

    #[test]
    fn a_ref_name_is_judged_by_git_once_a_process() {
        let here = Repo::new(std::env::temp_dir());
        assert!(here.ref_name_ok("refs/heads/judged-once").unwrap());
        assert!(!here.ref_name_ok("refs/heads/judged..once").unwrap());
        // No git runs in a directory that is not there: these are remembered.
        let nowhere = Repo::new(PathBuf::from("/nowhere"));
        assert!(nowhere.ref_name_ok("refs/heads/judged-once").unwrap());
        assert!(!nowhere.ref_name_ok("refs/heads/judged..once").unwrap());
        assert!(nowhere.ref_name_ok("refs/heads/never-judged").is_err());
    }
}

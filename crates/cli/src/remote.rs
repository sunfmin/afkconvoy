//! The remote view: every read and every write of the remote's refs.
//!
//! A read names how fresh it must be (`Fresh`). A write — a `git push` or a
//! receive-pack publication — states the refs it writes up front, and the
//! view drops exactly those from what this process has read. Nothing outside
//! this module runs a push or an ls-remote (`Repo::try_git` refuses both),
//! and no git command line is read to learn what it wrote.

mod publication;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail, ensure};

use crate::git::{REMOTE, Ran, Repo};
use crate::transient;

pub(crate) use publication::{Update, check};

/// How fresh a read of the remote must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fresh {
    /// As the view was opened (`Remote::open`): what a fleet's view was
    /// read from, so every step of it agrees.
    Opened,
    /// Current for this process: a ref this process already read is not
    /// asked again until this process writes it.
    Process,
    /// Live: the remote is asked now. A check that another process may have
    /// moved the ref, or a write that must be certain (ADR 0043), reads this.
    Live,
}

/// One repository's remote, as this process sees it. Every checkout of the
/// repository shares what was read (`Repo::remote`); a view opened with a
/// listing also keeps that listing, which `Fresh::Opened` reads.
#[derive(Debug, Clone)]
pub struct Remote {
    repo: Repo,
    opened: Option<Arc<Listing>>,
}

/// What carries a view's calls to the remote: git itself, or a fake remote
/// in a test.
pub(crate) trait Wire: Send + Sync + std::fmt::Debug {
    /// Every ref matching one of `patterns`, by name with its sha.
    fn list(&self, dir: &Path, patterns: &[&str]) -> Result<Vec<(String, String)>>;
    /// Runs `push` from the checkout `dir`.
    fn push(&self, dir: &Path, push: &Push) -> Result<Ran>;
    /// Runs `push`, ending its process group if it outlives `limit`.
    fn push_within(&self, dir: &Path, push: &Push, limit: Duration) -> Result<Ran>;
    /// Publishes `updates` all together, or none of them → whether it took.
    fn publish(&self, repo: &Repo, updates: &[Update<'_>]) -> Result<bool>;
}

/// The remote named `origin`, through git.
#[derive(Debug, Default)]
pub(crate) struct Git;

impl Wire for Git {
    fn list(&self, dir: &Path, patterns: &[&str]) -> Result<Vec<(String, String)>> {
        let mut args = Vec::with_capacity(patterns.len() + 2);
        args.push("ls-remote");
        args.push(REMOTE);
        args.extend_from_slice(patterns);
        let ran = crate::git::run_within(dir, &args, &[], ls_remote_limit())?;
        if !ran.ok {
            bail!("git {} failed: {}", args.join(" "), ran.stderr.trim());
        }
        Ok(ran
            .stdout
            .lines()
            .filter_map(|row| row.split_once('\t'))
            .map(|(sha, name)| (name.to_string(), sha.to_string()))
            .collect())
    }

    fn push(&self, dir: &Path, push: &Push) -> Result<Ran> {
        let args = push.args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::git::run(dir, &args, &[])
    }

    fn push_within(&self, dir: &Path, push: &Push, limit: Duration) -> Result<Ran> {
        let args = push.args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::git::run_within(dir, &args, &[], limit)
    }

    fn publish(&self, repo: &Repo, updates: &[Update<'_>]) -> Result<bool> {
        publication::push(repo, updates)
    }
}

/// One `git push`: each ref it writes, by its full name, stated as it is
/// built — so the view knows what it wrote without reading a command line.
#[derive(Debug, Clone, Default)]
pub struct Push {
    force: bool,
    atomic: bool,
    progress: bool,
    writes: Vec<Write>,
}

#[derive(Debug, Clone)]
struct Write {
    /// The full name of the remote ref (`refs/...`).
    name: String,
    /// What it is set to — a sha, `HEAD`, a local ref; `None` deletes it.
    from: Option<String>,
    /// The sha the ref must still be at for the write to take.
    lease: Option<String>,
}

impl Push {
    /// Sets the remote ref `name` to `from`.
    pub fn set(from: &str, name: &str) -> Push {
        Push::default().and_set(from, name)
    }

    /// Deletes the remote ref `name`.
    pub fn delete(name: &str) -> Push {
        Push::default().and_delete(name)
    }

    /// Deletes every one of `names`, in one push.
    pub fn deleting<'a>(names: impl IntoIterator<Item = &'a str>) -> Push {
        names.into_iter().fold(Push::default(), Push::and_delete)
    }

    pub fn and_set(mut self, from: &str, name: &str) -> Push {
        self.writes.push(Write { name: name.into(), from: Some(from.into()), lease: None });
        self
    }

    pub fn and_delete(mut self, name: &str) -> Push {
        self.writes.push(Write { name: name.into(), from: None, lease: None });
        self
    }

    /// Writes even what is not a fast-forward.
    pub fn force(mut self) -> Push {
        self.force = true;
        self
    }

    /// Every write takes, or none does.
    pub fn atomic(mut self) -> Push {
        self.atomic = true;
        self
    }

    /// Reports pack progress even when output is captured, so a failed push
    /// can say how much it had written.
    pub fn progress(mut self) -> Push {
        self.progress = true;
        self
    }

    /// The last write takes only while its ref is still at `expected`.
    pub fn leased(mut self, expected: &str) -> Push {
        if let Some(write) = self.writes.last_mut() {
            write.lease = Some(expected.into());
        }
        self
    }

    /// Every ref this push writes.
    pub fn refs(&self) -> impl Iterator<Item = &str> {
        self.writes.iter().map(|write| write.name.as_str())
    }

    fn args(&self) -> Vec<String> {
        let mut args = vec!["push".to_string(), "--quiet".to_string()];
        if self.force {
            args.push("--force".into());
        }
        if self.atomic {
            args.push("--atomic".into());
        }
        if self.progress {
            args.push("--progress".into());
        }
        for write in &self.writes {
            if let Some(lease) = &write.lease {
                args.push(format!("--force-with-lease={}:{lease}", write.name));
            }
        }
        args.push(REMOTE.into());
        for write in &self.writes {
            args.push(format!("{}:{}", write.from.as_deref().unwrap_or(""), write.name));
        }
        args
    }
}

/// Refs of a GitHub repo with no checkout on this host. Keep these reads
/// behind the remote boundary too; callers read record subjects through gh.
pub(crate) fn foreign_refs(
    dir: &Path,
    url: &str,
    patterns: &[&str],
) -> Result<Vec<(String, String)>> {
    let mut args = vec!["ls-remote", url];
    args.extend_from_slice(patterns);
    let ran = crate::git::run_within(dir, &args, &[], ls_remote_limit())?;
    ensure!(ran.ok, "remote {url} could not be read: {}", ran.stderr.trim());
    Ok(ran
        .stdout
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(sha, name)| (name.to_string(), sha.to_string()))
        .collect())
}

/// The work refs of a repo without a local checkout, shared across trackers
/// in one look. Report claims use the live read above instead. A report
/// transition starts a fresh look before rereading these work refs.
pub(crate) fn foreign_work_refs(dir: &Path, url: &str) -> Result<Vec<(String, String)>> {
    static WORK: crate::look::Reading<Result<Vec<(String, String)>, String>> =
        crate::look::Reading::per_look("foreign work refs");
    WORK.read(url, || {
        foreign_refs(dir, url, &["refs/convoy/*", "refs/heads/convoy-*"])
            .map_err(|e| format!("{e:#}"))
    })
    .map_err(anyhow::Error::msg)
}

/// An atomic compare-and-swap on a tracker ref. The object is a parentless
/// empty-tree record made locally, so pushing it carries no checkout history.
pub(crate) fn foreign_push(dir: &Path, url: &str, push: &Push) -> Result<bool> {
    let mut args = push.args();
    let at = args.iter().position(|arg| arg == REMOTE).expect("push remote");
    args[at] = url.to_string();
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let ran = crate::git::run(dir, &args, &[])?;
    if ran.ok {
        return Ok(true);
    }
    // Only a rejected compare-and-swap is a peer winning, never a transport
    // error. A lost receipt leaves the record for a later look to recover.
    if ran.stderr.contains("[rejected]") || ran.stderr.contains("[remote rejected]") {
        for write in &push.writes {
            if let Some(expected) = &write.lease {
                let current = foreign_refs(dir, url, &[&write.name])?;
                if current.first().map_or("", |(_, sha)| sha.as_str()) != expected {
                    return Ok(false);
                }
            }
        }
    }
    bail!("tracker claim push failed: {}", ran.stderr.trim())
}

impl Remote {
    /// The remote of `repo`, sharing what this process has read of it.
    /// Opened with no listing: a `Fresh::Opened` read is refused.
    pub fn of(repo: &Repo) -> Remote {
        Remote { repo: repo.clone(), opened: None }
    }

    /// The remote of `repo`, opened on ONE listing of every branch tip,
    /// including `base_branch`, every version tag and every ref the fleet
    /// keeps: what a rebuild reads, all of it from one moment, and what a
    /// `Fresh::Opened` read answers from.
    pub fn open(repo: &Repo, base_branch: &str) -> Result<Remote> {
        let remote = Remote::of(repo);
        let every = repo.layout().every("");
        let patterns = [every.as_str(), "refs/heads/*", TAGS];
        let listed = remote.ask(&patterns)?;
        let base_ref = format!("refs/heads/{base_branch}");
        let base = listed.iter().find(|record| record.name == base_ref);
        let base = base.map(|record| record.sha.clone());
        let tags = tags_of(&listed);
        let heads = listed
            .iter()
            .filter(|r| r.name.starts_with("refs/heads/"))
            .map(|r| (r.name.clone(), r.sha.clone()))
            .collect();
        let mut records: Vec<RemoteRecord> = listed
            .iter()
            .filter(|record| record.name.starts_with(every.trim_end_matches('*')))
            .cloned()
            .collect();
        repo.read_record_subjects(&mut records)?;
        // A record ref deleted since the listing is absent from all of it.
        let mut listed = listed;
        let every = every.trim_end_matches('*');
        listed.retain(|r| !r.name.starts_with(every) || records.iter().any(|k| k.name == r.name));
        let patterns = patterns.iter().map(|pattern| pattern.to_string()).collect();
        let listing = Listing { base_ref, base, records, tags, heads, listed, patterns };
        Ok(Remote { opened: Some(Arc::new(listing)), ..remote })
    }

    /// The listing this view was opened on.
    pub fn listing(&self) -> Result<&Listing> {
        self.opened.as_deref().ok_or_else(|| anyhow!("this view of the remote was opened unlisted"))
    }

    /// The sha the remote has at `refname`, as fresh as `fresh`; `None` when
    /// it has no such ref. An error when the remote cannot be read.
    pub fn sha(&self, refname: &str, fresh: Fresh) -> Result<Option<String>> {
        let found = self.refs(&[refname], fresh)?.into_iter().find(|r| r.name == refname);
        Ok(found.map(|record| record.sha))
    }

    /// The sha at `refname`, with its commits brought into this repo; `None`
    /// when the remote has no such ref. Like every read of the remote it
    /// writes no ref here and no FETCH_HEAD.
    pub fn fetched(&self, refname: &str, fresh: Fresh) -> Result<Option<String>> {
        let Some(sha) = self.sha(refname, fresh)? else { return Ok(None) };
        self.repo.bring(&sha, refname)?;
        Ok(Some(sha))
    }

    /// Every ref the remote has that matches one of `patterns` — a ref's
    /// name, or `<namespace>/*` — with no record read yet. A ref the remote
    /// does not have is absent, not an error.
    pub fn refs(&self, patterns: &[&str], fresh: Fresh) -> Result<Vec<RemoteRecord>> {
        match fresh {
            Fresh::Opened => self.listing()?.matching(patterns),
            Fresh::Process => self.listed(patterns),
            Fresh::Live => self.ask(patterns),
        }
    }

    /// `refs`, each with the subject of the commit it points at.
    ///
    /// Nothing is written to this checkout's refs or its FETCH_HEAD: worktrees
    /// share both, and a worker's command may be reading the remote meanwhile.
    pub fn records(&self, patterns: &[&str], fresh: Fresh) -> Result<Vec<RemoteRecord>> {
        let mut records = self.refs(patterns, fresh)?;
        self.repo.read_record_subjects(&mut records)?;
        Ok(records)
    }

    /// Every version tag the remote has, each with the commit it is on.
    pub fn tags(&self, fresh: Fresh) -> Result<Vec<(String, String)>> {
        Ok(tags_of(&self.refs(&[TAGS], fresh)?))
    }

    /// Every branch the remote has, by its short name.
    pub fn heads(&self, fresh: Fresh) -> Result<Vec<String>> {
        Ok(self
            .refs(&["refs/heads/*"], fresh)?
            .into_iter()
            .filter_map(|record| record.name.strip_prefix("refs/heads/").map(str::to_string))
            .collect())
    }

    /// Runs `push`, whose failure the caller reads. Each ref it names is
    /// dropped whatever came of it: a push that failed or was refused may
    /// still have changed the remote.
    pub fn try_push(&self, push: &Push) -> Result<Ran> {
        self.try_push_bounded(push, None)
    }

    fn try_push_bounded(&self, push: &Push, limit: Option<Duration>) -> Result<Ran> {
        let refs: Vec<String> = push.refs().map(str::to_string).collect();
        ensure!(!refs.is_empty(), "a push names the refs it writes");
        for name in &refs {
            ensure!(
                name.starts_with("refs/") && is_exact(name),
                "a push names each ref it writes in full, not {name:?}"
            );
        }
        let shared = self.shared();
        let ran = match limit {
            Some(limit) => shared.wire.push_within(&self.repo.dir, push, limit),
            None => shared.wire.push(&self.repo.dir, push),
        };
        lock(&shared.seen).wrote(&refs);
        ran
    }

    /// Runs `push` as `try_push`, ending its process group at `limit` when
    /// one is given — its remote outcome is then unknown, and it is not tried
    /// again — and
    /// runs it again while the remote refuses it for a fault of its own
    /// (`remote_fault`): up to `PUSH_ATTEMPTS` attempts, with a growing pause
    /// between them (sunfmin/control#246, #542). Every push of a fleet ref
    /// goes through this one retry. Before each retry `settled` reads what
    /// the refused push left, and `true` ends the retries: another host's
    /// push took the ref meanwhile, and the caller reads the outcome as a
    /// lost race. → the last attempt, whose failure the caller reads.
    pub fn try_push_retried(
        &self,
        push: &Push,
        limit: Option<Duration>,
        mut settled: impl FnMut() -> Result<bool>,
    ) -> Result<Ran> {
        let mut attempt = 0;
        loop {
            let ran = self.try_push_bounded(push, limit)?;
            attempt += 1;
            if ran.ok || !remote_fault(&ran.stderr) || attempt >= PUSH_ATTEMPTS || settled()? {
                return Ok(ran);
            }
            std::thread::sleep(BETWEEN_PUSH_ATTEMPTS * (1 << attempt));
        }
    }

    /// Runs `push`, which must succeed, tried again as `try_push_retried`.
    pub fn push(&self, push: &Push) -> Result<()> {
        let ran = self.try_push_retried(push, None, || Ok(false))?;
        if !ran.ok {
            bail!("git {} failed: {}", push.args().join(" "), ran.stderr.trim());
        }
        Ok(())
    }

    /// Publishes `updates` together over receive-pack (`publication`) →
    /// whether all of them took. Each ref is dropped whatever came of it.
    pub(crate) fn publish(&self, updates: &[Update<'_>]) -> Result<bool> {
        let refs: Vec<String> = updates.iter().map(|update| update.name.to_string()).collect();
        let published = self.shared().wire.publish(&self.repo, updates);
        lock(&self.shared().seen).wrote(&refs);
        published
    }

    fn shared(&self) -> Arc<crate::git::RemoteReads> {
        self.repo.reads()
    }

    /// The refs `patterns` names, answered from this process's reads when
    /// every pattern is an exact ref they already hold.
    fn listed(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        if patterns.is_empty() || !patterns.iter().copied().all(is_exact) {
            return self.ask(patterns);
        }
        let shared = self.shared();
        let seen = lock(&shared.seen);
        let mut records = Vec::new();
        for pattern in patterns {
            match seen.answer(pattern) {
                Answer::Miss => {
                    drop(seen);
                    return self.ask(patterns);
                }
                Answer::Absent => {}
                Answer::Sha(sha) => records.push(RemoteRecord::of(pattern, &sha)),
            }
        }
        Ok(records)
    }

    /// Lists `patterns` on the remote now, and keeps the answer.
    fn ask(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        let shared = self.shared();
        // Before the remote is asked: a write that lands while it answers is
        // newer than the answer, which must not overwrite it.
        let began = lock(&shared.seen).began();
        let mut attempts = 1;
        let listed = loop {
            match shared.wire.list(&self.repo.dir, patterns) {
                Ok(listed) => break listed,
                Err(error) if !connection_failure(&format!("{error:#}")) => return Err(error),
                Err(error) if attempts == LIST_ATTEMPTS => {
                    return Err(anyhow!(
                        "remote ref read failed after {attempts} attempts: {error:#}"
                    ));
                }
                Err(_) => std::thread::sleep(BETWEEN_LIST_ATTEMPTS),
            }
            attempts += 1;
        };
        // A publication's scratch ref is gone seconds after it is made, and
        // holds no record: no listing carries it.
        let layout = self.repo.layout();
        let records: Vec<RemoteRecord> = listed
            .into_iter()
            .filter(|(name, _)| !publication::is_scratch(layout, name))
            .map(|(name, sha)| RemoteRecord { name, sha, subject: String::new() })
            .collect();
        lock(&shared.seen).remember(&records, patterns, began);
        Ok(records)
    }
}

/// A read changes nothing on the remote, so a failure in transit
/// (`transient::GIT_READ`) is safe to ask again. Any answer from the remote
/// itself fails at once.
fn connection_failure(said: &str) -> bool {
    transient::met(transient::GIT_READ, said)
}

/// A push the remote refused for a fault of its own, not of the push
/// (`transient::GIT_PUSH`). Nothing was written, so the push is safe to try
/// again.
pub(crate) fn remote_fault(stderr: &str) -> bool {
    transient::met(transient::GIT_PUSH, stderr)
}

/// How many times `try_push_retried` tries a push the remote refuses for a
/// fault of its own.
const PUSH_ATTEMPTS: u32 = 5;
/// The first pause between push attempts, which doubles each attempt:
/// 0.5s, 1s, 2s, 4s.
#[cfg(not(test))]
const BETWEEN_PUSH_ATTEMPTS: Duration = Duration::from_millis(250);
#[cfg(test)]
const BETWEEN_PUSH_ATTEMPTS: Duration = Duration::ZERO;

fn ls_remote_limit() -> Duration {
    std::env::var("AFKCONVOY_LS_REMOTE_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(crate::git::LS_REMOTE_LIMIT)
}

pub(crate) fn reads_help() -> String {
    format!(
        "Every git call that reads the remote ends within a limit: an ls-remote within {} seconds and a fetch within {} minutes. At the limit its process group is ended. A read of the remote's refs that meets a connection-level failure — git said {}, in upper or lower case; the last is that limit — is asked again five seconds later, up to three attempts in all. Authentication, a missing repository and every other failure end the read at once. When every attempt fails, the error says how many were made and gives the last failure's text. A push the remote refuses for a fault of its own — it could not write the pack, another push held the ref's lock, or it failed with a server error of its own: git said {} — is tried again after a growing pause, up to {} attempts in all; every other refusal ends it at once. Publications are not retried.",
        crate::git::LS_REMOTE_LIMIT.as_secs(),
        crate::git::FETCH_LIMIT.as_secs() / 60,
        transient::listed(transient::GIT_READ),
        transient::listed(transient::GIT_PUSH),
        PUSH_ATTEMPTS,
    )
}

const LIST_ATTEMPTS: usize = 3;
#[cfg(not(test))]
const BETWEEN_LIST_ATTEMPTS: Duration = Duration::from_secs(5);
#[cfg(test)]
const BETWEEN_LIST_ATTEMPTS: Duration = Duration::ZERO;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What this process has read of one repository's remote refs. An exact
/// ref already covered by a listing is not asked again. A write of the ref
/// drops it, so the next read sees the remote — including a peer that won
/// the push.
///
/// Threads of one process share it, and a read takes the time the remote
/// takes to answer: what a read began before a write of this process landed
/// is older than that write, and changes nothing of the ref it wrote
/// (sunfmin/control#828). Each read says which write it began after
/// (`began`).
#[derive(Debug, Default)]
pub(crate) struct Seen {
    /// `Some` is the sha the remote had; `None` is a read that found it absent.
    sha: HashMap<String, Option<String>>,
    /// Glob prefixes (`refs/heads/`) for which every ref is in `sha` or absent.
    covered: Vec<String>,
    /// Written by this process since the read that filled `sha`.
    dirty: std::collections::HashSet<String>,
    /// How many writes this process has made.
    writes: u64,
    /// Each ref this process wrote, by the count of its last write.
    written: HashMap<String, u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Miss,
    Absent,
    Sha(String),
}

impl Seen {
    fn answer(&self, name: &str) -> Answer {
        if self.dirty.contains(name) {
            return Answer::Miss;
        }
        if let Some(found) = self.sha.get(name) {
            return match found {
                Some(sha) => Answer::Sha(sha.clone()),
                None => Answer::Absent,
            };
        }
        if self.covered.iter().any(|prefix| name.starts_with(prefix)) {
            return Answer::Absent;
        }
        Answer::Miss
    }

    /// What a read beginning now is to say it began after (`remember`).
    pub(crate) fn began(&self) -> u64 {
        self.writes
    }

    /// Whether this process wrote `name` after a read that `began` then.
    fn newer(&self, name: &str, began: u64) -> bool {
        self.written.get(name).is_some_and(|&at| at > began)
    }

    /// Forgets every read but of a ref written after `began`.
    fn clear(&mut self, began: u64) {
        let written = &self.written;
        let newer = |name: &str| written.get(name).is_some_and(|&at| at > began);
        self.sha.retain(|name, _| newer(name));
        self.dirty.retain(|name| newer(name));
        self.covered.clear();
    }

    /// A fetch by name, begun after the write `began` says, found the remote
    /// no longer has `name`: it is absent until a read finds it again.
    pub(crate) fn vanished(&mut self, name: &str, began: u64) {
        if self.newer(name, began) {
            return;
        }
        self.sha.insert(name.to_string(), None);
        self.dirty.remove(name);
    }

    /// A write to the remote changed `refs`, each an exact name: the next
    /// read of each asks the remote. Other refs stay.
    fn wrote(&mut self, refs: &[String]) {
        self.writes += 1;
        for name in refs {
            self.sha.remove(name);
            self.dirty.insert(name.clone());
            self.written.insert(name.clone(), self.writes);
        }
    }

    /// `records` is what `patterns` listed, in a read that `began` after
    /// that many writes. A glob prefix becomes complete: a name under it that
    /// was not listed is absent. A ref written since the read began keeps
    /// what that write left: the next read of it asks the remote, or answers
    /// what a read begun after the write found.
    fn remember(&mut self, records: &[RemoteRecord], patterns: &[&str], began: u64) {
        if patterns.iter().any(|pattern| glob_prefix(pattern).is_none() && !is_exact(pattern)) {
            self.clear(began);
            return;
        }
        let written = &self.written;
        let older = |name: &str| written.get(name).is_none_or(|&at| at <= began);
        for pattern in patterns {
            if let Some(prefix) = glob_prefix(pattern) {
                self.sha.retain(|name, _| !name.starts_with(&prefix) || !older(name));
                self.dirty.retain(|name| !name.starts_with(&prefix) || !older(name));
                if !self.covered.iter().any(|had| had == &prefix) {
                    self.covered.push(prefix);
                }
            } else if older(pattern) {
                self.sha.remove(*pattern);
                self.dirty.remove(*pattern);
            }
        }
        for record in records.iter().filter(|record| older(&record.name)) {
            self.sha.insert(record.name.clone(), Some(record.sha.clone()));
            self.dirty.remove(&record.name);
        }
        for pattern in patterns {
            if is_exact(pattern)
                && older(pattern)
                && !records.iter().any(|record| record.name == *pattern)
            {
                self.sha.insert((*pattern).to_string(), None);
            }
        }
    }
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

/// One ref of the remote and the record its commit carries.
#[derive(Debug, Clone)]
pub struct RemoteRecord {
    pub name: String,
    pub sha: String,
    pub subject: String,
}

impl RemoteRecord {
    fn of(name: &str, sha: &str) -> RemoteRecord {
        RemoteRecord { name: name.to_string(), sha: sha.to_string(), subject: String::new() }
    }
}

/// Every branch tip and ref of the fleet, as one listing of the remote saw
/// them (`Remote::open`).
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
    /// Every ref the listing returned, with no record read.
    listed: Vec<RemoteRecord>,
    /// What it asked for: the refs it can answer for.
    patterns: Vec<String>,
}

impl Listing {
    /// The sha a fleet ref or branch had in this listing, `None` if absent.
    pub fn sha(&self, refname: &str) -> Option<&str> {
        let found = self.records.iter().find(|record| record.name == refname);
        found.map(|record| record.sha.as_str()).or_else(|| {
            self.heads.iter().find(|(name, _)| name == refname).map(|(_, sha)| sha.as_str())
        })
    }

    /// The listed refs matching `patterns`. A pattern the listing did not
    /// ask for is an error: the listing cannot say it is absent.
    fn matching(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        let covered: Vec<String> = self.patterns.iter().filter_map(|p| glob_prefix(p)).collect();
        let mut found = Vec::new();
        for pattern in patterns {
            let prefix = glob_prefix(pattern);
            let within = |had: &String| match &prefix {
                Some(prefix) => prefix.starts_with(had.as_str()),
                None => pattern.starts_with(had.as_str()),
            };
            ensure!(
                covered.iter().any(within),
                "{pattern} is not in the listing this view of the remote was opened on"
            );
            found.extend(
                self.listed
                    .iter()
                    .filter(|record| match &prefix {
                        Some(prefix) => record.name.starts_with(prefix.as_str()),
                        None => record.name == *pattern,
                    })
                    .filter(|record| {
                        !found.iter().any(|had: &RemoteRecord| had.name == record.name)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            );
        }
        Ok(found)
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::path::PathBuf;

    #[test]
    fn tracker_compare_and_swap_admits_one_host_and_never_releases_a_successor() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ran = crate::git::run(dir.path(), args, &[]).unwrap();
            assert!(ran.ok, "{}", ran.stderr);
        };
        run(&["init", "-q", "--bare", "remote.git"]);
        run(&["init", "-q", "local"]);
        let repo = Repo::at(&dir.path().join("local")).unwrap();
        let remote = dir.path().join("remote.git").display().to_string();
        let name = "refs/heads/convoy-unworked/acme/repo";
        let first = repo.record_commit("first host").unwrap();
        let second = repo.record_commit("second host").unwrap();
        let (a, b) = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                foreign_push(&repo.dir, &remote, &Push::set(&first, name).leased("")).unwrap()
            });
            let b = scope.spawn(|| {
                foreign_push(&repo.dir, &remote, &Push::set(&second, name).leased("")).unwrap()
            });
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_ne!(a, b);
        let won = if a { &first } else { &second };
        assert_eq!(foreign_refs(&repo.dir, &remote, &[name]).unwrap()[0].1, *won);
        let successor = repo.record_commit("next host after the lease").unwrap();
        assert!(
            foreign_push(&repo.dir, &remote, &Push::set(&successor, name).leased(won)).unwrap()
        );
        assert!(!foreign_push(&repo.dir, &remote, &Push::delete(name).leased(won)).unwrap());
        assert_eq!(foreign_refs(&repo.dir, &remote, &[name]).unwrap()[0].1, successor);
        assert!(foreign_push(&repo.dir, &remote, &Push::delete(name).leased(&successor)).unwrap());
    }

    /// sunfmin/control#531: a record ref deleted between the listing and the
    /// fetch reads as absent, and the rest of the state reads as usual. A
    /// publication's scratch ref is never listed as a record.
    #[test]
    fn a_ref_deleted_after_the_listing_reads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ran = crate::git::run(dir.path(), args, &[]).unwrap();
            assert!(ran.ok, "{}", ran.stderr);
        };
        run(&["init", "-q", "--bare", "remote.git"]);
        for clone in ["writer", "reader"] {
            run(&["init", "-q", clone]);
            run(&["-C", clone, "remote", "add", "origin", "../remote.git"]);
        }
        let writer = Repo::at(&dir.path().join("writer")).unwrap();
        let reader = Repo::at(&dir.path().join("reader")).unwrap();
        let layout = reader.layout();
        let (gone, kept) = (layout.under("test-gone"), layout.under("test-kept"));
        let scratch = layout.under("publish-1-0");
        let (a, b) = (writer.record_commit("gone").unwrap(), writer.record_commit("kept").unwrap());
        let push = Push::set(&a, &gone).and_set(&b, &kept).and_set(&a, &scratch);
        assert!(Remote::of(&writer).try_push(&push).unwrap().ok);

        let every = layout.every("");
        let mut records = Remote::of(&reader).refs(&[&every], Fresh::Live).unwrap();
        let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, [gone.as_str(), kept.as_str()], "a scratch ref is no record");

        assert!(Remote::of(&writer).try_push(&Push::delete(&gone)).unwrap().ok);
        reader.read_record_subjects(&mut records).unwrap();
        let read: Vec<(&str, &str)> =
            records.iter().map(|r| (r.name.as_str(), r.subject.as_str())).collect();
        assert_eq!(read, [(kept.as_str(), "kept")]);
        let again = Remote::of(&reader).refs(&[&gone], Fresh::Process).unwrap();
        assert!(again.is_empty(), "{again:?}");
        let opened = Remote::open(&reader, "main").unwrap();
        assert!(opened.listing().unwrap().sha(&gone).is_none());

        // A fetch that fails for any other reason fails the read.
        run(&["-C", "reader", "remote", "set-url", "origin", "../nowhere.git"]);
        let other = Repo::at(&dir.path().join("reader")).unwrap();
        let mut unread =
            vec![RemoteRecord { name: kept.clone(), sha: "0".repeat(40), subject: String::new() }];
        let error = other.read_record_subjects(&mut unread).unwrap_err();
        assert!(!format!("{error:#}").contains("couldn't find remote ref"), "{error:#}");
        assert_eq!(unread.len(), 1);
    }

    /// A remote held in memory: its refs, and how many times it was listed
    /// and pushed to.
    #[derive(Debug, Default)]
    pub(crate) struct Fake {
        refs: Mutex<BTreeMap<String, String>>,
        lists: Mutex<usize>,
        failures: Mutex<VecDeque<String>>,
        /// The next pushes of a ref it refuses, in order.
        refusals: Mutex<VecDeque<Refusal>>,
        pushes: Mutex<usize>,
        /// The next listings, each answering what the remote had as it was
        /// asked only once its `Stall` lets it.
        stalls: Mutex<VecDeque<Stall>>,
    }

    /// A listing that has read the remote waits at `read`, and then at
    /// `answer` before it answers.
    #[derive(Debug, Clone)]
    struct Stall {
        read: Arc<std::sync::Barrier>,
        answer: Arc<std::sync::Barrier>,
    }

    impl Stall {
        fn new() -> Stall {
            let barrier = || Arc::new(std::sync::Barrier::new(2));
            Stall { read: barrier(), answer: barrier() }
        }
    }

    /// A push of `name` the remote refuses saying `stderr`; meanwhile
    /// another host's push may have set `name` to `taken`.
    #[derive(Debug)]
    struct Refusal {
        name: String,
        stderr: String,
        taken: Option<String>,
    }

    impl Fake {
        pub(crate) fn with(refs: &[(&str, &str)]) -> Arc<Fake> {
            let fake = Fake::default();
            for (name, sha) in refs {
                lock(&fake.refs).insert(name.to_string(), sha.to_string());
            }
            Arc::new(fake)
        }

        /// Another process moves `name` on the remote.
        fn moves(&self, name: &str, sha: &str) {
            lock(&self.refs).insert(name.to_string(), sha.to_string());
        }

        fn fails_next(&self, error: &str) {
            lock(&self.failures).push_back(error.to_string());
        }

        fn lists(&self) -> usize {
            *lock(&self.lists)
        }

        /// The next push of `name` is refused, saying `stderr`.
        pub(crate) fn refuses(&self, name: &str, stderr: &str) {
            let refusal = Refusal { name: name.into(), stderr: stderr.into(), taken: None };
            lock(&self.refusals).push_back(refusal);
        }

        /// The next push of `name` is refused, saying `stderr`, while another
        /// host's push sets it to `sha`.
        pub(crate) fn refuses_taken(&self, name: &str, stderr: &str, sha: &str) {
            let taken = Some(sha.to_string());
            lock(&self.refusals).push_back(Refusal {
                name: name.into(),
                stderr: stderr.into(),
                taken,
            });
        }

        pub(crate) fn sha(&self, name: &str) -> Option<String> {
            lock(&self.refs).get(name).cloned()
        }

        /// How many pushes it was sent.
        pub(crate) fn pushes(&self) -> usize {
            *lock(&self.pushes)
        }
    }

    impl Wire for Fake {
        fn list(&self, _: &Path, patterns: &[&str]) -> Result<Vec<(String, String)>> {
            *lock(&self.lists) += 1;
            if let Some(error) = lock(&self.failures).pop_front() {
                bail!(error);
            }
            let listed = lock(&self.refs)
                .iter()
                .filter(|(name, _)| {
                    patterns.iter().any(|pattern| match pattern.strip_suffix('*') {
                        Some(prefix) => name.starts_with(prefix),
                        None => name.as_str() == *pattern,
                    })
                })
                .map(|(name, sha)| (name.clone(), sha.clone()))
                .collect();
            let stall = lock(&self.stalls).pop_front();
            if let Some(stall) = stall {
                stall.read.wait();
                stall.answer.wait();
            }
            Ok(listed)
        }

        fn push(&self, _: &Path, push: &Push) -> Result<Ran> {
            *lock(&self.pushes) += 1;
            let mut refs = lock(&self.refs);
            let mut refusals = lock(&self.refusals);
            let first = refusals.iter().position(|r| push.refs().any(|name| name == r.name));
            if let Some(refusal) = first.and_then(|at| refusals.remove(at)) {
                if let Some(sha) = refusal.taken {
                    refs.insert(refusal.name, sha);
                }
                return Ok(Ran { ok: false, stdout: String::new(), stderr: refusal.stderr });
            }
            drop(refusals);
            let refused = push.writes.iter().any(|write| {
                let at = refs.get(&write.name);
                match &write.lease {
                    Some(lease) => at != Some(lease),
                    None => !push.force && write.from.is_some() && at.is_some(),
                }
            });
            if refused {
                let stderr = "! [rejected] (stale info)".to_string();
                return Ok(Ran { ok: false, stdout: String::new(), stderr });
            }
            for write in &push.writes {
                match &write.from {
                    Some(sha) => refs.insert(write.name.clone(), sha.clone()),
                    None => refs.remove(&write.name),
                };
            }
            Ok(Ran { ok: true, stdout: String::new(), stderr: String::new() })
        }

        fn push_within(&self, dir: &Path, push: &Push, _: Duration) -> Result<Ran> {
            self.push(dir, push)
        }

        fn publish(&self, _: &Repo, updates: &[Update<'_>]) -> Result<bool> {
            let mut refs = lock(&self.refs);
            let zero = |sha: &str| sha.bytes().all(|c| c == b'0');
            let held = updates.iter().all(|update| {
                refs.get(update.name).map(String::as_str)
                    == (!zero(update.old)).then_some(update.old)
            });
            if held {
                for update in updates {
                    refs.insert(update.name.to_string(), update.new.to_string());
                }
            }
            Ok(held)
        }
    }

    fn remote(fake: &Arc<Fake>) -> Remote {
        let wire: Arc<dyn Wire> = fake.clone();
        Remote::of(&Repo::on_wire(PathBuf::from("/nowhere"), wire))
    }

    /// What GitHub said refusing a push while another push held the ref's
    /// lock, on 2026-11-03 (sunfmin/control#542).
    pub(crate) const HELD: &str = "To https://github.com/o/r.git\n ! [remote rejected]   31515b8c -> refs/convoy/heartbeat/h (failed)\nerror: failed to push some refs";

    /// What GitHub said refusing a claim push during its incident of
    /// 2026-11-04, tick 56 (sunfmin/control#593).
    const SERVER_ERROR: &str = "To https://github.com/sunfmin/control.git\n ! [remote rejected]   4f2a9c1e -> refs/convoy/claim/sunfmin/control/589 (Internal Server Error)\nerror: failed to push some refs to 'https://github.com/sunfmin/control.git'";

    #[test]
    fn a_ref_update_refused_with_internal_server_error_is_the_remotes_fault() {
        assert!(remote_fault(SERVER_ERROR));
    }

    #[test]
    fn a_push_refused_with_internal_server_error_takes_on_a_later_attempt() {
        let fake = Fake::with(&[]);
        let name = "refs/convoy/claim/sunfmin/control/589";
        fake.refuses(name, SERVER_ERROR);
        let ran =
            remote(&fake).try_push_retried(&Push::set("b", name), None, || Ok(false)).unwrap();
        assert!(ran.ok, "{}", ran.stderr);
        assert_eq!(fake.sha(name).as_deref(), Some("b"));
        assert_eq!(fake.pushes(), 2);
    }

    #[test]
    fn a_ref_lock_held_by_another_push_or_an_unwritten_pack_is_the_remotes_fault() {
        assert!(remote_fault(HELD));
        assert!(remote_fault(
            "error: cannot lock ref 'refs/convoy/candidates/main': is at 1 but expected 2"
        ));
        let unpack = "remote: fatal: unable to rename temporary '*.pack' file\n\
                      error: remote unpack failed: index-pack failed\n \
                      ! [remote rejected]   d5531db -> refs/convoy/gate/47 (unpacker error)";
        assert!(remote_fault(unpack));
        for refused in [
            " ! [remote rejected] HEAD -> refs/convoy/candidates/main (permission denied)",
            " ! [remote rejected] d5531db -> refs/convoy/gate/47 (protected)",
            " ! [rejected]        HEAD -> main (non-fast-forward)",
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host",
            "fatal: 'origin' does not appear to be a git repository",
            "remote: Invalid username or token. Password authentication is not supported for Git operations.\n\
             fatal: Authentication failed for 'https://github.com/o/r.git/'",
        ] {
            assert!(!remote_fault(refused), "{refused}");
        }
    }

    #[test]
    fn a_push_refused_for_a_held_ref_lock_takes_on_a_later_attempt() {
        let fake = Fake::with(&[]);
        let name = "refs/convoy/heartbeat/h";
        fake.refuses(name, HELD);
        fake.refuses(name, HELD);
        remote(&fake).push(&Push::set("b", name).force()).unwrap();
        assert_eq!(fake.sha(name).as_deref(), Some("b"));
        assert_eq!(fake.pushes(), 3);
    }

    #[test]
    fn a_push_refused_otherwise_or_held_on_every_attempt_fails_with_what_the_remote_said() {
        let fake = Fake::with(&[]);
        let name = "refs/convoy/heartbeat/h";
        fake.refuses(name, " ! [remote rejected] b -> refs/convoy/heartbeat/h (protected)");
        let error = remote(&fake).push(&Push::set("b", name)).unwrap_err().to_string();
        assert!(error.contains("(protected)"), "{error}");
        assert_eq!(fake.pushes(), 1);

        let fake = Fake::with(&[]);
        for _ in 0..PUSH_ATTEMPTS {
            fake.refuses(name, HELD);
        }
        let error = remote(&fake).push(&Push::set("b", name)).unwrap_err().to_string();
        assert!(error.contains("(failed)"), "{error}");
        assert_eq!(fake.pushes(), PUSH_ATTEMPTS as usize);
        assert_eq!(fake.sha(name), None);
    }

    #[test]
    fn a_settled_refusal_is_not_tried_again() {
        let fake = Fake::with(&[]);
        let name = "refs/convoy/claim/7";
        fake.refuses_taken(name, HELD, "theirs");
        let remote = remote(&fake);
        let settled = || Ok(remote.sha(name, Fresh::Live)?.is_some());
        let ran = remote.try_push_retried(&Push::set("mine", name), None, settled).unwrap();
        assert!(!ran.ok);
        assert_eq!(fake.pushes(), 1);
        assert_eq!(fake.sha(name).as_deref(), Some("theirs"));
    }

    #[test]
    fn a_read_for_this_process_is_not_asked_twice() {
        let fake = Fake::with(&[("refs/heads/trunk", "a")]);
        let remote = remote(&fake);
        assert_eq!(remote.sha("refs/heads/trunk", Fresh::Process).unwrap().as_deref(), Some("a"));
        fake.moves("refs/heads/trunk", "b");
        assert_eq!(remote.sha("refs/heads/trunk", Fresh::Process).unwrap().as_deref(), Some("a"));
        assert_eq!(fake.lists(), 1);
    }

    #[test]
    fn a_live_read_asks_the_remote_and_the_process_reads_follow_it() {
        let fake = Fake::with(&[("refs/heads/trunk", "a")]);
        let remote = remote(&fake);
        remote.sha("refs/heads/trunk", Fresh::Process).unwrap();
        fake.moves("refs/heads/trunk", "b");
        assert_eq!(remote.sha("refs/heads/trunk", Fresh::Live).unwrap().as_deref(), Some("b"));
        assert_eq!(remote.sha("refs/heads/trunk", Fresh::Process).unwrap().as_deref(), Some("b"));
        assert_eq!(fake.lists(), 2);
    }

    #[test]
    fn a_connection_failure_is_read_again() {
        let fake = Fake::with(&[("refs/heads/trunk", "a")]);
        fake.fails_next(
            "git ls-remote origin refs/heads/trunk failed: fatal: unable to access 'https://github.com/sunfmin/afkconvoy.git/': TLS connect error: error:0A000126:SSL routines::unexpected eof while reading",
        );
        let remote = remote(&fake);
        assert_eq!(remote.sha("refs/heads/trunk", Fresh::Live).unwrap().as_deref(), Some("a"));
        assert_eq!(fake.lists(), 2);
    }

    #[test]
    fn an_authentication_failure_is_not_read_again() {
        let fake = Fake::with(&[("refs/heads/trunk", "a")]);
        fake.fails_next("git ls-remote origin refs/heads/trunk failed: Authentication failed");
        let remote = remote(&fake);
        assert!(remote.sha("refs/heads/trunk", Fresh::Live).is_err());
        assert_eq!(fake.lists(), 1);
    }

    #[test]
    fn a_read_that_keeps_failing_says_how_many_attempts_it_made() {
        let fake = Fake::with(&[]);
        for _ in 0..3 {
            fake.fails_next("git ls-remote origin refs/heads/trunk failed: SSL_ERROR_SYSCALL");
        }
        let remote = remote(&fake);
        let error = remote.sha("refs/heads/trunk", Fresh::Live).unwrap_err().to_string();
        assert!(error.contains("3 attempts"), "{error}");
        assert!(error.contains("SSL_ERROR_SYSCALL"), "{error}");
        assert_eq!(fake.lists(), 3);
    }

    /// Each marker of `list`, in its order, with what was said holding it.
    fn each_said(list: &[transient::Marker], said: &[(&str, &str)]) {
        let markers: Vec<&str> = list.iter().map(|marker| marker.said).collect();
        let named: Vec<&str> = said.iter().map(|(marker, _)| *marker).collect();
        assert_eq!(named, markers, "a marker with no example, or one out of order");
    }

    #[test]
    fn a_read_that_said_any_marker_of_its_list_is_read_again() {
        let said = [
            (
                "TLS connect error",
                "fatal: unable to access 'https://github.com/o/r.git/': TLS connect error: error:0A000126:SSL routines::unexpected eof while reading",
            ),
            ("unexpected EOF", "error: RPC failed; curl 18 unexpected EOF"),
            (
                "SSL_ERROR_SYSCALL",
                "fatal: unable to access 'https://github.com/o/r.git/': LibreSSL SSL_connect: SSL_ERROR_SYSCALL in connection to github.com:443",
            ),
            (
                "connection reset",
                "fatal: unable to access '…': Recv failure: Connection reset by peer",
            ),
            (
                "connection refused",
                "fatal: unable to access '…': Failed to connect to github.com port 443: Connection refused",
            ),
            (
                "could not resolve host",
                "fatal: unable to access '…': Could not resolve host: github.com",
            ),
            ("timeout", "fatal: unable to access '…': SSL connection timeout"),
            (
                "timed out",
                "fatal: unable to access '…': Failed to connect to github.com port 443: Operation timed out",
            ),
            (
                crate::git::PAST_LIMIT,
                "git ls-remote origin refs/heads/trunk did not end within 60s; its process group was ended",
            ),
        ];
        each_said(transient::GIT_READ, &said);
        for (marker, said) in said {
            assert!(connection_failure(said), "{marker}: {said}");
            let fake = Fake::with(&[("refs/heads/trunk", "a")]);
            fake.fails_next(said);
            let read = remote(&fake).sha("refs/heads/trunk", Fresh::Live).unwrap();
            assert_eq!(read.as_deref(), Some("a"), "{marker}");
            assert_eq!(fake.lists(), 2, "{marker}");
        }
    }

    #[test]
    fn a_push_refused_with_any_marker_of_its_list_is_tried_again() {
        let said = [
            (
                "remote unpack failed",
                "error: remote unpack failed: index-pack failed\n ! [remote rejected] d5531db -> refs/convoy/gate/47 (unpacker error)",
            ),
            ("(failed)", HELD),
            (
                "cannot lock ref",
                "error: cannot lock ref 'refs/convoy/candidates/main': is at 1 but expected 2",
            ),
            ("failed to lock", " ! [remote rejected] HEAD -> refs/convoy/claim/7 (failed to lock)"),
            ("(Internal Server Error)", SERVER_ERROR),
        ];
        each_said(transient::GIT_PUSH, &said);
        for (marker, said) in said {
            assert!(remote_fault(said), "{marker}: {said}");
            let fake = Fake::with(&[]);
            let name = "refs/convoy/claim/sunfmin/control/589";
            fake.refuses(name, said);
            let ran =
                remote(&fake).try_push_retried(&Push::set("b", name), None, || Ok(false)).unwrap();
            assert!(ran.ok, "{marker}: {}", ran.stderr);
            assert_eq!(fake.pushes(), 2, "{marker}");
        }
    }

    #[test]
    fn the_help_names_every_marker_of_the_reads_and_the_pushes_from_their_lists() {
        let help = reads_help();
        assert!(help.contains(&transient::listed(transient::GIT_READ)), "{help}");
        assert!(help.contains(&transient::listed(transient::GIT_PUSH)), "{help}");
        for marker in transient::GIT_READ.iter().chain(transient::GIT_PUSH) {
            assert!(help.contains(marker.said), "{}: {help}", marker.said);
        }
    }

    #[test]
    fn only_connection_failures_are_read_again() {
        for said in [
            "TLS connect error",
            "unexpected EOF",
            "SSL_ERROR_SYSCALL",
            "Connection reset by peer",
            "Connection refused",
            "Could not resolve host",
            "Operation timed out",
            "TLS handshake timeout",
            "git ls-remote origin did not end within 60s; its process group was ended",
        ] {
            assert!(connection_failure(said), "{said}");
        }
        for said in [
            "Authentication failed",
            "Repository not found",
            "The requested URL returned error: 403",
        ] {
            assert!(!connection_failure(said), "{said}");
        }
    }

    #[test]
    fn a_read_as_opened_answers_from_the_listing_whatever_happened_since() {
        let fake = Fake::with(&[("refs/heads/trunk", "a"), ("refs/convoy/claim/7", "c")]);
        let repo = Repo::on_wire(PathBuf::from("/nowhere"), fake.clone());
        let listing = Listing {
            heads: vec![("refs/heads/trunk".into(), "a".into())],
            base_ref: "refs/heads/trunk".into(),
            base: Some("a".into()),
            records: vec![],
            tags: vec![],
            listed: vec![RemoteRecord::of("refs/heads/trunk", "a")],
            patterns: vec!["refs/heads/*".into()],
        };
        let opened = Remote { repo, opened: Some(Arc::new(listing)) };
        fake.moves("refs/heads/trunk", "b");
        opened.push(&Push::set("n", "refs/heads/trunk").force()).unwrap();
        assert_eq!(opened.sha("refs/heads/trunk", Fresh::Opened).unwrap().as_deref(), Some("a"));
        assert_eq!(opened.sha("refs/heads/gone", Fresh::Opened).unwrap(), None);
        assert!(opened.sha("refs/convoy/claim/7", Fresh::Opened).is_err());
        assert_eq!(opened.sha("refs/heads/trunk", Fresh::Process).unwrap().as_deref(), Some("n"));
        assert_eq!(fake.lists(), 1);
        assert!(remote(&fake).sha("refs/heads/trunk", Fresh::Opened).is_err());
    }

    #[test]
    fn a_push_drops_exactly_the_refs_it_wrote_whatever_the_base_branch_is_called() {
        let fake = Fake::with(&[("refs/heads/trunk", "a"), ("refs/heads/other", "o")]);
        let remote = remote(&fake);
        remote.refs(&["refs/heads/*"], Fresh::Process).unwrap();
        remote.sha("refs/convoy/target/7", Fresh::Process).unwrap();
        assert_eq!(fake.lists(), 2);

        remote.push(&Push::set("b", "refs/heads/trunk").force()).unwrap();
        fake.moves("refs/heads/other", "elsewhere");
        assert_eq!(remote.sha("refs/heads/trunk", Fresh::Process).unwrap().as_deref(), Some("b"));
        assert_eq!(fake.lists(), 3);
        assert_eq!(remote.sha("refs/heads/other", Fresh::Process).unwrap().as_deref(), Some("o"));
        assert_eq!(remote.sha("refs/heads/gone", Fresh::Process).unwrap(), None);
        assert_eq!(remote.sha("refs/convoy/target/7", Fresh::Process).unwrap(), None);
        assert_eq!(fake.lists(), 3);
    }

    #[test]
    fn a_refused_push_still_drops_the_refs_it_named() {
        let fake = Fake::with(&[("refs/convoy/claim/7", "theirs")]);
        let remote = remote(&fake);
        remote.sha("refs/convoy/claim/7", Fresh::Process).unwrap();
        fake.moves("refs/convoy/claim/7", "peer");
        let push = Push::delete("refs/convoy/claim/7").leased("theirs");
        assert!(!remote.try_push(&push).unwrap().ok);
        let now = remote.sha("refs/convoy/claim/7", Fresh::Process).unwrap();
        assert_eq!(now.as_deref(), Some("peer"));
    }

    #[test]
    fn a_publication_drops_exactly_the_refs_it_wrote() {
        let fake = Fake::with(&[("refs/heads/trunk", "a"), ("refs/heads/other", "o")]);
        let remote = remote(&fake);
        remote.refs(&["refs/heads/*", "refs/convoy/*"], Fresh::Process).unwrap();
        let zero = "0".repeat(40);
        let updates = [
            Update { name: "refs/heads/trunk", old: "a", new: "b" },
            Update { name: "refs/convoy/delivery/x", old: &zero, new: "d" },
        ];
        assert!(remote.publish(&updates).unwrap());
        fake.moves("refs/heads/other", "elsewhere");
        assert_eq!(remote.sha("refs/heads/trunk", Fresh::Process).unwrap().as_deref(), Some("b"));
        let delivered = remote.sha("refs/convoy/delivery/x", Fresh::Process).unwrap();
        assert_eq!(delivered.as_deref(), Some("d"));
        assert_eq!(remote.sha("refs/heads/other", Fresh::Process).unwrap().as_deref(), Some("o"));
    }

    #[test]
    fn a_push_names_each_ref_in_full() {
        let fake = Fake::with(&[]);
        let remote = remote(&fake);
        assert!(remote.try_push(&Push::set("HEAD", "main")).is_err());
        assert!(remote.try_push(&Push::default()).is_err());
        assert!(lock(&fake.refs).is_empty());
    }

    #[test]
    fn a_push_says_what_it_writes() {
        let push = Push::set("abc", "refs/convoy/claim/7").leased("old").and_delete("refs/x");
        assert_eq!(
            push.args(),
            [
                "push",
                "--quiet",
                "--force-with-lease=refs/convoy/claim/7:old",
                "origin",
                "abc:refs/convoy/claim/7",
                ":refs/x"
            ]
        );
        assert_eq!(push.refs().collect::<Vec<_>>(), ["refs/convoy/claim/7", "refs/x"]);
    }

    #[test]
    fn an_atomic_push_says_so() {
        let push = Push::default().atomic().and_delete("refs/a").leased("1").and_delete("refs/b");
        assert_eq!(
            push.args(),
            [
                "push",
                "--quiet",
                "--atomic",
                "--force-with-lease=refs/a:1",
                "origin",
                ":refs/a",
                ":refs/b"
            ]
        );
    }

    #[test]
    fn a_listing_answers_an_absent_ref_under_its_prefix() {
        let mut seen = Seen::default();
        seen.remember(&[RemoteRecord::of("refs/heads/main", "abc")], &["refs/heads/*"], 0);
        assert_eq!(seen.answer("refs/heads/main"), Answer::Sha("abc".into()));
        assert_eq!(seen.answer("refs/heads/other"), Answer::Absent);
        assert_eq!(seen.answer("refs/convoy/target/7"), Answer::Miss);
    }

    /// sunfmin/control#828: one thread lists every fleet ref while another
    /// pins a target and reads it back; the listing, begun before the pin,
    /// answers after the read-back. The target is never read as absent.
    #[test]
    fn a_listing_begun_before_a_write_never_answers_for_the_ref_written() {
        let target = "refs/convoy/target/5";
        for patterns in [&["refs/convoy/*"][..], &["refs/convoy/target/5"], &["refs/*/x?"]] {
            let mut seen = Seen::default();
            let listing = seen.began();
            seen.wrote(&[target.to_string()]);
            assert_eq!(seen.answer(target), Answer::Miss);
            let read_back = seen.began();
            seen.remember(&[RemoteRecord::of(target, "t5")], &[target], read_back);
            assert_eq!(seen.answer(target), Answer::Sha("t5".into()));
            // the old listing, which did not see the pin
            seen.remember(&[], patterns, listing);
            assert_eq!(seen.answer(target), Answer::Sha("t5".into()), "{patterns:?}");
        }

        // Before any read-back, the old listing leaves the write to be asked.
        let mut seen = Seen::default();
        let listing = seen.began();
        seen.wrote(&[target.to_string()]);
        seen.remember(&[], &["refs/convoy/*"], listing);
        assert_eq!(seen.answer(target), Answer::Miss);
        seen.vanished(target, listing);
        assert_eq!(seen.answer(target), Answer::Miss);
        // Any other ref under the prefix is answered from the listing.
        assert_eq!(seen.answer("refs/convoy/claim/5"), Answer::Absent);

        // A listing begun after the write answers for it as ever.
        let after = seen.began();
        seen.remember(&[], &["refs/convoy/*"], after);
        assert_eq!(seen.answer(target), Answer::Absent);
    }

    /// The same race through the remote view and two threads: a listing
    /// whose remote answers only once a pin landed and was read back.
    #[test]
    fn a_slow_listing_beside_a_pin_leaves_the_pin_readable() {
        let fake = Fake::with(&[]);
        let view = remote(&fake);
        let target = "refs/convoy/target/5";
        let stall = Stall::new();
        lock(&fake.stalls).push_back(stall.clone());
        std::thread::scope(|scope| {
            let listing = scope.spawn(|| view.refs(&["refs/convoy/*"], Fresh::Live).unwrap());
            stall.read.wait();
            view.push(&Push::set("t5", target)).unwrap();
            assert_eq!(view.sha(target, Fresh::Process).unwrap().as_deref(), Some("t5"));
            stall.answer.wait();
            assert!(listing.join().unwrap().is_empty(), "the listing began before the pin");
        });
        assert_eq!(view.sha(target, Fresh::Process).unwrap().as_deref(), Some("t5"));
    }

    #[test]
    fn an_annotated_tag_is_on_the_commit_it_peels_to() {
        let listed = RemoteRecord::of;
        let tags = tags_of(&[
            listed("refs/tags/v0.5.15", "c15"),
            listed("refs/tags/v0.5.16", "t16"),
            listed("refs/tags/v0.5.16^{}", "c16"),
            listed("refs/heads/main", "m"),
        ]);
        let named = |name: &str, commit: &str| (name.to_string(), commit.to_string());
        assert_eq!(tags, [named("v0.5.15", "c15"), named("v0.5.16", "c16")]);
    }
}

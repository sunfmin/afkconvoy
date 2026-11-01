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
        let ran = crate::git::run(dir, &args, &[])?;
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

    /// As `try_push`, ending the push's process group at `limit`. Its remote
    /// outcome is then unknown, and the written refs are still dropped.
    pub fn try_push_within(&self, push: &Push, limit: Duration) -> Result<Ran> {
        self.try_push_bounded(push, Some(limit))
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

    /// Runs `push`, which must succeed.
    pub fn push(&self, push: &Push) -> Result<()> {
        let ran = self.try_push(push)?;
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
        let records: Vec<RemoteRecord> = listed
            .into_iter()
            .map(|(name, sha)| RemoteRecord { name, sha, subject: String::new() })
            .collect();
        lock(&shared.seen).remember(&records, patterns);
        Ok(records)
    }
}

/// A read changes nothing on the remote, so a failure in transit is safe to
/// ask again. Any answer from the remote itself fails at once.
fn connection_failure(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    [
        "tls connect error",
        "unexpected eof",
        "ssl_error_syscall",
        "connection reset",
        "connection refused",
        "could not resolve host",
        "timeout",
        "timed out",
    ]
    .iter()
    .any(|mark| said.contains(mark))
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
#[derive(Debug, Default)]
pub(crate) struct Seen {
    /// `Some` is the sha the remote had; `None` is a read that found it absent.
    sha: HashMap<String, Option<String>>,
    /// Glob prefixes (`refs/heads/`) for which every ref is in `sha` or absent.
    covered: Vec<String>,
    /// Written by this process since the read that filled `sha`.
    dirty: std::collections::HashSet<String>,
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

    fn clear(&mut self) {
        self.sha.clear();
        self.covered.clear();
        self.dirty.clear();
    }

    /// A write to the remote changed `refs`, each an exact name: the next
    /// read of each asks the remote. Other refs stay.
    fn wrote(&mut self, refs: &[String]) {
        for name in refs {
            self.sha.remove(name);
            self.dirty.insert(name.clone());
        }
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
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::path::PathBuf;

    /// A remote held in memory: its refs, and how many times it was listed.
    #[derive(Debug, Default)]
    struct Fake {
        refs: Mutex<BTreeMap<String, String>>,
        lists: Mutex<usize>,
        failures: Mutex<VecDeque<String>>,
    }

    impl Fake {
        fn with(refs: &[(&str, &str)]) -> Arc<Fake> {
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
    }

    impl Wire for Fake {
        fn list(&self, _: &Path, patterns: &[&str]) -> Result<Vec<(String, String)>> {
            *lock(&self.lists) += 1;
            if let Some(error) = lock(&self.failures).pop_front() {
                bail!(error);
            }
            let refs = lock(&self.refs);
            Ok(refs
                .iter()
                .filter(|(name, _)| {
                    patterns.iter().any(|pattern| match pattern.strip_suffix('*') {
                        Some(prefix) => name.starts_with(prefix),
                        None => name.as_str() == *pattern,
                    })
                })
                .map(|(name, sha)| (name.clone(), sha.clone()))
                .collect())
        }

        fn push(&self, _: &Path, push: &Push) -> Result<Ran> {
            let mut refs = lock(&self.refs);
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
        seen.remember(&[RemoteRecord::of("refs/heads/main", "abc")], &["refs/heads/*"]);
        assert_eq!(seen.answer("refs/heads/main"), Answer::Sha("abc".into()));
        assert_eq!(seen.answer("refs/heads/other"), Answer::Absent);
        assert_eq!(seen.answer("refs/convoy/target/7"), Answer::Miss);
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

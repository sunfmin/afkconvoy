//! git, as a subprocess: the checkout a command runs in and its one remote.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use afkconvoy_core::layout::Layout;
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
    /// Every copy of this value shares it, so git is asked each thing once.
    fixed: Arc<Fixed>,
}

#[derive(Debug, Default)]
struct Fixed {
    /// The repo's git dir (`common_dir`).
    common: OnceLock<PathBuf>,
    /// The empty tree's id in the repo's object format (`record_commit`).
    empty_tree: OnceLock<String>,
}

impl Repo {
    /// The checkout the command was run in. git is asked once in a process:
    /// every later call is a copy of the first one's answer.
    pub fn here() -> Result<Repo> {
        static HERE: OnceLock<Repo> = OnceLock::new();
        if let Some(repo) = HERE.get() {
            return Ok(repo.clone());
        }
        let repo = Repo::at(Path::new("."))?;
        Ok(HERE.get_or_init(|| repo).clone())
    }

    /// The checkout that contains `dir`.
    pub fn at(dir: &Path) -> Result<Repo> {
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
            let dir = self.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
            let _ = self.fixed.common.set(PathBuf::from(dir));
        }
        Ok(self.fixed.common.get().expect("set above"))
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
        let noted = self.note(LAYOUT_NOTE).and_then(|note| Ok(std::fs::read_to_string(note)?));
        noted.ok().and_then(|name| Layout::named(name.trim())).unwrap_or(Layout::ALL[0])
    }

    /// Notes the layout a start settled on.
    pub fn note_layout(&self, layout: Layout) -> Result<()> {
        if self.layout() != layout {
            let note = self.note(LAYOUT_NOTE)?;
            std::fs::write(&note, format!("{}\n", layout.name()))
                .with_context(|| format!("cannot write {}", note.display()))?;
        }
        Ok(())
    }

    /// Where a note called `name` is kept on this host: in the repo's git
    /// dir, which every worktree of it shares.
    pub fn note(&self, name: &str) -> Result<PathBuf> {
        Ok(self.common_dir()?.join(name))
    }

    /// One git call whose failure the caller reads.
    pub fn try_git(&self, args: &[&str]) -> Result<Ran> {
        run(&self.dir, args, &[])
    }

    /// One git call under extra environment, whose failure the caller reads.
    pub fn try_git_with(&self, args: &[&str], env: &[(&str, &str)]) -> Result<Ran> {
        run(&self.dir, args, env)
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
        let here = |repo: &Repo| -> Result<bool> {
            Ok(repo.try_git(&["cat-file", "-e", &format!("{sha}^{{commit}}")])?.ok)
        };
        if !here(self)? {
            self.git(&["fetch", "--quiet", "--no-write-fetch-head", "--no-tags", REMOTE, refname])?;
            if !here(self)? {
                bail!("{refname} moved on the remote while it was read — run this again");
            }
        }
        Ok(())
    }

    /// One git call that must succeed → its stdout as it came: for output whose
    /// leading spaces mean something.
    pub fn git_raw(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            .env("LC_ALL", "C")
            .output()
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

    /// The sha the remote has for one ref, `None` if it has none. An error when
    /// the remote cannot be read at all.
    pub fn remote_sha(&self, refname: &str) -> Result<Option<String>> {
        let found = self.listed(&[refname])?.into_iter().find(|listed| listed.name == refname);
        Ok(found.map(|listed| listed.sha))
    }

    /// Every ref the remote has that matches one of `patterns` — a ref's
    /// name, or `<namespace>/*` — in one call, with no record read yet. An
    /// error when the remote cannot be read.
    fn listed(&self, patterns: &[&str]) -> Result<Vec<RemoteRecord>> {
        let listed = self.git(&[&["ls-remote", REMOTE], patterns].concat())?;
        let rows = listed.lines().filter_map(|row| row.split_once('\t'));
        Ok(rows
            .map(|(sha, name)| RemoteRecord {
                name: name.to_string(),
                sha: sha.to_string(),
                subject: String::new(),
            })
            .collect())
    }

    /// The tip of the branch `base_branch` and every ref the fleet keeps, as
    /// ONE listing of the remote saw them: what a rebuild reads of the remote,
    /// all of it from one moment.
    pub fn listing(&self, base_branch: &str) -> Result<Listing> {
        let (every, base_ref) = (self.layout().every(""), format!("refs/heads/{base_branch}"));
        let mut records = self.listed(&[&every, &base_ref])?;
        let base = records.iter().find(|record| record.name == base_ref);
        let base = base.map(|record| record.sha.clone());
        records.retain(|record| record.name != base_ref);
        self.read_records(&mut records, &every)?;
        Ok(Listing { base_ref, base, records })
    }

    /// Every branch the remote has, by its short name. An error when the
    /// remote cannot be read.
    pub fn remote_heads(&self) -> Result<Vec<String>> {
        let listed = self.git(&["ls-remote", "--heads", REMOTE])?;
        let names = listed.lines().filter_map(|row| row.split_once('\t'));
        Ok(names
            .filter_map(|(_, name)| name.strip_prefix("refs/heads/"))
            .map(str::to_string)
            .collect())
    }

    /// Every ref the remote has that matches `pattern` (one ref's name, or
    /// `<namespace>/*`), each with the subject of the commit it points at. An
    /// error when the remote cannot be read.
    ///
    /// Nothing is written to this checkout's refs or its FETCH_HEAD: worktrees
    /// share both, and a worker's command may be reading the remote meanwhile.
    pub fn remote_records(&self, pattern: &str) -> Result<Vec<RemoteRecord>> {
        let mut records = self.listed(&[pattern])?;
        self.read_records(&mut records, pattern)?;
        Ok(records)
    }

    /// Reads the record each of `records` carries: the subject of its commit,
    /// fetched with everything `pattern` matches when it is not here yet.
    fn read_records(&self, records: &mut [RemoteRecord], pattern: &str) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        // The commits are usually here already: fetched by an earlier read.
        let subjects = match self.subjects(records)? {
            Some(subjects) => subjects,
            None => {
                self.git(&[
                    "fetch",
                    "--quiet",
                    "--no-write-fetch-head",
                    "--no-tags",
                    REMOTE,
                    pattern,
                ])?;
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
    /// in this repo yet.
    fn subjects(&self, records: &[RemoteRecord]) -> Result<Option<HashMap<String, String>>> {
        let mut args = vec!["log", "--no-walk=unsorted", "--format=%H%x09%s"];
        args.extend(records.iter().map(|record| record.sha.as_str()));
        let ran = self.try_git(&args)?;
        if !ran.ok {
            return Ok(None);
        }
        let rows = ran.stdout.lines().filter_map(|row| row.split_once('\t'));
        Ok(Some(rows.map(|(sha, subject)| (sha.to_string(), subject.to_string())).collect()))
    }
}

/// One ref of the remote and the record its commit carries.
#[derive(Debug, Clone)]
pub struct RemoteRecord {
    pub name: String,
    pub sha: String,
    pub subject: String,
}

/// The base branch's tip and every ref of the fleet, as one listing of the
/// remote saw them (`Repo::listing`).
#[derive(Debug, Clone)]
pub struct Listing {
    /// The base branch's ref.
    pub base_ref: String,
    /// Its tip; `None` when the remote has no such branch.
    pub base: Option<String>,
    /// Every ref of the fleet, with the record its commit carries.
    pub records: Vec<RemoteRecord>,
}

impl Listing {
    /// The sha the fleet's ref `refname` was at, `None` if the remote had none.
    pub fn sha(&self, refname: &str) -> Option<&str> {
        let found = self.records.iter().find(|record| record.name == refname);
        found.map(|record| record.sha.as_str())
    }
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
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("LC_ALL", "C")
        .envs(env.iter().copied())
        .output()
        .context("git could not run")?;
    Ok(Ran {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

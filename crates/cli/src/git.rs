//! git, as a subprocess: the checkout a command runs in and its one remote.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The remote every ref and branch is pushed to and fetched from.
pub const REMOTE: &str = "origin";

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
}

impl Repo {
    /// The checkout the command was run in.
    pub fn here() -> Result<Repo> {
        Repo::at(Path::new("."))
    }

    /// The checkout that contains `dir`.
    pub fn at(dir: &Path) -> Result<Repo> {
        let ran = run(dir, &["rev-parse", "--show-toplevel"], &[])?;
        if !ran.ok {
            bail!("{} is not in a git checkout", dir.display());
        }
        Ok(Repo { dir: PathBuf::from(ran.stdout) })
    }

    /// The main worktree of this checkout: where its `.git` directory is.
    pub fn main_checkout(&self) -> Result<PathBuf> {
        let common = self.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        let common = PathBuf::from(common);
        Ok(common.parent().map(Path::to_path_buf).unwrap_or(common))
    }

    /// One git call whose failure the caller reads.
    pub fn try_git(&self, args: &[&str]) -> Result<Ran> {
        run(&self.dir, args, &[])
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
        let tree = self.git(&["hash-object", "-t", "tree", "/dev/null"])?;
        self.record_commit_of(&tree, record)
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
        let listed = self.git(&["ls-remote", REMOTE, refname])?;
        Ok(listed.split_whitespace().next().map(str::to_string))
    }

    /// Every ref the remote has that matches `pattern` (one ref's name, or
    /// `<namespace>/*`), each with the subject of the commit it points at. An
    /// error when the remote cannot be read.
    ///
    /// Nothing is written to this checkout's refs or its FETCH_HEAD: worktrees
    /// share both, and a worker's command may be reading the remote meanwhile.
    pub fn remote_records(&self, pattern: &str) -> Result<Vec<RemoteRecord>> {
        let listed = self.git(&["ls-remote", REMOTE, pattern])?;
        let mut records: Vec<RemoteRecord> = listed
            .lines()
            .filter_map(|row| row.split_once('\t'))
            .map(|(sha, name)| RemoteRecord {
                name: name.to_string(),
                sha: sha.to_string(),
                subject: String::new(),
            })
            .collect();
        if records.is_empty() {
            return Ok(records);
        }
        // The commits are usually here already: fetched by an earlier read.
        let subjects = match self.subjects(&records)? {
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
                self.subjects(&records)?.unwrap_or_default()
            }
        };
        for record in &mut records {
            // A ref that moved between the listing and the fetch reads as a
            // record nobody can read; the next read sees it where it is.
            record.subject = subjects.get(&record.sha).cloned().unwrap_or_default();
        }
        Ok(records)
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

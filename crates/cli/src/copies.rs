//! The fleet's own copies of a team's directory and of each role's: one per
//! commit, read-only, kept on this host (docs/adr/0009). A worker starts in
//! one, so a push is the only way a change to a role reaches a worker, and
//! the checkout a human edits is another. A repo is fetched once per commit,
//! and each directory is taken out of that fetch on its own: a worker is
//! given its role's directory and nothing else of its repo (docs/adr/0014).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use afkconvoy_core::config::{RepoDir, RepoName};
use anyhow::{Context, Result, anyhow, bail};

use crate::gh::GitHub;
use crate::git::Repo;

/// Where on this host this user's fleets keep what they need not read again:
/// their cache directory, outside every checkout.
pub fn cache() -> Result<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let cache = match (set("XDG_CACHE_HOME"), set("HOME")) {
        (Some(cache), _) => PathBuf::from(cache),
        (None, Some(home)) => PathBuf::from(home).join(".cache"),
        (None, None) => bail!("neither XDG_CACHE_HOME nor HOME says where the fleet's cache is"),
    };
    Ok(cache.join("afkconvoy"))
}

/// Where on this host the copies are kept.
#[derive(Clone)]
pub struct Copies {
    root: PathBuf,
}

/// One repo as it stood at one commit, or one directory taken out of it:
/// plain files, with no git dir.
#[derive(Debug, Clone)]
pub struct Copy {
    pub dir: PathBuf,
    /// The repo's commit, whichever of its directories this is.
    pub commit: String,
}

impl Copies {
    /// This user's copies: in their cache directory, outside every checkout —
    /// a worker started in one finds no rules above it but its role's.
    pub fn here() -> Result<Copies> {
        Ok(Copies { root: cache()?.join("copies") })
    }

    /// The copy of `repo` at the head of its default branch
    /// now. One already here is used as it is: only the commit is asked for,
    /// and nothing is fetched.
    pub fn of(&self, gh: &GitHub, repo: &RepoName) -> Result<Copy> {
        let head = gh.head_commit(repo)?;
        let kept = self.at(repo, &head);
        if kept.dir.is_dir() {
            return Ok(kept);
        }
        self.fetch(gh, repo).with_context(|| format!("{repo} cannot be copied"))
    }

    fn at(&self, repo: &RepoName, commit: &str) -> Copy {
        Copy { dir: self.root.join(repo.as_str()).join(commit), commit: commit.to_string() }
    }

    /// The copy of `repo` at `commit`, where this host keeps one: nothing is
    /// asked of GitHub, and nothing fetched.
    pub fn kept(&self, repo: &RepoName, commit: &str) -> Option<Copy> {
        Some(self.at(repo, commit)).filter(|kept| kept.dir.is_dir())
    }

    /// The copy of the role's directory `dir` alone, taken out of `fetched`,
    /// the copy of its repo: what a worker in the role starts in. Nothing
    /// above the directory in its repo is in it or above it, and no
    /// neighbour's. A role that is a whole repo is `fetched` itself.
    pub fn role(&self, fetched: &Copy, dir: &RepoDir) -> Result<Copy> {
        self.take(fetched, dir, "role", &[])
    }

    /// The copy of the team's directory `dir`, taken out of `fetched`, the
    /// copy of its repo, without what is at each of `without`, a path from
    /// `dir`. A team that is a whole repo holding none of them is `fetched`
    /// itself. It is kept as a team with its rules: a copy an older afkconvoy
    /// kept, which left those out, is not taken for one.
    pub fn team(&self, fetched: &Copy, dir: &RepoDir, without: &[PathBuf]) -> Result<Copy> {
        self.take(fetched, dir, "team+rules", without)
    }

    /// The copy of `dir` out of `fetched`, leaving out each of `without`, a
    /// path from `dir`. It is kept beside `fetched`, named for the commit,
    /// for `kind` — what the directory is copied as, which says what is left
    /// out — and for its path: one already here is used as it is.
    fn take(&self, fetched: &Copy, dir: &RepoDir, kind: &str, without: &[PathBuf]) -> Result<Copy> {
        let from = fetched.dir.join(dir.path());
        let there = |path: &PathBuf| from.join(path).symlink_metadata().is_ok();
        if dir.path().is_empty() && !without.iter().any(there) {
            return Ok(fetched.clone());
        }
        let repo = self.root.join(dir.repo.as_str());
        // One name, not a path: no copy is inside another.
        let path = dir.path().replace('%', "%25").replace('/', "%2F");
        let name = format!("{}-{kind}-{path}", fetched.commit);
        let kept = Copy { dir: repo.join(&name), commit: fetched.commit.clone() };
        if kept.dir.is_dir() {
            return Ok(kept);
        }
        let scratch = repo.join(format!("taking-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let taken = copy_dir(&from, &scratch, Path::new(""), without)
            .and_then(|()| keep_whole(&scratch, &kept.dir))
            .with_context(|| format!("{dir} cannot be copied"));
        let _ = std::fs::remove_dir_all(&scratch);
        taken.map(|()| kept)
    }

    /// Clones the head of `repo` and keeps its files, named for the commit
    /// the clone has: the branch may have moved since it was asked about.
    fn fetch(&self, gh: &GitHub, repo: &RepoName) -> Result<Copy> {
        let scratch =
            self.root.join(repo.as_str()).join(format!("fetching-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let (clone, files) = (scratch.join("git"), scratch.join("files"));
        std::fs::create_dir_all(&files)
            .with_context(|| format!("cannot create {}", files.display()))?;
        let copied = (|| {
            gh.clone_head(repo, &clone)?;
            let clone = Repo { dir: clone };
            let head = clone.try_git(&["rev-parse", "--quiet", "--verify", "HEAD^{commit}"])?;
            if !head.ok {
                bail!("it has no commit on its default branch");
            }
            let kept = self.at(repo, &head.stdout);
            if !kept.dir.is_dir() {
                let into = files.to_str().ok_or_else(|| anyhow!("a path that is not text"))?;
                clone.git(&[
                    "--work-tree",
                    into,
                    "checkout",
                    "--quiet",
                    "--force",
                    &kept.commit,
                ])?;
                keep_whole(&files, &kept.dir)?;
            }
            Ok(kept)
        })();
        let _ = std::fs::remove_dir_all(&scratch);
        copied
    }
}

/// Whether `dir` holds the same in `this` copy of its repo as in `that` one,
/// each of `without` — a path from `dir` — left out of both: the same files,
/// each with the same text and as much a program, and the same symlinks. A
/// `dir` that is no directory of one of them is not the same.
pub fn alike(this: &Copy, that: &Copy, dir: &RepoDir, without: &[PathBuf]) -> Result<bool> {
    let (this, that) = (this.dir.join(dir.path()), that.dir.join(dir.path()));
    Ok(this.is_dir() && that.is_dir() && alike_dir(&this, &that, Path::new(""), without)?)
}

/// `alike`, of what is at `under` of the directories `this` and `that`.
fn alike_dir(this: &Path, that: &Path, under: &Path, without: &[PathBuf]) -> Result<bool> {
    let names = |root: &Path| -> Result<Vec<std::ffi::OsString>> {
        let dir = root.join(under);
        let mut names = vec![];
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("cannot list {}", dir.display()))?
        {
            let name = entry?.file_name();
            if !without.contains(&under.join(&name)) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    };
    let held = names(this)?;
    if held != names(that)? {
        return Ok(false);
    }
    for name in held {
        let path = under.join(name);
        let (one, other) = (this.join(&path), that.join(&path));
        // A file's kind, and whether it is a program: a copy is read-only
        // whatever else its mode was.
        let found = |file: &Path| {
            let found = std::fs::symlink_metadata(file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            anyhow::Ok((found.file_type(), found.permissions().mode() & 0o111))
        };
        let (kind, other_kind) = (found(&one)?, found(&other)?);
        let same = if kind != other_kind {
            false
        } else if kind.0.is_dir() {
            alike_dir(this, that, &path, without)?
        } else if kind.0.is_symlink() {
            std::fs::read_link(&one)? == std::fs::read_link(&other)?
        } else {
            let text = |file: &Path| {
                std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))
            };
            text(&one)? == text(&other)?
        };
        if !same {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Keeps the directory `files` as `kept`, whole or not there: moved into place
/// in one step, then closed. Another start may have kept the same meanwhile:
/// the two are alike, and `files` is left where it is.
fn keep_whole(files: &Path, kept: &Path) -> Result<()> {
    match std::fs::rename(files, kept) {
        Ok(()) => read_only(kept),
        Err(_) if kept.is_dir() => Ok(()),
        Err(error) => bail!("cannot keep it at {}: {error}", kept.display()),
    }
}

/// Copies what is at `under` of the directory `from` to the same place of
/// `to`, which is made: every file, with its mode, and every symlink as the
/// link it is — but none of `without`, each a path from `from`.
fn copy_dir(from: &Path, to: &Path, under: &Path, without: &[PathBuf]) -> Result<()> {
    let (source, made) = (from.join(under), to.join(under));
    std::fs::create_dir_all(&made).with_context(|| format!("cannot create {}", made.display()))?;
    for entry in
        std::fs::read_dir(&source).with_context(|| format!("cannot list {}", source.display()))?
    {
        let entry = entry?;
        let path = under.join(entry.file_name());
        if without.contains(&path) {
            continue;
        }
        let (kind, copied) = (entry.file_type()?, to.join(&path));
        if kind.is_dir() {
            copy_dir(from, to, &path, without)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, &copied)
                .with_context(|| format!("cannot link {}", copied.display()))?;
        } else {
            std::fs::copy(entry.path(), &copied)
                .with_context(|| format!("cannot copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Takes the write permission off `path` and everything under it. A symlink
/// is left as it is: its mode is its target's.
fn read_only(path: &Path) -> Result<()> {
    let found = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    if found.file_type().is_symlink() {
        return Ok(());
    }
    if found.is_dir() {
        for entry in
            std::fs::read_dir(path).with_context(|| format!("cannot list {}", path.display()))?
        {
            read_only(&entry?.path())?;
        }
    }
    let mode = found.permissions().mode() & !0o222;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set the mode of {}", path.display()))
}

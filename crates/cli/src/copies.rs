//! The fleet's own copies of a team's repo and of each role's: one per commit,
//! read-only, kept on this host (docs/adr/0009). A worker starts in one, so a
//! push is the only way a change to a role reaches a worker, and the checkout
//! a human edits is another.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use afkconvoy_core::config::RepoName;
use anyhow::{Context, Result, anyhow, bail};

use crate::gh::GitHub;
use crate::git::Repo;

/// Where on this host the copies are kept.
pub struct Copies {
    root: PathBuf,
}

/// One repo as it stood at one commit: plain files, with no git dir.
#[derive(Debug, Clone)]
pub struct Copy {
    pub dir: PathBuf,
    pub commit: String,
}

impl Copies {
    /// This user's copies: in their cache directory, outside every checkout —
    /// a worker started in one finds no rules above it but its role's.
    pub fn here() -> Result<Copies> {
        let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let cache = match (set("XDG_CACHE_HOME"), set("HOME")) {
            (Some(cache), _) => PathBuf::from(cache),
            (None, Some(home)) => PathBuf::from(home).join(".cache"),
            (None, None) => bail!("neither XDG_CACHE_HOME nor HOME says where copies are kept"),
        };
        Ok(Copies { root: cache.join("afkconvoy").join("copies") })
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
                // Whole or not there: moved into place in one step, then closed.
                // Another start may have kept the same commit meanwhile: the two are alike.
                match std::fs::rename(&files, &kept.dir) {
                    Ok(()) => read_only(&kept.dir)?,
                    Err(_) if kept.dir.is_dir() => {}
                    Err(error) => bail!("cannot keep it at {}: {error}", kept.dir.display()),
                }
            }
            Ok(kept)
        })();
        let _ = std::fs::remove_dir_all(&scratch);
        copied
    }
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

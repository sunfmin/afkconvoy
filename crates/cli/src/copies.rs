//! The fleet's own copies of a team's directory and of each role's: one per
//! commit, read-only, kept on this host (docs/adr/0009). A worker starts in
//! one, so a push is the only way a change to a role reaches a worker, and
//! the checkout a human edits is another. A repo is fetched once per commit,
//! and each directory is taken out of that fetch on its own: a worker is
//! given its role's directory and nothing else of its repo (docs/adr/0014).
//! What a harness needs in the directory it starts in, and a role does not
//! hold for it, is in the copy that harness's workers start in: the fleet's,
//! and never the role's repo's (docs/adr/0013).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use afkconvoy_core::config::{RepoDir, RepoName};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::team::{RULES, SKILLS};
use anyhow::{Context, Result, anyhow, bail};

use crate::gh::GitHub;
use crate::git::Repo;

/// Where on this host this user's fleets keep what they need not read again:
/// their cache directory, outside every checkout.
pub fn cache() -> Result<PathBuf> {
    Ok(cache_home()?.join("afkconvoy"))
}

/// The directory the cache directory is in: this user's, as XDG_CACHE_HOME
/// or HOME says it.
#[cfg(not(test))]
fn cache_home() -> Result<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    Ok(match (set("XDG_CACHE_HOME"), set("HOME")) {
        (Some(cache), _) => PathBuf::from(cache),
        (None, Some(home)) => PathBuf::from(home).join(".cache"),
        (None, None) => bail!(
            "neither XDG_CACHE_HOME nor HOME says where the fleet's cache directory is: \
             `afkconvoy help cache`"
        ),
    })
}

/// The same, of a unit test: one of its own, made as it first asks and gone
/// once it ends — no test reads or writes the cache of whoever runs it. Each
/// thread has its own, and a test runs on one of its own.
#[cfg(test)]
fn cache_home() -> Result<PathBuf> {
    thread_local! {
        static OWN: tempfile::TempDir = tempfile::tempdir().expect("a unit test's own cache");
    }
    Ok(OWN.with(|own| own.path().to_path_buf()))
}

/// Where on this host the copies are kept.
#[derive(Clone)]
pub struct Copies {
    root: PathBuf,
}

/// One repo as it stood at one commit, or one directory taken out of it:
/// plain files, with no Git metadata. A Claude role has an empty `.git`
/// directory solely as its native discovery boundary.
#[derive(Debug, Clone)]
pub struct Copy {
    pub dir: PathBuf,
    /// The repo's commit, whichever of its directories this is.
    pub commit: String,
}

impl Copies {
    /// This user's copies on this host, whichever repo a command is about: in
    /// their cache directory, outside every checkout — a worker started in
    /// one finds no rules above it but its role's.
    pub fn of_host() -> Result<Copies> {
        Ok(Self::under(&cache()?))
    }

    /// The same copies under an explicitly named fleet cache.
    pub fn under(cache: &Path) -> Copies {
        Copies { root: cache.join("copies") }
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
    /// the copy of its repo: what a worker of `harness` in the role starts
    /// in. Nothing above the directory in its repo is in it or above it, and
    /// no neighbour's. Where `harness` needs a link to the role's skills or
    /// shared rules, the copy is that harness's and holds it (`role_links`),
    /// made once with the copy. Claude Code's copy also holds a discovery
    /// boundary: its project skill loader must not walk into host ancestors.
    /// Otherwise it is every harness's, and a whole repo is `fetched` itself.
    pub fn role(&self, fetched: &Copy, dir: &RepoDir, harness: Harness) -> Result<Copy> {
        let links = role_links(&fetched.dir.join(dir.path()), harness);
        let rooted = harness == Harness::ClaudeCode;
        if links.is_empty() && !rooted {
            self.take(fetched, dir, "role", &[], &[], false)
        } else {
            self.take(fetched, dir, &format!("role+{}", harness.key()), &[], &links, rooted)
        }
    }

    /// The copy of the team's directory `dir`, taken out of `fetched`, the
    /// copy of its repo, without what is at each of `without`, a path from
    /// `dir`. A team that is a whole repo holding none of them is `fetched`
    /// itself. It is kept as a team with its rules: a copy an older afkconvoy
    /// kept, which left those out, is not taken for one.
    pub fn team(&self, fetched: &Copy, dir: &RepoDir, without: &[PathBuf]) -> Result<Copy> {
        self.take(fetched, dir, "team+rules", without, &[], false)
    }

    /// The copy of `dir` out of `fetched`, leaving out each of `without`, a
    /// path from `dir`, and with each of `links` added. It is kept beside
    /// `fetched`, named for the commit, for `kind` — what the directory is
    /// copied as, which says what is left out and what is added — and for
    /// its path: one already here is used as it is. `rooted` adds a native
    /// discovery boundary before the copy is closed.
    fn take(
        &self,
        fetched: &Copy,
        dir: &RepoDir,
        kind: &str,
        without: &[PathBuf],
        links: &[Link],
        rooted: bool,
    ) -> Result<Copy> {
        let from = fetched.dir.join(dir.path());
        let there = |path: &PathBuf| from.join(path).symlink_metadata().is_ok();
        if dir.path().is_empty() && !without.iter().any(there) && links.is_empty() && !rooted {
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
        let scratch = repo.join(format!("taking-{}-{name}", crate::unique()));
        let _ = std::fs::remove_dir_all(&scratch);
        let taken = copy_dir(&from, &scratch, Path::new(""), without)
            .and_then(|()| links.iter().try_for_each(|link| link.add(&scratch)))
            .and_then(
                |()| if rooted { discovery_root(&scratch).map_err(Into::into) } else { Ok(()) },
            )
            .and_then(|()| keep_whole(&scratch, &kept.dir))
            .with_context(|| format!("{dir} cannot be copied"));
        let _ = std::fs::remove_dir_all(&scratch);
        taken.map(|()| kept)
    }

    /// Clones the head of `repo` and keeps its files, named for the commit
    /// the clone has: the branch may have moved since it was asked about.
    fn fetch(&self, gh: &GitHub, repo: &RepoName) -> Result<Copy> {
        let scratch = self.root.join(repo.as_str()).join(format!("fetching-{}", crate::unique()));
        let _ = std::fs::remove_dir_all(&scratch);
        let (clone, files) = (scratch.join("git"), scratch.join("files"));
        std::fs::create_dir_all(&files)
            .with_context(|| format!("cannot create {}", files.display()))?;
        let copied = (|| {
            gh.clone_head(repo, &clone)?;
            let clone = Repo::new(clone);
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

/// Stops Claude Code's native project discovery at the role. Its loader
/// recognises a `.git` entry before walking to a parent; this empty directory
/// carries no repository, hooks, config or history. Role settings and skills
/// still load natively, and opted-in host skills arrive through `--add-dir`.
fn discovery_root(role: &Path) -> std::io::Result<()> {
    std::fs::create_dir(role.join(".git"))
}

/// A link the fleet adds to a copy of a directory.
struct Link {
    /// Where it is: a path from the directory.
    at: PathBuf,
    /// What it points at: a path from where it is, to something of the
    /// directory, so that it holds wherever the copy is kept.
    to: PathBuf,
}

impl Link {
    /// Adds it to the copy being made at `copy`.
    fn add(&self, copy: &Path) -> Result<()> {
        let at = copy.join(&self.at);
        let made = at.parent().map_or(Ok(()), std::fs::create_dir_all);
        made.and_then(|()| std::os::unix::fs::symlink(&self.to, &at))
            .with_context(|| format!("cannot link {}", at.display()))
    }
}

/// The directories where `harness` finds skills by itself (`Harness::skills`)
/// that the role's directory `root` holds: what a worker of it is shown in
/// place of the role's skills, and the fleet leaves as it is.
pub fn own_skills(root: &Path, harness: Harness) -> Vec<&'static str> {
    let held = |dir: &&str| root.join(dir).symlink_metadata().is_ok();
    harness.skills().iter().copied().filter(held).collect()
}

/// The link that has `harness` find the skills of the role's directory
/// `root`: from the first directory it finds skills in to the role's own.
/// `None` where it finds a role's skills without one, where the role holds
/// such a directory itself (`own_skills`), and where it holds no skills.
fn skills_link(root: &Path, harness: Harness) -> Option<Link> {
    let at = Path::new(harness.skills().first()?);
    if !own_skills(root, harness).is_empty() || !root.join(SKILLS).is_dir() {
        return None;
    }
    let up = at.components().skip(1).map(|_| "..");
    Some(Link { at: at.to_path_buf(), to: up.collect::<PathBuf>().join(SKILLS) })
}

/// Every link a harness needs in its copy of the role.
fn role_links(root: &Path, harness: Harness) -> Vec<Link> {
    let mut links = skills_link(root, harness).into_iter().collect::<Vec<_>>();
    if let Some(rules) = harness.rules_link()
        && root.join(RULES).is_file()
        && root.join(rules).symlink_metadata().is_err()
    {
        links.push(Link { at: PathBuf::from(rules), to: PathBuf::from(RULES) });
    }
    links
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

/// Gives the owner back the write permission on every directory under `path`,
/// so that it can be removed: a copy is read-only, directories included, and
/// a test killed before it ends leaves the copies it made where it ran. A
/// symlink is left as it is, and nothing it names is opened. Best effort: what
/// cannot be opened, the removal that follows fails on and says.
pub fn removable(path: &Path) {
    let Ok(found) = std::fs::symlink_metadata(path) else { return };
    if !found.is_dir() {
        return;
    }
    let mode = found.permissions().mode();
    if (mode & 0o200) == 0 {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o200));
    }
    for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            removable(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    #[test]
    fn a_unit_test_has_a_cache_of_its_own_and_never_that_of_whoever_runs_it() {
        let own = cache().unwrap();
        let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let host = [set("XDG_CACHE_HOME").map(PathBuf::from), set("HOME").map(PathBuf::from)];
        for host in host.into_iter().flatten() {
            assert_ne!(own, host.join("afkconvoy"));
            assert_ne!(own, host.join(".cache/afkconvoy"));
        }
        assert_eq!(cache().unwrap(), own, "the same each time it is asked");
        let other = std::thread::spawn(|| cache().unwrap()).join().unwrap();
        assert_ne!(other, own, "another thread's is another");
        assert!(!other.parent().unwrap().exists(), "and gone once its thread ended");
    }

    #[test]
    fn removable_gives_the_owner_write_and_clears_no_other_mode_bit() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("afkconvoy-removable-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("afkconvoy-removable-out-{}", std::process::id()));
        let _root = Clean(&root);
        let _outside = Clean(&outside);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        let nested = root.join("sealed/nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("f"), "role").unwrap();
        std::fs::set_permissions(nested.join("f"), std::fs::Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o2555)).unwrap();
        std::fs::set_permissions(root.join("sealed"), std::fs::Permissions::from_mode(0o1555))
            .unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o555)).unwrap();
        symlink(&outside, root.join("link")).unwrap();

        removable(&root);

        assert_eq!(mode(&root.join("sealed")), 0o1755);
        assert_eq!(mode(&nested), 0o2755);
        assert_eq!(mode(&nested.join("f")), 0o444);
        assert_eq!(mode(&outside), 0o555, "a symlink is not opened");
    }

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
    }

    /// Gives the owner write back so the test's directory can be removed.
    struct Clean<'a>(&'a Path);

    impl Drop for Clean<'_> {
        fn drop(&mut self) {
            removable(self.0);
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_dir_all(self.0);
        }
    }
}

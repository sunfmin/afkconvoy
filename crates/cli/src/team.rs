//! Reads a team — its own files, and the repo of each role it names — for the
//! core to load (`afkconvoy_core::team`), and answers `check` with what was
//! read or with every fault. `load` is how any command comes by a team.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use afkconvoy_core::team::{
    self, Fault, File, Node, PATROLS, Pass, Patrol, Performer, RoleRepo, SKILLS, Sources, Stage,
    TEAM_FILE, Team, WORKFLOWS, Work, Workflow,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config;
use crate::copies::{Copies, Copy};
use crate::gh::GitHub;
use crate::git::Repo;

/// Where a team's own files are taken from. Its roles' repos are always read
/// from GitHub.
pub enum From {
    /// The team the config of this checkout names.
    Config,
    /// A team's repo on GitHub, as `owner/repo`.
    Repo(String),
    /// A directory holding a team's files, read as they are, committed or not.
    Dir(PathBuf),
}

/// A team that does not load: every fault it has.
#[derive(Debug)]
pub struct Refused {
    pub team: String,
    pub faults: Vec<Fault>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.faults.len();
        let faults = if count == 1 { "1 fault".to_string() } else { format!("{count} faults") };
        write!(f, "the team {} has {faults}", self.team)?;
        for fault in &self.faults {
            write!(f, "\n  {fault}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Refused {}

impl Refused {
    /// The faults, each as `{file, key, problem}`.
    pub fn json(&self) -> Value {
        let fault =
            |fault: &Fault| json!({"file": fault.file, "key": fault.key, "problem": fault.problem});
        self.faults.iter().map(fault).collect()
    }
}

/// Why a repo's `patrols` cannot be run: each name among `unknown` that
/// `team` has no patrol for, as a fault of the repo's config.
pub fn unknown_patrols(team: &str, has: &Team, unknown: &[String]) -> Refused {
    let names: Vec<&str> = has.patrols.iter().map(|patrol| patrol.name.as_str()).collect();
    let has = if names.is_empty() { "none".to_string() } else { names.join(", ") };
    let fault = |name: &String| Fault {
        file: config::FILE.to_string(),
        key: "patrols".to_string(),
        problem: format!(
            "`{name}` is no patrol of the team {team}: it has {has}. A repo names patrols its \
             team has, each a file {PATROLS}/<name>.md there"
        ),
    };
    Refused { team: team.to_string(), faults: unknown.iter().map(fault).collect() }
}

/// A directory this command clones into, removed when it is done.
struct Scratch {
    dir: PathBuf,
    used: std::cell::Cell<u32>,
}

impl Scratch {
    fn new() -> Scratch {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let name = format!("afkconvoy-check-{}-{at}", std::process::id());
        Scratch { dir: std::env::temp_dir().join(name), used: 0.into() }
    }

    /// A path in it nothing has been put at.
    fn next(&self) -> PathBuf {
        self.used.set(self.used.get() + 1);
        self.dir.join(self.used.get().to_string())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Files to read: a directory as it is, or a repo at one commit.
enum Tree {
    Dir(PathBuf),
    Commit(Repo, String),
}

impl Tree {
    /// `repo` on GitHub at the head of its default branch.
    fn head(gh: &GitHub, repo: &str, scratch: &Scratch) -> Result<Tree> {
        let into = scratch.next();
        std::fs::create_dir_all(&scratch.dir)
            .with_context(|| format!("cannot create {}", scratch.dir.display()))?;
        gh.clone_head(repo, &into)?;
        let clone = Repo { dir: into };
        let head = clone.try_git(&["rev-parse", "--quiet", "--verify", "HEAD^{commit}"])?;
        if !head.ok {
            bail!("it has no commit on its default branch");
        }
        Ok(Tree::Commit(clone, head.stdout))
    }

    fn commit(&self) -> Option<&str> {
        match self {
            Tree::Dir(_) => None,
            Tree::Commit(_, commit) => Some(commit),
        }
    }

    /// The names of the entries of the directory `dir`, sorted; none when
    /// there is no such directory.
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        let mut names: Vec<String> = match self {
            Tree::Dir(top) => match std::fs::read_dir(top.join(dir)) {
                Ok(entries) => entries
                    .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
                    .collect::<Result<_>>()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
                Err(error) => bail!("cannot list {dir}: {error}"),
            },
            Tree::Commit(repo, commit) => {
                let under = format!("{dir}/");
                let listed = repo.git_raw(&["ls-tree", "-z", "--name-only", commit, &under])?;
                let names = listed.split('\0').filter_map(|path| path.strip_prefix(&under));
                names.map(str::to_string).collect()
            }
        };
        names.sort();
        Ok(names)
    }

    /// The text of the file at `path`; `None` when no file is there.
    fn read(&self, path: &str) -> Result<Option<String>> {
        match self {
            Tree::Dir(top) => {
                let file = top.join(path);
                if !file.is_file() {
                    return Ok(None);
                }
                let text = std::fs::read_to_string(&file);
                Ok(Some(text.with_context(|| format!("cannot read {}", file.display()))?))
            }
            Tree::Commit(repo, commit) => {
                let object = format!("{commit}:{path}");
                if repo.try_git(&["cat-file", "-t", &object])?.stdout != "blob" {
                    return Ok(None);
                }
                Ok(Some(repo.git_raw(&["show", &object])?))
            }
        }
    }

    /// The definition file at `path`: the first yaml block of its markdown.
    fn file(&self, path: &str) -> File {
        let yaml = match self.read(path) {
            Ok(Some(markdown)) => yaml(&markdown),
            Ok(None) => Err("is not there, or is no file".to_string()),
            Err(error) => Err(format!("{error:#}")),
        };
        File { path: path.to_string(), yaml }
    }
}

fn yaml(markdown: &str) -> Result<Node, String> {
    let block = config::yaml_block(markdown).ok_or("has no ```yaml block")?;
    serde_yaml::from_str(&block).map_err(|error| format!("its yaml cannot be read: {error}"))
}

/// A team that loaded, and what it was read from.
pub struct Loaded {
    /// What the team was named by: its repo, or its directory.
    pub name: String,
    /// The commit its own files were read at; `None` for a directory.
    pub commit: Option<String>,
    /// The definition files read, the team file first.
    pub files: Vec<String>,
    /// The patrols the repo of this checkout names, when it is its team that
    /// was read: the ones that run on it.
    pub runs: Vec<String>,
    pub team: Team,
}

/// Reads the team `from` names and every role's repo, and loads it → what was
/// read. A team with a fault fails with `Refused`.
pub fn check(from: From) -> Result<Value> {
    Ok(answer(&load(from)?))
}

/// Reads the team `from` names and every role's repo, and loads it. A team
/// with a fault fails with `Refused`.
pub fn load(from: From) -> Result<Loaded> {
    let scratch = Scratch::new();
    let gh = GitHub { dir: PathBuf::from(".") };
    // The patrols this checkout's repo names, when it is its team that is read.
    let mut named: Vec<String> = vec![];
    let (name, tree) = match from {
        From::Dir(dir) => {
            if !dir.is_dir() {
                bail!("{} is not a directory", dir.display());
            }
            (dir.display().to_string(), Tree::Dir(dir))
        }
        From::Repo(repo) => {
            let tree = Tree::head(&gh, &repo, &scratch);
            (repo.clone(), tree.with_context(|| format!("the team {repo} cannot be read"))?)
        }
        From::Config => {
            let config = config::load(&config::default_file()?)?;
            let Some(bound) = config.bound else {
                bail!(
                    "this repo is unbound: its config names no team. Name one with --team, \
                     or a directory of a team's files with --dir"
                );
            };
            let tree = Tree::head(&gh, &bound.team, &scratch);
            let tree = tree.with_context(|| format!("the team {} cannot be read", bound.team))?;
            named = bound.patrols;
            (bound.team, tree)
        }
    };
    let sources = read(&tree, |repo| {
        let tree = Tree::head(&gh, repo, &scratch)?;
        let commit = tree.commit().unwrap_or_default().to_string();
        Ok(RoleRepo { commit, skills: tree.list(SKILLS)? })
    })?;
    let mut files = vec![sources.team.path.clone()];
    files.extend(sources.workflows.iter().map(|file| file.path.clone()));
    files.extend(sources.patrols.iter().map(|file| file.path.clone()));
    let team = match team::load(&sources) {
        Ok(team) => team,
        Err(faults) => return Err(Refused { team: name, faults }.into()),
    };
    if let Err(unknown) = team.patrols_named(&named) {
        return Err(unknown_patrols(&name, &team, &unknown).into());
    }
    Ok(Loaded { commit: tree.commit().map(str::to_string), name, files, runs: named, team })
}

/// Everything the core loads a team from: the team's own files out of
/// `tree`, and of each role's repo what `role` reads.
fn read(tree: &Tree, role: impl Fn(&str) -> Result<RoleRepo>) -> Result<Sources> {
    let team = tree.file(TEAM_FILE);
    let entries = tree.list(WORKFLOWS)?;
    let workflows = entries.iter().map(|name| tree.file(&format!("{WORKFLOWS}/{name}"))).collect();
    let entries = tree.list(PATROLS)?;
    let patrols = entries.iter().map(|name| tree.file(&format!("{PATROLS}/{name}"))).collect();
    let repos = team::role_repos(&team).into_iter();
    let repos = repos.map(|repo| {
        let read = role(&repo).map_err(|error| format!("{error:#}"));
        (repo, read)
    });
    Ok(Sources { team, workflows, patrols, repos: repos.collect() })
}

/// A team as a stage starts under it: loaded from the fleet's copies, at the
/// commits those were taken at.
pub struct Kept {
    pub team: Team,
    /// The copy of the team's own repo.
    pub copy: Copy,
    /// The copy of each role's repo, by the repo's name.
    roles: Vec<(String, Copy)>,
}

impl Kept {
    /// The copy of the repo of `role`.
    pub fn role(&self, role: &team::Role) -> Result<&Copy> {
        let kept =
            self.roles.iter().find(|(repo, copy)| *repo == role.repo && copy.commit == role.commit);
        kept.map(|(_, copy)| copy).ok_or_else(|| anyhow!("no copy of {} was kept", role.repo))
    }
}

/// Loads the team `repo` names as it stands now: the head of its repo and of
/// each role's, each read from the fleet's copy of that commit and fetched
/// only when no copy of it is here. A team with a fault fails with `Refused`.
pub fn keep(gh: &GitHub, copies: &Copies, repo: &str) -> Result<Kept> {
    let copy = copies.of(gh, repo).with_context(|| format!("the team {repo} cannot be read"))?;
    let roles = std::cell::RefCell::new(vec![]);
    let sources = read(&Tree::Dir(copy.dir.clone()), |role| {
        let kept = copies.of(gh, role)?;
        let skills = Tree::Dir(kept.dir.clone()).list(SKILLS)?;
        let commit = kept.commit.clone();
        roles.borrow_mut().push((role.to_string(), kept));
        Ok(RoleRepo { commit, skills })
    })?;
    match team::load(&sources) {
        Ok(team) => Ok(Kept { team, copy, roles: roles.into_inner() }),
        Err(faults) => Err(Refused { team: repo.to_string(), faults }.into()),
    }
}

fn answer(loaded: &Loaded) -> Value {
    let team = &loaded.team;
    let roles: Vec<Value> = team
        .roles
        .iter()
        .map(|role| {
            json!({"name": role.name, "repo": role.repo, "commit": role.commit,
                "skills": role.skills})
        })
        .collect();
    json!({
        "team": loaded.name,
        "commit": loaded.commit,
        "files": loaded.files,
        "escalate_label": team.escalate_label,
        "roles": roles,
        "workflows": team.workflows.iter().map(workflow).collect::<Vec<_>>(),
        "patrols": team.patrols.iter().map(|p| patrol(p, &loaded.runs)).collect::<Vec<_>>(),
        "precedents": team.precedents.as_ref().map(|precedents| {
            json!({"by": precedents.work.role, "do": precedents.work.skill,
                "max_open": precedents.max_open})
        }),
    })
}

/// One patrol, and whether it is among `runs`: the ones the repo names.
fn patrol(patrol: &Patrol, runs: &[String]) -> Value {
    json!({
        "name": patrol.name,
        "by": patrol.work.role,
        "do": patrol.work.skill,
        "every": patrol.every,
        "model": patrol.model,
        "max_open": patrol.max_open,
        "runs_here": runs.contains(&patrol.name),
    })
}

fn work(work: &Work) -> Value {
    json!({"by": work.role, "do": work.skill})
}

fn workflow(workflow: &Workflow) -> Value {
    let name = |at: usize| workflow.stage(at).name.clone();
    let stage = |stage: &Stage| {
        let (by, skill) = match &stage.by {
            Performer::Human => (team::HUMAN, None),
            Performer::Role(work) => (work.role.as_str(), Some(&work.skill)),
        };
        let pass = match stage.pass() {
            Pass::Stage(at) => name(at),
            Pass::Land => team::LAND.to_string(),
            Pass::Done => team::DONE.to_string(),
        };
        json!({
            "name": stage.name,
            "by": by,
            "do": skill,
            "model": stage.model,
            "writes": stage.writes,
            "escalate": stage.consult.as_ref().map(|consult| json!({"to": consult.role, "do": consult.skill})),
            "pass": pass,
            "reject": stage.reject().map(name),
        })
    };
    json!({
        "name": workflow.name,
        "flow_label": workflow.flow_label(),
        "max_rework": workflow.max_rework,
        "max_consecutive_rework": workflow.max_consecutive_rework,
        "stages": workflow.stages().iter().map(stage).collect::<Vec<_>>(),
        "land": workflow.land.as_ref().map(work),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_is_the_first_yaml_block_of_its_markdown() {
        let read = yaml("# Feature\n\n```yaml\nmax_rework: 1\non: { pass: done }\n```\n").unwrap();
        let Node::Map(entries) = read else { panic!("a map") };
        assert_eq!(entries[0], ("max_rework".to_string(), Node::Int(1)));
        // `on` is a key, not yaml's old word for true.
        assert_eq!(entries[1].0, "on");
        assert_eq!(yaml("stages: []").unwrap_err(), "has no ```yaml block");
        assert!(yaml("```yaml\nstages: [\n```\n").unwrap_err().contains("its yaml cannot be read"));
        assert_eq!(yaml("```yaml\n```\n"), Ok(Node::Null));
    }
}

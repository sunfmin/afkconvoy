//! `scaffold`: writes the skeleton of a team with its roles, or of one role,
//! or a whole example team — files `check` accepts as they are — and, when
//! asked, creates the repo on GitHub and pushes what it wrote to it.

use std::path::{Component, Path, PathBuf};

use afkconvoy_core::config::{RepoDir, RepoName};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::team::{HUMAN, RULES, SKILLS, TEAM_FILE, WORKFLOWS};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::Repo;

/// The one skill a role's skeleton holds, and the one a team's example
/// workflow asks of its role: the two skeletons fit because both name this.
pub const EXAMPLE_SKILL: &str = "implement";
/// The example workflow of a team's skeleton.
pub const EXAMPLE_WORKFLOW: &str = "feature";
/// The harnesses a role's skeleton holds a file of limits for, each with that
/// file, which names no hook yet: the ones whose limits are one file. pi's
/// are the entries of a directory, and a skeleton writes no empty one.
const LIMITS: [(Harness, &str); 2] = [
    (Harness::ClaudeCode, include_str!("scaffold/settings.json")),
    (Harness::Codex, include_str!("scaffold/hooks.json")),
];
/// Where a team's skeleton writes the roles it holds, in the team's directory.
pub const ROLES: &str = "roles";

/// What a skeleton is of.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum What {
    Team,
    Role,
}

/// A role a team's skeleton names: its short name there, and the directory
/// it is — `None` for one the skeleton itself writes, under `roles/<name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    pub at: Option<RepoDir>,
}

/// Reads `<name>` or `<name>=<owner/repo[/path]>`.
pub fn named(text: &str) -> Result<Named, String> {
    let form = "a role is given as <name>, or as <name>=<owner/repo[/path]>";
    let (name, at) = match text.split_once('=') {
        Some((name, at)) => (name, Some(at)),
        None => (text, None),
    };
    let plain = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !plain || name == HUMAN {
        return Err(format!(
            "{form}: the name starts with a lowercase letter, holds lowercase letters, digits \
             and `-`, and is not `{HUMAN}`"
        ));
    }
    let at = match at {
        Some(at) => Some(
            RepoDir::read(at)
                .map_err(|_| format!("{form}: `{at}` is no owner/repo or owner/repo/path"))?,
        ),
        None => None,
    };
    Ok(Named { name: name.to_string(), at })
}

/// A whole team the binary holds, written out by `--example <name>`: its
/// files, each with its path in the team's directory, as one repo holds a
/// team — the team at the root, each role under `roles/`.
pub struct Example {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// Every example there is: the directories of `scaffold/examples/`, which the
/// build script lists.
pub const EXAMPLES: &[Example] = include!(concat!(env!("OUT_DIR"), "/examples.rs"));

/// The names `--example` takes.
pub fn example_names() -> Vec<&'static str> {
    EXAMPLES.iter().map(|example| example.name).collect()
}

/// The example called `name`.
pub fn example(name: &str) -> Option<&'static Example> {
    EXAMPLES.iter().find(|example| example.name == name)
}

impl Example {
    /// Its files as they are written for a team whose directory is `root`, a
    /// path from the root of its repo: the team file names each role by its
    /// path from there (docs/adr/0014), and holds it as `./roles/<name>`.
    fn files(&self, root: &str) -> Vec<(String, String)> {
        let written = |path: &str, text: &str| {
            if path == TEAM_FILE && !root.is_empty() {
                text.replace(&format!(": ./{ROLES}/"), &format!(": ./{root}/{ROLES}/"))
            } else {
                text.to_string()
            }
        };
        self.files.iter().map(|(path, text)| (path.to_string(), written(path, text))).collect()
    }
}

/// Where a skeleton's repo is created: its name on GitHub, and who may see it.
pub struct Create {
    pub repo: RepoName,
    pub public: bool,
}

/// The files of a role's skeleton, each with its path in the role's directory.
fn role_files() -> Vec<(String, String)> {
    let skill = include_str!("scaffold/skill.md").replace("{skill}", EXAMPLE_SKILL);
    let mut files = vec![
        (RULES.to_string(), include_str!("scaffold/rules.md").to_string()),
        (format!("{SKILLS}/{EXAMPLE_SKILL}/SKILL.md"), skill),
    ];
    files.extend(LIMITS.map(|(harness, text)| (harness.limits().to_string(), text.to_string())));
    files
}

/// The files of the skeleton of a team that names `roles`, each with its
/// path in the team's directory: the team's own, and under `roles/<name>/`
/// those of each role given by its name alone. `root` is the team's
/// directory as a path from the root of its repo, and `repo` that repo where
/// it is known.
fn team_files(
    roles: &[Named],
    root: &str,
    repo: Option<&RepoName>,
) -> Result<Vec<(String, String)>> {
    let Some(first) = roles.first() else {
        bail!(
            "a team's skeleton needs --role <name>, once for each role it starts with: the first \
             is the one its example workflow is performed by"
        );
    };
    for (at, role) in roles.iter().enumerate() {
        if roles[..at].iter().any(|other| other.name == role.name) {
            bail!("--role names `{}` twice: a team names each of its roles once", role.name);
        }
    }
    // A role written here is named by its path from the root of the team's
    // repo, not from the team's directory (docs/adr/0014).
    let under = |path: String| if root.is_empty() { path } else { format!("{root}/{path}") };
    let lines: Vec<String> = roles
        .iter()
        .map(|role| match &role.at {
            Some(at) => format!("  {}: {at}", role.name),
            None => format!("  {}: ./{}", role.name, under(format!("{ROLES}/{}", role.name))),
        })
        .collect();
    // A role named `./path` is read from the team's repo, which a directory
    // does not say: the lines the skeleton teaches say it.
    let from = if roles.iter().all(|role| role.at.is_some()) {
        "--dir .".to_string()
    } else {
        let repo = repo.map_or("<owner/repo>", RepoName::as_str);
        let team = if root.is_empty() { repo.to_string() } else { format!("{repo}/{root}") };
        format!("--dir . --team {team}")
    };
    let fill = |text: &str| {
        text.replace("{roles}", &lines.join("\n"))
            .replace("{role}", &first.name)
            .replace("{from}", &from)
            .replace("{skill}", EXAMPLE_SKILL)
            .replace("{workflow}", EXAMPLE_WORKFLOW)
    };
    let mut files = vec![
        (TEAM_FILE.to_string(), fill(include_str!("scaffold/team.md"))),
        (RULES.to_string(), include_str!("scaffold/team_rules.md").to_string()),
        (format!("{WORKFLOWS}/{EXAMPLE_WORKFLOW}.md"), fill(include_str!("scaffold/workflow.md"))),
    ];
    for role in roles.iter().filter(|role| role.at.is_none()) {
        let held = role_files().into_iter();
        files.extend(held.map(|(path, text)| (format!("{ROLES}/{}/{path}", role.name), text)));
    }
    Ok(files)
}

/// `dir`, which need not be there yet, as an absolute path with no symlink
/// in the part of it that is there.
fn resolved(dir: &Path) -> Result<PathBuf> {
    let dir = std::path::absolute(dir).with_context(|| format!("cannot find {}", dir.display()))?;
    let there = dir.ancestors().find(|dir| dir.exists()).unwrap_or(&dir);
    let whole = there.canonicalize().with_context(|| format!("cannot find {}", there.display()))?;
    // What is not there yet is put back on, and nothing where all of it is:
    // joining an empty path would end it in a `/`.
    Ok(dir
        .strip_prefix(there)
        .unwrap_or(Path::new(""))
        .components()
        .fold(whole, |at, part| at.join(part)))
}

/// The path of `dir` from the root of the checkout it is in, its segments
/// joined by `/` and empty for the root itself; `None` when it is in none.
fn in_checkout(dir: &Path) -> Option<String> {
    let there = dir.ancestors().find(|dir| dir.is_dir())?;
    let top = Repo::at(there).ok()?.dir.canonicalize().ok()?;
    let segments = dir.strip_prefix(top).ok()?.components().map(|part| match part {
        Component::Normal(name) => name.to_str(),
        _ => None,
    });
    Some(segments.collect::<Option<Vec<&str>>>()?.join("/"))
}

/// What to add under `roles` in a team's file to name the role at `dir`: by
/// the repo created for it, or by its path in the checkout it is in. `None`
/// when it is in neither, or is a checkout's root: nothing says its repo.
fn team_line(dir: &Path, create: Option<&Create>) -> Option<String> {
    let name = dir.file_name()?.to_str()?;
    match create {
        Some(create) => Some(format!("{name}: {}", create.repo)),
        None => {
            let path = in_checkout(dir).filter(|path| !path.is_empty())?;
            Some(format!("{name}: ./{path}"))
        }
    }
}

/// Writes `text` as `file`, which a harness is to run when it starts with a
/// `#!`: a role's hook is a program.
fn write(file: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(file, text)?;
    #[cfg(unix)]
    if text.starts_with("#!") {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Writes the skeleton of `what` in `dir`, or the team `example` whole — `dir`
/// must not be there yet, or be empty: nothing is overwritten — and, with
/// `create`, commits it there and creates its repo on GitHub from it → what
/// was written.
pub fn scaffold(
    what: What,
    dir: &Path,
    roles: &[Named],
    example: Option<&Example>,
    create: Option<&Create>,
) -> Result<Value> {
    let dir = resolved(dir)?;
    let (files, line) = match what {
        What::Role if !roles.is_empty() => {
            bail!("--role is a team's: a role's skeleton names no role")
        }
        What::Role if example.is_some() => {
            bail!("--example is a team's: an example is a whole team, with its roles")
        }
        What::Role => (role_files(), team_line(&dir, create)),
        What::Team if example.is_some() && !roles.is_empty() => {
            bail!("--example names its own roles: give --role, or --example, not both")
        }
        What::Team => {
            // The repo created is the team's alone; without one the team is
            // a directory of the checkout it is written in, if it is in one.
            let root = match create {
                Some(_) => String::new(),
                None => in_checkout(&dir).unwrap_or_default(),
            };
            let files = match example {
                Some(example) => example.files(&root),
                None => team_files(roles, &root, create.map(|create| &create.repo))?,
            };
            (files, None)
        }
    };
    let taken = match std::fs::read_dir(&dir) {
        Ok(mut entries) => entries.next().is_some(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => bail!("cannot read {}: {error}", dir.display()),
    };
    if taken {
        bail!("{} is not empty: a skeleton is written where nothing is", dir.display());
    }
    for (path, text) in &files {
        let file = dir.join(path);
        let parent = file.parent().unwrap_or(&dir);
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        write(&file, text).with_context(|| format!("cannot write {}", file.display()))?;
    }
    if let Some(create) = create {
        let message = match (what, example) {
            (_, Some(example)) => format!("The example team `{}`", example.name),
            (What::Team, None) => "The skeleton of a team".to_string(),
            (What::Role, None) => "The skeleton of a role".to_string(),
        };
        self::create(&message, &dir, create).with_context(|| {
            format!("the skeleton is written in {}, and its repo is not created", dir.display())
        })?;
    }
    let paths: Vec<&String> = files.iter().map(|(path, _)| path).collect();
    Ok(json!({
        "scaffolded": if what == What::Team { "team" } else { "role" },
        "dir": dir,
        "files": paths,
        "repo": create.map(|create| &create.repo),
        "team_line": line,
    }))
}

/// Makes `dir` a git repo holding one commit, whose message is `message`, and
/// creates `create.repo` on GitHub from it: `gh` adds it as the remote and
/// pushes.
fn create(message: &str, dir: &Path, create: &Create) -> Result<()> {
    let repo = Repo { dir: dir.to_path_buf() };
    repo.git(&["init", "--quiet"])?;
    repo.git(&["add", "--all"])?;
    repo.git(&["commit", "--quiet", "-m", message])?;
    let visibility = if create.public { "--public" } else { "--private" };
    let gh = GitHub { dir: dir.to_path_buf() };
    gh.gh(&["repo", "create", create.repo.as_str(), visibility, "--source", ".", "--push"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_is_given_by_its_name_alone_or_with_its_directory() {
        let at = |text: &str| named(text).unwrap().at.map(|at| at.to_string());
        assert_eq!(named("dev").unwrap(), Named { name: "dev".to_string(), at: None });
        assert_eq!(at("dev=acme/dev").as_deref(), Some("acme/dev"));
        assert_eq!(at("dev=acme/hospital/roles/dev").as_deref(), Some("acme/hospital/roles/dev"));
        for wrong in ["=acme/dev", "Dev", "Dev=acme/dev", "human", "dev=acme", "dev=", "1=a/b"] {
            assert!(named(wrong).unwrap_err().contains("<name>=<owner/repo[/path]>"), "{wrong}");
        }
    }
}

//! `scaffold`: writes the skeleton of a team's repo or of a role's — files
//! `check` accepts as they are — and, when asked, creates the repo on GitHub
//! and pushes the skeleton to it.

use std::path::Path;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::team::{HUMAN, SKILLS, TEAM_FILE, WORKFLOWS};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::Repo;

/// The one skill a role's skeleton holds, and the one a team's example
/// workflow asks of its role: the two skeletons fit because both name this.
pub const EXAMPLE_SKILL: &str = "implement";
/// The example workflow of a team's skeleton.
pub const EXAMPLE_WORKFLOW: &str = "feature";
/// A role's rules, and its hooks: where Claude Code reads each from in the
/// directory a worker starts in.
pub const RULES: &str = "CLAUDE.md";
pub const HOOKS: &str = ".claude/settings.json";

/// What a skeleton is of.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum What {
    Team,
    Role,
}

/// The role a team's skeleton names: its short name there, and its repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    pub repo: RepoName,
}

/// Reads `<name>=<owner/repo>`.
pub fn named(text: &str) -> Result<Named, String> {
    let form = "a role is given as <name>=<owner/repo>";
    let (name, repo) = text.split_once('=').ok_or(form)?;
    let plain = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !plain || name == HUMAN {
        return Err(format!(
            "{form}: the name starts with a lowercase letter, holds lowercase letters, digits \
             and `-`, and is not `{HUMAN}`"
        ));
    }
    let repo = RepoName::read(repo).ok_or_else(|| format!("{form}: `{repo}` is no owner/repo"))?;
    Ok(Named { name: name.to_string(), repo })
}

/// Where a skeleton's repo is created: its name on GitHub, and who may see it.
pub struct Create {
    pub repo: RepoName,
    pub public: bool,
}

/// The files of a skeleton, each with its path in the repo.
fn files(what: What, role: Option<&Named>) -> Result<Vec<(String, String)>> {
    match (what, role) {
        (What::Role, Some(_)) => bail!("--role is a team's: a role's skeleton names no role"),
        (What::Role, None) => {
            let skill = include_str!("scaffold/skill.md").replace("{skill}", EXAMPLE_SKILL);
            Ok(vec![
                (RULES.to_string(), include_str!("scaffold/rules.md").to_string()),
                (format!("{SKILLS}/{EXAMPLE_SKILL}/SKILL.md"), skill),
                (HOOKS.to_string(), include_str!("scaffold/settings.json").to_string()),
            ])
        }
        (What::Team, None) => bail!(
            "a team's skeleton needs --role <name>=<owner/repo>: the role its example workflow \
             is performed by. Write that role first, with `scaffold role`"
        ),
        (What::Team, Some(role)) => {
            let fill = |text: &str| {
                text.replace("{role}", &role.name)
                    .replace("{repo}", role.repo.as_str())
                    .replace("{skill}", EXAMPLE_SKILL)
                    .replace("{workflow}", EXAMPLE_WORKFLOW)
            };
            Ok(vec![
                (TEAM_FILE.to_string(), fill(include_str!("scaffold/team.md"))),
                (
                    format!("{WORKFLOWS}/{EXAMPLE_WORKFLOW}.md"),
                    fill(include_str!("scaffold/workflow.md")),
                ),
            ])
        }
    }
}

/// Writes the skeleton of `what` in `dir` — which must not be there yet, or be
/// empty: nothing is overwritten — and, with `create`, commits it there and
/// creates its repo on GitHub from it → what was written.
pub fn scaffold(
    what: What,
    dir: &Path,
    role: Option<&Named>,
    create: Option<&Create>,
) -> Result<Value> {
    let files = files(what, role)?;
    let taken = match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_some(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => bail!("cannot read {}: {error}", dir.display()),
    };
    if taken {
        bail!("{} is not empty: a skeleton is written where nothing is", dir.display());
    }
    for (path, text) in &files {
        let file = dir.join(path);
        let parent = file.parent().unwrap_or(dir);
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        std::fs::write(&file, text).with_context(|| format!("cannot write {}", file.display()))?;
    }
    let dir = dir.canonicalize().with_context(|| format!("cannot find {}", dir.display()))?;
    if let Some(create) = create {
        self::create(what, &dir, create).with_context(|| {
            format!("the skeleton is written in {}, and its repo is not created", dir.display())
        })?;
    }
    let paths: Vec<&String> = files.iter().map(|(path, _)| path).collect();
    Ok(json!({
        "scaffolded": if what == What::Team { "team" } else { "role" },
        "dir": dir,
        "files": paths,
        "repo": create.map(|create| &create.repo),
    }))
}

/// Makes `dir` a git repo holding one commit, and creates `create.repo` on
/// GitHub from it: `gh` adds it as the remote and pushes.
fn create(what: What, dir: &Path, create: &Create) -> Result<()> {
    let repo = Repo { dir: dir.to_path_buf() };
    let message = match what {
        What::Team => "The skeleton of a team",
        What::Role => "The skeleton of a role",
    };
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
    fn a_role_is_given_as_its_name_and_its_repo() {
        let read = named("dev=acme/dev").unwrap();
        assert_eq!((read.name.as_str(), read.repo.as_str()), ("dev", "acme/dev"));
        for wrong in ["dev", "=acme/dev", "Dev=acme/dev", "human=acme/dev", "dev=acme", "1=a/b"] {
            assert!(named(wrong).unwrap_err().contains("<name>=<owner/repo>"), "{wrong}");
        }
    }
}

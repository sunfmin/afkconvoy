//! `scaffold`: writes the skeleton of a team with its roles, or of one role,
//! or a whole example team — files `check` accepts as they are — and, when
//! asked, creates the repo on GitHub and pushes what it wrote to it.

use std::path::{Component, Path, PathBuf};

use afkconvoy_core::config::{RepoDir, RepoName};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::team::{HUMAN, RULES, SKILLS, TEAM_FILE, WORKFLOWS};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::gh::{GitHub, Tracker};
use crate::git::Repo;

/// The one skill a role's skeleton holds, and the one a team's example
/// workflow asks of its role: the two skeletons fit because both name this.
/// In a team's skeleton only the role that workflow is performed by holds it.
pub const EXAMPLE_SKILL: &str = "implement";
/// The example workflow of a team's skeleton.
pub const EXAMPLE_WORKFLOW: &str = "feature";
/// The harnesses a role's skeleton holds a file of limits for, each with that
/// file, which names no hook yet: the ones whose limits are one file. pi's
/// are the entries of a directory, and a skeleton writes no empty one.
const LIMITS: [(Harness, &str); 4] = [
    (Harness::ClaudeCode, include_str!("scaffold/settings.json")),
    (Harness::Codex, include_str!("scaffold/hooks.json")),
    (Harness::Kimi, include_str!("scaffold/limits.toml")),
    (Harness::Grok, include_str!("scaffold/grok.toml")),
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

/// The files of a role's skeleton, each with its path in the role's
/// directory: its rules, its limits and, when it is `skilled`, the example
/// skill.
fn role_files(skilled: bool) -> Vec<(String, String)> {
    let mut files = vec![(RULES.to_string(), include_str!("scaffold/rules.md").to_string())];
    if skilled {
        let skill = include_str!("scaffold/skill.md").replace("{skill}", EXAMPLE_SKILL);
        files.push((format!("{SKILLS}/{EXAMPLE_SKILL}/SKILL.md"), skill));
    }
    files.extend(LIMITS.map(|(harness, text)| (harness.limits().to_string(), text.to_string())));
    files
}

/// The files of the skeleton of a team that names `roles`, each with its
/// path in the team's directory: the team's own, and under `roles/<name>/`
/// those of each role given by its name alone — the first role with the
/// example skill, which the example workflow asks of it, and the others with
/// none: what a role after it does is not to implement. `root` is the team's
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
        let held = role_files(role.name == first.name).into_iter();
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
        What::Role => (role_files(true), team_line(&dir, create)),
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
    let repo = Repo::new(dir.to_path_buf());
    repo.git(&["init", "--quiet"])?;
    repo.git(&["add", "--all"])?;
    repo.git(&["commit", "--quiet", "-m", message])?;
    let visibility = if create.public { "--public" } else { "--private" };
    let gh = GitHub::new(dir.to_path_buf(), Tracker::Own);
    gh.gh(&["repo", "create", create.repo.as_str(), visibility, "--source", ".", "--push"])?;
    Ok(())
}

/// What `afkconvoy scaffold --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

To start a team from files `check` accepts as they are, instead of from an empty repo. A team and its roles, in one repo created on GitHub:

```
afkconvoy scaffold team hospital --role fellow --role attending --repo acme/hospital
afkconvoy check --team acme/hospital
```

And to add a role to a team that is there, in a checkout of its repo:

```
afkconvoy scaffold role roles/reviewer
```

From there on a team is designed by editing its files and running `check`.

And to see a finished team first, or to start from one — a whole team the binary holds, written as one repo holds it:

```
afkconvoy scaffold team hospital --example hospital --repo acme/hospital
afkconvoy help example
```

Read that one first when the team is to have more than one stage — a review, a landing role of its own: a skeleton's workflow has one stage, and a skeleton holds no skill to review or to land with. The example holds each, to copy from.

## Who runs it

The operator, or whoever designs the team: anywhere. It reads no config.

## What it reads and writes

It writes in the directory named, which must not be there yet or be empty: nothing is ever overwritten.

`scaffold team <dir> --role <name>…` writes the team's files, and each role's under `roles/<name>/`:

- `team.md`: the team file, naming each role as `./roles/<name>`
- `CLAUDE.md`: the team rules: a heading, what belongs there and what belongs in a repo's own rules or a role's, and that the host user's rules give way to all of them
- `workflows/feature.md`: one workflow, `feature`: one stage the first role performs as its skill `implement`, then a turn that lands its PR
- `roles/<name>/`: that role's skeleton, as `scaffold role` writes it — the first role's with the skill `implement`, every other's with no skill

`--role` is given once for each role, and at least once. `--role <name>` is a role written here. `--role <name>=<owner/repo>` or `<name>=<owner/repo/path>` is one that is elsewhere — a role several teams share, in a repo of its own: `team.md` names it as given and nothing is written for it. The two forms mix. The example workflow is performed by the first role given; one that is elsewhere must hold a skill `implement`.

A role after the first is written with its rules and its limits, and no skill: a reviewer does not implement. What it is to perform is yours to write, as `roles/<name>/.claude/skills/<skill>/SKILL.md`, with the stage that names it in `workflows/feature.md` — that file says so too.

`scaffold team <dir> --example <name>` writes an example team whole in place of a skeleton: its team file, team rules, workflows and patrols, and every role it names under `roles/<name>/`, with each role's rules, skills and hooks. The one name it takes is `hospital`: seven roles, three workflows and four patrols, which `afkconvoy help example` goes through file by file. It takes no `--role` — the team file names its own, each as `./roles/<name>` — and is a team's alone. A hook, like any file written that starts with `#!`, is written executable. Skills by other authors that the example's roles load are not written, and one of its stages runs on pi: `afkconvoy help example-run` says what a host needs before a fleet works with it.

`scaffold role <dir>` writes a role's three kinds of file, and takes no `--role`:

- `CLAUDE.md`: rules to replace with the role's own
- `.claude/skills/implement/SKILL.md`: one example skill, `implement`: rename it, and rewrite it, for a role that does not; its form is in `afkconvoy help skill-file`
- `.claude/settings.json`: its limits for claude-code — hooks: none yet
- `.codex/hooks.json`: its limits for codex — hooks: none yet
- `.kimi-code/limits.toml`: its limits for kimi — hooks and denied permissions: none yet
- `.grok/config.toml`: its limits for grok — hooks and denied permissions: none yet

The rules and the skills are written once, whatever harness a stage runs the role on. The role's limits are written once for each harness, in that harness's own form: for claude-code, hooks and denied permissions in `.claude/settings.json`; for codex, hooks in `.codex/hooks.json`; for kimi, hooks and denied permissions in `.kimi-code/limits.toml`; for grok, hooks and denied permissions in `.grok/config.toml`; and for pi, extensions in `.pi/extensions/`, a directory this command does not write. For dsh, a role writes its own overlay in `.dsh/cordis.patch.yml`; this command writes none. A file that names no hook holds no limit: the skeleton is right for every harness. `check` refuses a stage on a harness its role holds no limits for while it holds some for another.

The role's name is the directory's. Run in a checkout, it writes there and creates nothing on GitHub: commit and push it with the team.

With `--repo <owner/repo>` it then makes the directory a git repo, commits the skeleton, and creates that repo on GitHub with the commit pushed to it: `gh repo create <owner/repo> --private --source . --push`, run there. `--public` creates it public instead. Without `--repo` it runs no `gh`, and of `git` only the question of which checkout the directory is in: a role's path in `team.md` is written from the root of the team's repo, so a team written in a subdirectory of a checkout names its roles from that checkout's root.

A skill is added to a role afterwards with the skills installer, run in the role's directory and committed: this command installs none.

## What it answers

{"scaffolded", "dir", "files", "repo", "team_line"} — "team" or "role"; the directory written in, as an absolute path; the files written, as paths in it; the repo created, null without `--repo`; and, of a role, the line to add under `roles` in `team.md`:

- scaffold role roles/reviewer: "team_line": "reviewer: ./roles/reviewer"
- scaffold role dev --repo acme/dev: "team_line": "dev: acme/dev"

`team_line` is null of a team, and of a role nothing says the repo of: one written outside any checkout with no `--repo`, or as the root of one.

Errors, as {"error": …} with a non-zero exit: the directory is not empty; a team with no `--role` and no `--example`, or one name given twice; a role with a `--role` or an `--example`; an `--example` with a `--role`; an `--example` there is no example of, which lists the names there are; a `--role` that is neither <name> nor <name>=<owner/repo[/path]>; a file that cannot be written; and, with `--repo`, a `git` or `gh` call that failed — the error says the skeleton is written and its repo is not created.

## What trips you up

- `check` reads every role from GitHub: a team's skeleton passes it once it is pushed — `--repo` does that — and not before.
- `check --dir <dir>` alone refuses a role named `./path`: a directory says no repo. Say the team's beside it — `check --dir <dir> --team <owner/repo>` — as the skeleton's own files do.
- A role that is elsewhere must be on GitHub, with its skill on its default branch, before the team that names it passes `check`.
- After a failed `--repo` the directory is no longer empty. Create the repo yourself from it with the `gh` line above; do not run this again on it.
- `--repo` commits as whoever git is set up for on this host, and creates the repo as whoever `gh` is logged in as. Every host of a fleet must be able to read it."#;

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

//! Reads a team — its own files, and the directory of each role it names —
//! for the core to load (`afkconvoy_core::team`), and answers `check` with
//! what was read or with every fault. `load` is how a command comes by a team
//! and `keep` how a tick does: both read every repo from the fleet's copies.

use std::path::{Component, Path, PathBuf};

use afkconvoy_core::casting;
use afkconvoy_core::config::{Config, RepoDir, RepoLabels, RepoName};
use afkconvoy_core::harness::{Harness, Held};
use afkconvoy_core::team::{
    self, Consult, Fault, File, FlowLabels, Node, PATROLS, Pass, Patrol, Performer, RULES, RoleDir,
    SKILL_FILE, SKILLS, Sources, Stage, TEAM_FILE, Team, Unread, WORKFLOWS, Work, Workflow,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config;
use crate::copies::{self, Copies, Copy};
use crate::gh::{GitHub, Unreached, unreached};
use crate::git::Repo;
use crate::topic::See;

/// Where a team's own files are taken from. Its roles' directories are
/// always read from the fleet's copies of their repos' heads on GitHub.
pub enum From {
    /// The team the config of the checkout it is read for names.
    Config,
    /// A team's directory on GitHub: a repo, or a subdirectory of one.
    Repo(RepoDir),
    /// A directory holding a team's files, read as they are, committed or
    /// not — with the directory on GitHub they are an edit of, where that is
    /// said: the repo a role named `./path` is read from.
    Dir(PathBuf, Option<RepoDir>),
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
    /// The faults, each as `{file, key, problem}` — and `see`, where a topic
    /// describes the file it is in.
    pub fn json(&self) -> Value {
        let fault = |fault: &Fault| {
            let mut said = json!({"file": fault.file, "key": fault.key, "problem": fault.problem});
            if let Some(see) = See::of_fault(fault) {
                said["see"] = json!(see);
            }
            said
        };
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

/// A repo's files, as a directory holds them: a checkout as it is, or the
/// fleet's copy of one commit.
struct Tree {
    dir: PathBuf,
}

impl Tree {
    /// The names of the entries of the directory `dir`, sorted; none when
    /// there is no such directory.
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        let mut names: Vec<String> = match std::fs::read_dir(self.dir.join(dir)) {
            Ok(entries) => entries
                .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
                .collect::<Result<_>>()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(error) => bail!("cannot list {dir}: {error}"),
        };
        names.sort();
        Ok(names)
    }

    /// The text of the file at `path`; `None` when no file is there.
    fn read(&self, path: &str) -> Result<Option<String>> {
        let file = self.dir.join(path);
        if !file.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&file);
        Ok(Some(text.with_context(|| format!("cannot read {}", file.display()))?))
    }

    /// The definition file at `path`: the first yaml block of its markdown,
    /// and the prose above it.
    fn file(&self, path: &str) -> File {
        let (prose, yaml) = match self.read(path) {
            Ok(Some(markdown)) => (config::prose(&markdown), yaml(&markdown)),
            Ok(None) => (String::new(), Err("is not there, or is no file".to_string())),
            Err(error) => (String::new(), Err(format!("{error:#}"))),
        };
        File { path: path.to_string(), prose, yaml }
    }
}

/// The yaml a skill's file `markdown` opens with: what stands between a
/// first line `---` and the next such line.
fn frontmatter(markdown: &str) -> Result<Node, String> {
    let mut lines = markdown.lines();
    let mut block = String::new();
    let mut closed = false;
    if lines.next().map(str::trim_end) == Some("---") {
        for line in lines {
            if line.trim_end() == "---" {
                closed = true;
                break;
            }
            block.push_str(line);
            block.push('\n');
        }
    }
    if !closed {
        return Err("does not open with frontmatter: a first line `---`, yaml, and a line `---`"
            .to_string());
    }
    serde_yaml::from_str(&block)
        .map_err(|error| format!("opens with frontmatter that cannot be read: {error}"))
}

fn yaml(markdown: &str) -> Result<Node, String> {
    let block = config::yaml_block(markdown).ok_or("has no ```yaml block")?;
    serde_yaml::from_str(&block).map_err(|error| format!("its yaml cannot be read: {error}"))
}

/// A team that loaded, and what it was read from.
pub struct Loaded {
    /// What the team's files were read from: its repo, or a directory of
    /// this host.
    pub name: String,
    /// What a fault of the team calls it: its directory on GitHub, where it
    /// was read from one.
    pub called: String,
    /// Its path in that repo, empty for a whole repo; `None` for a directory
    /// of this host.
    pub path: Option<String>,
    /// The commit its own files were read at; `None` for a directory of this
    /// host.
    pub commit: Option<String>,
    /// The definition files read, the team file first.
    pub files: Vec<String>,
    /// The patrols the repo of this checkout names, when it is its team that
    /// was read: the ones that run on it.
    pub runs: Vec<String>,
    /// Whether the team's directory holds a `.claude/`: hooks and skills no
    /// worker loads.
    pub settings: bool,
    pub team: Team,
    /// The team's directory on GitHub; `None` for a directory of this host.
    dir: Option<RepoDir>,
    /// Where the repos it was read from are kept.
    copies: Copies,
}

impl Loaded {
    /// Whether the team has moved since `since`, a commit of its repo: its
    /// directory, without what its copy leaves out (`left_out`), is not at
    /// its head what it was then. A push that changes only a role's
    /// directory, or something else of the repo, moves no team. One read
    /// from a directory of this host has moved from any commit.
    pub fn moved(&self, since: &str) -> bool {
        match (&self.dir, &self.commit) {
            (Some(dir), Some(head)) => {
                changed(&self.copies, dir, since, head, &left_out(dir, &self.team))
            }
            _ => true,
        }
    }

    /// Whether `role` has moved since `since`, a commit of its repo: its
    /// directory is not at the head what it was then. In a repo of several
    /// roles a push moves the commit of each, and the roles whose directory
    /// it changed.
    pub fn role_moved(&self, role: &team::Role, since: &str) -> bool {
        changed(&self.copies, &role.dir, since, &role.commit, &[])
    }
}

/// Whether `dir`, less each of `without`, holds at the commit `head` of its
/// repo anything else than at `since` (docs/adr/0014): asked of the fleet's
/// copies of the two, and of nobody else. A commit this host keeps no copy
/// of — no stage started under it here — is taken to have held something
/// else: two commits are then all there is to go by.
fn changed(copies: &Copies, dir: &RepoDir, since: &str, head: &str, without: &[PathBuf]) -> bool {
    if since == head {
        return false;
    }
    let (Some(then), Some(now)) = (copies.kept(&dir.repo, since), copies.kept(&dir.repo, head))
    else {
        return true;
    };
    !copies::alike(&then, &now, dir, without).unwrap_or(false)
}

/// Reads the team `from` names and every role's directory, loads it, and
/// holds it to the harnesses its workers would run on (`held`) → what was
/// read, with a warning for each workflow whose file says nothing of what it
/// is for, and a note for a team whose directory holds hooks and skills no
/// worker loads, and for each role a directory above which holds rules a
/// worker in it is not given. A team with a fault fails with `Refused`.
/// `checkout` is the checkout it is read for, as `load` reads it.
pub fn check(from: From, checkout: Option<&Repo>) -> Result<Value> {
    let loaded = load(from, checkout)?;
    let faults = held(&loaded);
    if !faults.is_empty() {
        return Err(Refused { team: loaded.called, faults }.into());
    }
    if loaded.settings {
        eprintln!(
            "note: the team's directory holds a {SETTINGS}/: no worker loads the hooks and skills \
             there. Of a team's directory a worker is given its rules ({RULES}); a hook and a \
             skill are a role's, in the role's own directory"
        );
    }
    for role in loaded.team.roles.iter().filter(|role| !role.above.is_empty()) {
        eprintln!(
            "note: above the role `{}` ({}), its repo holds {}: a session opened by hand in the \
             role's directory loads that too, and a worker does not — it is given the role's \
             directory alone",
            role.name,
            role.dir,
            role.above.join(", ")
        );
    }
    for workflow in loaded.team.workflows.iter().filter(|w| w.description.is_empty()) {
        eprintln!(
            "warning: {WORKFLOWS}/{}.md says nothing of what the workflow is for: whoever hands \
             the fleet work reads the prose above its yaml block, in `afkconvoy flows`",
            workflow.name
        );
    }
    Ok(answer(&loaded))
}

/// Holds the team `loaded` to the harnesses its workers may run, on this
/// host (`Team::held`) → every fault that finds. A role the roster names
/// none for may run what this host's host kit holds: none, on a host
/// without one. What may be wrong is said on stderr.
fn held(loaded: &Loaded) -> Vec<Fault> {
    let kit: Vec<Harness> = crate::kit::load()
        .map(|kit| kit.harnesses.iter().map(|held| held.harness).collect())
        .unwrap_or_default();
    let held = loaded.team.held(&kit);
    for note in &held.notes {
        eprintln!("note: {note}");
    }
    held.faults
}

/// Reads the team `from` names and every role's directory, and loads it
/// against the labels of the repo of `checkout`, the checkout it is read for
/// — or, where there is none or its config cannot be read, the labels a repo
/// that names none has. A team with a fault fails with `Refused`.
pub fn load(from: From, checkout: Option<&Repo>) -> Result<Loaded> {
    let gh = match checkout {
        Some(repo) => GitHub::of(repo)?,
        None => GitHub::in_no_checkout(),
    };
    let copies = Copies::of_host()?;
    let here = match checkout {
        Some(repo) => config::load(&config::file_of(repo)),
        None => Err(anyhow!("no checkout is named, whose config would name the team")),
    };
    let labels = here.as_ref().map(|config| config.labels()).unwrap_or_default();
    // The patrols this checkout's repo names, when it is its team that is
    // read.
    let mut named = vec![];
    let team = match from {
        From::Dir(dir, of) => {
            if !dir.is_dir() {
                bail!("{} is not a directory", dir.display());
            }
            let name = dir.display().to_string();
            let settings = dir.join(SETTINGS).is_dir();
            let (sources, roles) = read(&Tree { dir }, of, None, &gh, &copies, &labels)?;
            roles.reached()?;
            return loaded(name, None, copies, sources, named, settings);
        }
        From::Repo(team) => team,
        From::Config => {
            let Some(bound) = here?.bound else {
                bail!(
                    "this repo is unbound: its config names no team. Name one with --team, \
                     or a directory of a team's files with --dir"
                );
            };
            named = bound.patrols;
            bound.team
        }
    };
    let (copy, sources, roles) = read_kept(&gh, &copies, &team, &labels)?;
    roles.reached()?;
    let settings = within(&copy, &team).join(SETTINGS).is_dir();
    let at = Some((team.clone(), copy.commit));
    loaded(team.repo.to_string(), at, copies, sources, named, settings)
}

/// The team that `sources` load as — read from `name`: out of `copies`, from
/// the directory on GitHub and at the commit `at` says, or from a directory
/// of this host — on a repo whose config names the patrols `runs`.
/// `settings` is whether its directory holds a
/// `.claude/`.
fn loaded(
    name: String,
    at: Option<(RepoDir, String)>,
    copies: Copies,
    sources: Sources,
    runs: Vec<String>,
    settings: bool,
) -> Result<Loaded> {
    let mut files = vec![sources.team.path.clone()];
    files.extend(sources.workflows.iter().map(|file| file.path.clone()));
    files.extend(sources.patrols.iter().map(|file| file.path.clone()));
    // A team is called by its directory on GitHub, where it was read from one.
    let called = match (&sources.at, &at) {
        (Some(at), Some(_)) => at.to_string(),
        _ => name.clone(),
    };
    let team = match team::load(&sources) {
        Ok(team) => team,
        Err(faults) => return Err(Refused { team: called, faults }.into()),
    };
    if let Err(unknown) = team.patrols_named(&runs) {
        return Err(unknown_patrols(&called, &team, &unknown).into());
    }
    let (dir, commit) = at.unzip();
    let path = dir.as_ref().map(|dir: &RepoDir| dir.path().to_string());
    Ok(Loaded { name, called, path, commit, files, runs, settings, team, dir, copies })
}

/// What reading the repo of each role of a team came to, by the repo's name.
#[derive(Default)]
struct RoleReads {
    /// The copy of each repo that was read.
    copies: Vec<(RepoName, Copy)>,
    /// The read of each repo GitHub could not be reached for.
    unreached: Vec<(RepoName, Unreached)>,
}

impl RoleReads {
    /// Refuses a team that has a role GitHub could not be reached for: a
    /// command answers for the whole team, and a tick for the roles it uses.
    fn reached(&self) -> Result<()> {
        match self.unreached.first() {
            Some((_, unreached)) => Err(unreached.clone().into()),
            None => Ok(()),
        }
    }
}

/// Everything the core loads a team from: the team's own files out of
/// `tree`, and each role's directory out of the fleet's copy of its repo's
/// head as it stands now — fetched only when no copy of that commit is here,
/// and once however many roles the repo holds. `at` is the team's directory
/// on GitHub, where one is known, and `own` the copy `tree` is in: a role in
/// the team's repo is then read at the team's commit. The repos are read at
/// once, not one after another → with what those reads came to. A repo GitHub
/// could not be reached for is no fault of the team (`Unread::Unreached`).
fn read(
    tree: &Tree,
    at: Option<RepoDir>,
    own: Option<&Copy>,
    gh: &GitHub,
    copies: &Copies,
    labels: &RepoLabels,
) -> Result<(Sources, RoleReads)> {
    let team = tree.file(TEAM_FILE);
    let rules = tree.dir.join(RULES).is_file();
    let entries = tree.list(WORKFLOWS)?;
    let workflows = entries.iter().map(|name| tree.file(&format!("{WORKFLOWS}/{name}"))).collect();
    let entries = tree.list(PATROLS)?;
    let patrols = entries.iter().map(|name| tree.file(&format!("{PATROLS}/{name}"))).collect();
    let named = team::role_dirs(&team, at.as_ref());
    let mut repos: Vec<&RepoName> = vec![];
    for dir in &named {
        if !repos.contains(&&dir.repo) {
            repos.push(&dir.repo);
        }
    }
    let copy = |repo: &RepoName| match (own, &at) {
        (Some(own), Some(at)) if at.repo == *repo => Ok(own.clone()),
        _ => copies.of(gh, repo),
    };
    let reads: Vec<Result<Copy>> = std::thread::scope(|scope| {
        let reads: Vec<_> = repos.iter().map(|repo| scope.spawn(|| copy(repo))).collect();
        let joined = reads.into_iter().map(|read| read.join());
        joined.map(|read| read.unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
    });
    let (mut copied, mut kept) = (vec![], RoleReads::default());
    for (repo, read) in repos.into_iter().zip(reads) {
        let read = match read {
            Ok(copy) => {
                kept.copies.push((repo.clone(), copy.clone()));
                Ok(copy)
            }
            Err(error) => Err(match unreached(&error) {
                Some(read) => {
                    kept.unreached.push((repo.clone(), read.clone()));
                    Unread::Unreached(read.to_string())
                }
                None => Unread::Refused(format!("{error:#}")),
            }),
        };
        copied.push((repo.clone(), read));
    }
    let roles = named.into_iter().map(|dir| {
        let copy = copied.iter().find(|(repo, _)| *repo == dir.repo).map(|(_, read)| read.clone());
        let read = copy.unwrap_or_else(|| Err(Unread::Refused("its repo was not read".into())));
        let read = read.and_then(|copy| role_dir(&copy, &dir, at.as_ref()));
        (dir, read)
    });
    let roles = roles.collect();
    let labels = labels.clone();
    Ok((Sources { at, team, rules, workflows, patrols, roles, labels }, kept))
}

/// Where `dir` is in `copy`, the fleet's copy of its repo: the copy itself
/// for a whole repo, spelled as the copy is.
fn within(copy: &Copy, dir: &RepoDir) -> PathBuf {
    if dir.path().is_empty() { copy.dir.clone() } else { copy.dir.join(dir.path()) }
}

/// What the core is told of the role's directory `dir`, read out of `copy`,
/// the fleet's copy of its repo, for the team at `team`.
fn role_dir(copy: &Copy, dir: &RepoDir, team: Option<&RepoDir>) -> Result<RoleDir, Unread> {
    let root = within(copy, dir);
    if !root.is_dir() {
        return Err(Unread::Refused(format!(
            "it is no directory at the head of the default branch of {} ({})",
            dir.repo, copy.commit
        )));
    }
    let read = || {
        let skills = Tree { dir: root.clone() }.list(SKILLS)?;
        let mut links = vec![];
        // A whole repo is copied whole: a link in it leaves nothing behind.
        if !dir.path().is_empty() {
            leaving_links(&root, Path::new(""), &mut links)?;
            links.sort();
        }
        let mut malformed = vec![];
        for skill in &skills {
            let file =
                Tree { dir: root.clone() }.read(&format!("{SKILLS}/{skill}/{SKILL_FILE}"))?;
            let opens = match &file {
                Some(markdown) => frontmatter(markdown),
                None => Err("is not there, or is no file".to_string()),
            };
            if let Some(why) = team::malformed(skill, &opens) {
                malformed.push((skill.clone(), why));
            }
        }
        let held = Tree { dir: root.clone() }.list("")?;
        let in_place = Harness::ALL.into_iter().flat_map(|harness| {
            let before = harness.before_the_rules().iter();
            let held = before.filter(|file| held.iter().any(|name| name == *file));
            held.map(move |file| (harness, file.to_string()))
        });
        Ok(RoleDir {
            commit: copy.commit.clone(),
            skills,
            links,
            above: above(copy, dir, team),
            limits: limits(&root)?,
            in_place: in_place.collect(),
            own_skills: own_skills(&root)?,
            malformed,
        })
    };
    read().map_err(|error: anyhow::Error| Unread::Refused(format!("{error:#}")))
}

/// The skills the role whose directory is `root` holds where a harness finds
/// them by itself, for each harness it holds such a directory for
/// (`copies::own_skills`).
fn own_skills(root: &Path) -> Result<Vec<(Harness, Vec<String>)>> {
    let mut own = vec![];
    for harness in Harness::ALL {
        let dirs = copies::own_skills(root, harness);
        if dirs.is_empty() {
            continue;
        }
        let mut skills = vec![];
        for dir in dirs {
            skills.extend(Tree { dir: root.to_path_buf() }.list(dir)?);
        }
        skills.sort();
        skills.dedup();
        own.push((harness, skills));
    }
    Ok(own)
}

/// The harnesses the role whose directory is `root` holds limits for: what
/// each says of what the directory holds where its limits go
/// (`Harness::limited`). An error is one whose limits cannot be read.
fn limits(root: &Path) -> Result<Vec<Harness>> {
    let mut limited = vec![];
    for harness in Harness::ALL {
        let at = root.join(harness.limits());
        let held = if at.is_file() {
            let text = std::fs::read_to_string(&at)?;
            harness.limited_file(&text).map_err(anyhow::Error::msg)
        } else {
            let held: Held<&mut serde_json::Deserializer<serde_json::de::StrRead<'_>>> =
                if at.is_dir() {
                    // A file kept only to have git keep the directory is no limit.
                    let entries = Tree { dir: at.clone() }.list("")?;
                    Held::Entries(entries.iter().filter(|name| !name.starts_with('.')).count())
                } else {
                    Held::Nothing
                };
            let result: Result<bool, serde_json::Error> = harness.limited(held);
            result.map_err(anyhow::Error::msg)
        }
        .with_context(|| {
            format!("its limits for {}, in {}, cannot be read", harness.name(), harness.limits())
        })?;
        if held {
            limited.push(harness);
        }
    }
    Ok(limited)
}

/// The directory of skills and settings Claude Code loads, beside the rules
/// (`RULES`), from a directory a session is started in or under.
const SETTINGS: &str = ".claude";

/// Each `CLAUDE.md` and `.claude/` a directory above `dir` holds in `copy`,
/// the copy of its repo, as a path from the repo's root: what Claude Code
/// loads for a session started in `dir` of a checkout, and a worker is not
/// given (docs/adr/0014). The rules of the directory of `team`, the team the
/// role is read for, are none of them: a worker is given the team rules.
fn above(copy: &Copy, dir: &RepoDir, team: Option<&RepoDir>) -> Vec<String> {
    let mut found = vec![];
    let mut at = PathBuf::new();
    let teams =
        |at: &Path| team.is_some_and(|team| team.repo == dir.repo && Path::new(team.path()) == at);
    for segment in dir.path().split('/').filter(|segment| !segment.is_empty()) {
        let (rules, settings) = (at.join(RULES), at.join(SETTINGS));
        if copy.dir.join(&rules).is_file() && !teams(&at) {
            found.push(rules.display().to_string());
        }
        if copy.dir.join(&settings).is_dir() {
            found.push(format!("{}/", settings.display()));
        }
        at.push(segment);
    }
    found
}

/// Adds to `links` each symlink under `under` of the directory `root` that
/// leaves `root`, as `<link> -> <target>`: one whose target is absolute, or
/// climbs above `root`. A link is not followed.
fn leaving_links(root: &Path, under: &Path, links: &mut Vec<String>) -> Result<()> {
    let dir = root.join(under);
    for entry in
        std::fs::read_dir(&dir).with_context(|| format!("cannot list {}", dir.display()))?
    {
        let entry = entry?;
        let path = under.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            leaving_links(root, &path, links)?;
        } else if kind.is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            // How deep under `root` the link's target is, a part at a time.
            let mut depth = Some(under.components().count());
            for part in target.components() {
                depth = match (part, depth) {
                    (Component::Normal(_), Some(depth)) => Some(depth + 1),
                    (Component::CurDir, depth) => depth,
                    (Component::ParentDir, Some(depth)) => depth.checked_sub(1),
                    _ => None,
                };
            }
            if depth.is_none() {
                links.push(format!("{} -> {}", path.display(), target.display()));
            }
        }
    }
    Ok(())
}

/// `read`, of the team at `team`: its own files out of the fleet's copy of
/// its repo's head as it stands now → with that copy, of the whole repo.
fn read_kept(
    gh: &GitHub,
    copies: &Copies,
    team: &RepoDir,
    labels: &RepoLabels,
) -> Result<(Copy, Sources, RoleReads)> {
    let copy =
        copies.of(gh, &team.repo).with_context(|| format!("the team {team} cannot be read"))?;
    let dir = within(&copy, team);
    if !dir.is_dir() {
        bail!(
            "the team {team} cannot be read: {} is no directory at the head of the default \
             branch of {} ({})",
            team.path(),
            team.repo,
            copy.commit
        );
    }
    let (sources, roles) =
        read(&Tree { dir }, Some(team.clone()), Some(&copy), gh, copies, labels)?;
    Ok((copy, sources, roles))
}

/// A team as a stage starts under it: loaded from the fleet's copies, at the
/// commits those were taken at. A role GitHub could not be reached for is
/// among its roles with nothing known of it (`team::Role::unreached`): what
/// needs no worker in that role is carried out all the same.
pub struct Kept {
    pub team: Team,
    /// The copy of the team's own directory, without the directories of its
    /// roles, and its repo's commit.
    pub copy: Copy,
    /// What reading each role's repo came to.
    roles: RoleReads,
    /// Where each role's directory is taken out of the copy of its repo.
    copies: Copies,
}

impl Kept {
    /// The team rules, in the team's copy: what a brief names. `None` of a
    /// team that has none.
    pub fn rules(&self) -> Option<String> {
        self.team.rules.then(|| self.copy.dir.join(RULES).display().to_string())
    }

    /// The copy of the directory of `role`, and of nothing else of its repo:
    /// what a worker of `harness` in it starts in (`Copies::role`), taken
    /// out of the copy of its repo unless one of that commit is here
    /// (docs/adr/0014). `Unreached` of a role GitHub could not be reached
    /// for.
    pub fn role(&self, role: &team::Role, harness: Harness) -> Result<Copy> {
        let repo = &role.dir.repo;
        if let Some((_, unreached)) = self.roles.unreached.iter().find(|(read, _)| read == repo) {
            return Err(unreached.clone().into());
        }
        let mut copies = self.roles.copies.iter();
        let fetched = copies.find(|(read, copy)| read == repo && copy.commit == role.commit);
        let (_, fetched) = fetched.ok_or_else(|| anyhow!("no copy of {repo} was kept"))?;
        self.copies.role(fetched, &role.dir, harness)
    }
}

/// What the copy of the team `loaded` from the directory `dir` leaves out,
/// each a path from `dir`: the directory of every role in it, and the
/// `.claude/` at its top. The team's copy is added to every worker, and what
/// a harness loads from an added directory is then the `CLAUDE.md` at its top
/// alone: the team rules, and neither a neighbour's rules nor the skills and
/// settings a repo's root holds for a human (docs/adr/0014).
fn left_out(dir: &RepoDir, loaded: &Team) -> Vec<PathBuf> {
    let inside = loaded.roles.iter().filter(|role| dir.holds(&role.dir) && role.dir != *dir);
    let roles = inside.map(|role| {
        let under = role.dir.path().strip_prefix(dir.path()).unwrap_or(role.dir.path());
        PathBuf::from(under.trim_start_matches('/'))
    });
    roles.chain([PathBuf::from(SETTINGS)]).collect()
}

/// Loads the team at `team` as it stands now: the head of its repo and of
/// each role's, each read from the fleet's copy of that commit and fetched
/// only when no copy of it is here — and the team's directory taken out of
/// its repo's — read against the `labels` of the repo bound to it. A team
/// with a fault fails with `Refused`, and one whose own repo GitHub could not
/// be reached for with `Unreached`.
pub fn keep(gh: &GitHub, copies: &Copies, team: &RepoDir, labels: &RepoLabels) -> Result<Kept> {
    let (fetched, sources, roles) = read_kept(gh, copies, team, labels)?;
    let loaded = match team::load(&sources) {
        Ok(loaded) => loaded,
        Err(faults) => return Err(Refused { team: team.to_string(), faults }.into()),
    };
    let copy = copies.team(&fetched, team, &left_out(team, &loaded))?;
    Ok(Kept { team: loaded, copy, roles, copies: copies.clone() })
}

/// The flow labels of the team the repo of `config` is bound to, as it
/// stands now: none in an unbound repo, or where the team does not load.
pub fn flow_labels(gh: &GitHub, config: &Config) -> FlowLabels {
    let Some(bound) = &config.bound else { return FlowLabels::default() };
    let kept =
        Copies::of_host().and_then(|copies| keep(gh, &copies, &bound.team, &config.labels()));
    kept.map(|kept| kept.team.flow_labels()).unwrap_or_default()
}

/// A role's entry in the roster, as `check` and `show` answer it.
fn roster_entry(entry: &team::Entry) -> Value {
    let run: Vec<Value> = entry
        .run
        .iter()
        .map(|allowed| {
            json!({"harness": allowed.harness, "model": allowed.model, "good_at": allowed.good_at})
        })
        .collect();
    json!({"cast": entry.cast, "run": run})
}

/// Each harness `role`'s range on a host whose host kit holds `kit` leaves
/// out (`casting::left_out`), as `check` and `plan` answer it: with why.
pub fn left_out_json(role: &team::Role, kit: &[casting::Held]) -> Value {
    let each =
        |harness: Harness| json!({"harness": harness, "why": casting::left_out_why(harness)});
    Value::Array(casting::left_out(role, kit).into_iter().map(each).collect())
}

/// What `team` says of the role `name`'s roster entry where it matches
/// nothing in the host kit `kit` (`casting::unmatched_entries`).
fn unmatched_of(team: &Team, name: &str, kit: &[casting::Held]) -> Option<String> {
    let each = casting::unmatched_entries(team, kit).into_iter();
    each.filter(|(role, _)| role == name).map(|(_, said)| said).next()
}

/// A role's range on this host, as `check` and `show` answer it: each
/// harness with its models, in order.
fn range_json(range: &[casting::Held]) -> Value {
    let each = |held: &casting::Held| json!({"harness": held.harness, "models": held.ids()});
    Value::Array(range.iter().map(each).collect())
}

fn answer(loaded: &Loaded) -> Value {
    let team = &loaded.team;
    // A role's range is this host's: none where it has no host kit.
    let kit = crate::kit::load().ok().map(|kit| kit.range());
    let roles: Vec<Value> = team
        .roles
        .iter()
        .map(|role| {
            json!({"name": role.name, "repo": role.dir.repo, "path": role.dir.path(),
                "commit": role.commit, "host_user": role.host_user, "dsh_profile": role.dsh_profile.key(), "skills": role.skills,
                "above": role.above, "limits": role.limits,
                "roster": role.roster.as_ref().map(roster_entry),
                "range": kit.as_ref().map(|kit| range_json(&casting::range(role, kit))),
                "left_out": kit.as_ref().map(|kit| left_out_json(role, kit)),
                "unmatched": kit.as_ref().and_then(|kit| unmatched_of(team, &role.name, kit)),
                "nothing_safe": kit.as_ref().and_then(|kit| casting::nothing_safe(role, kit))})
        })
        .collect();
    json!({
        "team": loaded.name,
        "path": loaded.path,
        "commit": loaded.commit,
        "files": loaded.files,
        "escalate_label": team.escalate_label,
        "rules": team.rules,
        "host_user": team.host_user, "dsh_profile": team.dsh_profile.key(),
        "split": team.split,
        "roles": roles,
        // Each model the roster names that the host kit holds by no name.
        "unmatched_models": kit.as_ref().map(|kit| casting::unmatched_models(team, kit)),
        "workflows": team.workflows.iter().map(|w| workflow(loaded, w)).collect::<Vec<_>>(),
        "patrols": team.patrols.iter().map(|p| patrol(loaded, p)).collect::<Vec<_>>(),
        "precedents": team.precedents.as_ref().map(|precedents| {
            json!({"by": team.role_of(&precedents.work).name, "do": precedents.work.skill,
                "max_open": precedents.max_open})
        }),
    })
}

/// One patrol of the team `loaded`, and whether it is among the ones the
/// repo names.
fn patrol(loaded: &Loaded, patrol: &Patrol) -> Value {
    json!({
        "name": patrol.name,
        "by": loaded.team.role_of(&patrol.work).name,
        "do": patrol.work.skill,
        "every": patrol.every,
        "max_open": patrol.max_open,
        "max_runs_a_day": patrol.max_runs_a_day,
        "max_landed_a_day": patrol.max_landed_a_day,
        "runs_here": loaded.runs.contains(&patrol.name),
    })
}

/// One workflow of the team `loaded`.
fn workflow(loaded: &Loaded, workflow: &Workflow) -> Value {
    let team = &loaded.team;
    let name = |at: usize| workflow.stage(at).name.clone();
    let role = |work: &Work| team.role_of(work).name.as_str();
    let stage = |stage: &Stage| {
        let (by, skill) = match &stage.by {
            Performer::Human => (team::HUMAN, None),
            Performer::Role(work) => (role(work), Some(&work.skill)),
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
            "writes": stage.writes,
            "escalate": match (&stage.consult, &stage.by) {
                (Some(Consult::Role(work)), _) => json!({"to": role(work), "do": work.skill}),
                (None, Performer::Role(_)) => json!({"to": team::HUMAN}),
                (None, Performer::Human) => Value::Null,
            },
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
        "land": workflow.land.as_ref().map(|land| json!({"by": role(land), "do": land.skill})),
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

    #[test]
    fn a_skill_s_file_opens_with_its_frontmatter() {
        let read = frontmatter("---\nname: plan\ndescription: a: b\n---\n\n# plan\n---\n");
        assert!(read.unwrap_err().starts_with("opens with frontmatter that cannot be read: "));
        let read = frontmatter("---\nname: plan\n---\n\n# plan\n---\nname: no\n").unwrap();
        assert_eq!(read, Node::Map(vec![("name".into(), Node::Text("plan".into()))]));
        for none in ["", "# plan\n---\nname: plan\n---\n", "---\nname: plan\n"] {
            assert!(frontmatter(none).unwrap_err().starts_with("does not open with frontmatter"));
        }
    }
}

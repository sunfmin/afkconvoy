//! The agent setup (sunfmin/control#528): the operator's global instructions
//! and skills, declared once outside afkconvoy, given to every harness the
//! host kit holds, whatever harness a worker starts on. The operator owns
//! what: an instructions file and a skill list, named by two host settings
//! (`--agent-instructions`, `--agent-skills`). afkconvoy owns where each
//! harness reads them (`Harness::instructions`, `Harness::skills_agent`).
//!
//! Giving it is converging: the instructions file is linked at each held
//! harness's global instructions location, and the skill list is installed
//! through `npx skills` for each held harness's agent, with every installed
//! skill of a declared harness's agent that the list does not name removed.
//! The installed copies, in the shared `~/.agents/skills`, are read-only
//! between syncs; a sync opens them for its own writes and closes them again.
//! Writing the host kit gives it to every held harness; a tick gives it to a
//! harness held since the setup was last given. With none of the settings
//! said, nothing is given and nothing is reported. The guards, the third
//! part (`--agent-guards`), are wired into each held harness's hook
//! settings (`agent_guards`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use afkconvoy_core::harness::Harness;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host_settings::Settings;

/// How long one `npx skills add` or `remove` may take: it fetches a source.
const SYNC_TIMEOUT: Duration = Duration::from_secs(600);
/// How long one `npx skills ls` may take: it reads the host alone.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a tick waits before giving a harness the setup again after a
/// failed attempt.
const TRIED_AGAIN_AFTER: u64 = 3600;
/// Where the shared skill copies are, under the home.
const COPIES: &str = ".agents/skills";
/// The line that gives every held harness the agent setup again.
pub const FIX: &str = "afkconvoy kit";

/// What the host settings declare of the agent setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declared {
    pub instructions: Option<PathBuf>,
    pub skills: Option<PathBuf>,
    /// The guard list (sunfmin/control#529); a record of an older release
    /// says none.
    #[serde(default)]
    pub guards: Option<PathBuf>,
}

impl Declared {
    /// What `settings` declare; `None` where they say neither.
    pub fn of(settings: &Settings) -> Option<Declared> {
        let declared = Declared {
            instructions: settings.agent_instructions.clone(),
            skills: settings.agent_skills.clone(),
            guards: settings.agent_guards.clone(),
        };
        let any = declared.instructions.is_some()
            || declared.skills.is_some()
            || declared.guards.is_some();
        any.then_some(declared)
    }
}

/// One line of a skill list: a source and the skills it gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub source: String,
    pub skills: Which,
}

/// The skills one source gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Which {
    /// `*`: every skill the source ships.
    Every,
    Named(Vec<String>),
}

/// Reads a skill list: one line per source, `<source> <skill>...` or
/// `<source> *`; blank lines and `#` lines say nothing.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries = vec![];
    for (at, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let source = words.next().unwrap_or_default().to_string();
        let skills: Vec<String> = words.map(str::to_string).collect();
        let skills = match skills.as_slice() {
            [] => return Err(format!("line {}: `{source}` names no skill, nor `*`", at + 1)),
            [every] if every == "*" => Which::Every,
            named if named.iter().any(|skill| skill == "*") => {
                return Err(format!("line {}: `*` stands alone after its source", at + 1));
            }
            named => Which::Named(named.to_vec()),
        };
        entries.push(Entry { source, skills });
    }
    Ok(entries)
}

/// One skill installed for an agent, as `npx skills ls --json` says it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Installed {
    pub name: String,
    /// Where it came from; `None` for one `npx skills` does not track.
    #[serde(default)]
    pub source: Option<String>,
    /// The agents it is installed for: empty where it is a shared copy the
    /// agent asked about was not given.
    #[serde(default)]
    pub agents: Vec<String>,
}

/// What an agent lacks of `entries`, and holds beyond them, of what is
/// `installed` for it: the names of each. A skill `npx skills` does not
/// track is no one's to remove, and a `*` source lacks something only where
/// none of its skills is installed.
pub fn drift(entries: &[Entry], installed: &[Installed]) -> (Vec<String>, Vec<String>) {
    let given: Vec<&Installed> =
        installed.iter().filter(|skill| !skill.agents.is_empty()).collect();
    let has = |source: &str, name: Option<&str>| {
        given.iter().any(|skill| {
            skill.source.as_deref() == Some(source) && name.is_none_or(|name| skill.name == name)
        })
    };
    let mut missing = vec![];
    for entry in entries {
        match &entry.skills {
            Which::Every if !has(&entry.source, None) => {
                missing.push(format!("every skill of {}", entry.source));
            }
            Which::Every => {}
            Which::Named(names) => missing.extend(
                names.iter().filter(|name| !has(&entry.source, Some(name.as_str()))).cloned(),
            ),
        }
    }
    let declares = |skill: &Installed| {
        entries.iter().any(|entry| {
            skill.source.as_deref() == Some(entry.source.as_str())
                && match &entry.skills {
                    Which::Every => true,
                    Which::Named(names) => names.contains(&skill.name),
                }
        })
    };
    let extra = given
        .iter()
        .filter(|skill| skill.source.is_some() && !declares(skill))
        .map(|skill| skill.name.clone())
        .collect();
    (missing, extra)
}

/// Why `harness`'s global instructions under `home` are not the declared
/// `file`; `None` where they are a link to it.
pub fn unlinked(home: &Path, harness: Harness, file: &Path) -> Option<String> {
    let at = home.join(harness.instructions());
    let shown = format!("~/{}", harness.instructions());
    match std::fs::symlink_metadata(&at) {
        Err(_) => Some(format!("{shown} is missing")),
        Ok(meta) if !meta.file_type().is_symlink() => Some(format!("{shown} is not a link")),
        Ok(_) => {
            let to = std::fs::canonicalize(&at).ok();
            let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
            (to.as_ref() != Some(&file))
                .then(|| format!("{shown} links elsewhere than {}", file.display()))
        }
    }
}

/// Links `harness`'s global instructions under `home` to `file` → whether it
/// changed anything. What stood there and was no link to it is kept beside
/// it, as `<name>.before-agent-setup`, never removed.
pub fn link(home: &Path, harness: Harness, file: &Path, now: u64) -> Result<bool> {
    if unlinked(home, harness, file).is_none() {
        return Ok(false);
    }
    let at = home.join(harness.instructions());
    let parent = at.parent().context("an instructions location has a directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("cannot make {}", parent.display()))?;
    if std::fs::symlink_metadata(&at).is_ok() {
        let name = at.file_name().unwrap_or_default().to_string_lossy();
        let mut aside = at.with_file_name(format!("{name}.before-agent-setup"));
        if std::fs::symlink_metadata(&aside).is_ok() {
            aside = at.with_file_name(format!("{name}.before-agent-setup.{now}"));
        }
        std::fs::rename(&at, &aside)
            .with_context(|| format!("cannot move {} aside", at.display()))?;
    }
    std::os::unix::fs::symlink(file, &at)
        .with_context(|| format!("cannot link {} to {}", at.display(), file.display()))?;
    Ok(true)
}

/// Makes everything under the copy at `path` writable by its owner (`open`),
/// or by no one (`!open`). A link is left as it is.
fn set_mode(path: &Path, open: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mode = meta.permissions().mode();
    let mode = if open { mode | 0o200 } else { mode & !0o222 };
    // A directory is opened before what is in it is read, and closed after.
    if open || !meta.is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            set_mode(&entry?.path(), open)?;
        }
        if !open {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

/// `set_mode` of each copy in `copies`, leaving `copies` itself writable,
/// where a sync adds a new copy.
pub fn set_open(copies: &Path, open: bool) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(copies) else { return Ok(()) };
    for entry in entries {
        let path = entry?.path();
        set_mode(&path, open)
            .with_context(|| format!("cannot change the mode of {}", path.display()))?;
    }
    Ok(())
}

/// `set_open` of the shared copies under `home`, and `set_mode` of each skill
/// `npx skills` tracks for one of `harnesses` that is a copy of its own in a
/// directory where that harness reads its user's skills (Claude Code's
/// `~/.claude/skills`): an agent installed for alone may be given a copy
/// there, where several are given links to the shared one. A skill there
/// that `npx skills` does not track is the user's own, and left as it is.
fn set_open_all(home: &Path, harnesses: &[(Harness, &str)], open: bool) -> Result<()> {
    set_open(&home.join(COPIES), open)?;
    for (harness, agent) in harnesses {
        // An agent whose list cannot be read has no copy known to close.
        let names: Vec<String> = listed(agent)
            .unwrap_or_default()
            .into_iter()
            .filter(|skill| skill.source.is_some())
            .map(|skill| skill.name)
            .collect();
        let dirs = harness.kept().host_user.iter().filter(|link| link.home.ends_with("skills"));
        for dir in dirs.filter(|link| link.home != COPIES) {
            for name in &names {
                let path = home.join(dir.home).join(name);
                if std::fs::symlink_metadata(&path).is_ok() {
                    set_mode(&path, open)
                        .with_context(|| format!("cannot change the mode of {}", path.display()))?;
                }
            }
        }
    }
    Ok(())
}

/// `text` as one word of a POSIX shell.
fn word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The `npx skills` line that installs `entry` for `agents`.
pub fn add_line(entry: &Entry, agents: &[&str]) -> String {
    let skills = match &entry.skills {
        Which::Every => vec![word("*")],
        Which::Named(names) => names.iter().map(|name| word(name)).collect(),
    };
    let agents: Vec<String> = agents.iter().map(|agent| word(agent)).collect();
    format!(
        "npx -y skills add {} -g -y -a {} -s {}",
        word(&entry.source),
        agents.join(" "),
        skills.join(" ")
    )
}

/// The `npx skills` line that removes `skills` from `agent`.
pub fn remove_line(agent: &str, skills: &[String]) -> String {
    let skills: Vec<String> = skills.iter().map(|skill| word(skill)).collect();
    format!("npx -y skills remove -g -y -a {} -s {}", word(agent), skills.join(" "))
}

/// The `npx skills` line that lists what is installed for `agent`.
pub fn list_line(agent: &str) -> String {
    format!("npx -y skills ls -g -a {} --json", word(agent))
}

/// Runs `line` in the login shell a worker gets → what it printed; an error
/// says why it did not succeed.
fn ask(line: &str, timeout: Duration) -> Result<String> {
    match crate::launch::ask_host(line, timeout)? {
        Some((true, printed)) => Ok(printed),
        Some((false, printed)) => {
            let tail: Vec<&str> = printed.lines().rev().take(3).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            anyhow::bail!("`{line}` failed: {}", tail.join(" / "))
        }
        None => anyhow::bail!("`{line}` did not end within {}s", timeout.as_secs()),
    }
}

/// What is installed for `agent`, as `npx skills ls` says it.
pub fn listed(agent: &str) -> Result<Vec<Installed>> {
    let line = list_line(agent);
    let printed = ask(&line, LIST_TIMEOUT)?;
    let json = printed.find('[').map_or("", |at| &printed[at..]);
    serde_json::from_str(json.trim_end()).with_context(|| format!("`{line}` printed no list"))
}

/// The home of the host's user.
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")
}

/// Where what was given to which harness is recorded: beside the host kit.
fn record_path() -> Result<PathBuf> {
    Ok(crate::kit::path()?.with_file_name("agent-setup.json"))
}

/// What the setup was last given to each harness as.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    given: BTreeMap<String, Given>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Given {
    at: u64,
    declared: Declared,
    /// Why it was not given, where it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed: Option<String>,
}

fn read_record(file: &Path) -> Record {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Which harnesses a converge gives the setup to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum To {
    /// Every one named: writing the host kit, or a `run` that changed it.
    Every,
    /// Those not yet given it as it is declared now, and those whose last
    /// attempt failed an hour ago or more: a tick's.
    New,
}

/// Gives the agent setup the host settings declare to `harnesses` (`to`)
/// → what it did, `None` where nothing was to be done. With no setup
/// declared, nothing is done.
pub fn converge(harnesses: &[Harness], to: To, now: u64) -> Result<Option<Value>> {
    let Some(declared) = Declared::of(&crate::host_settings::of_host()?) else {
        return Ok(None);
    };
    let file = record_path()?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // One process of the host gives it at a time. A tick finding another at
    // it leaves it to that one; writing the host kit waits for it.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(file.with_extension("lock"))
        .with_context(|| format!("cannot open the lock beside {}", file.display()))?;
    match to {
        To::Every => lock.lock()?,
        To::New => match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        },
    }
    let mut record = read_record(&file);
    let due = |harness: &Harness| match record.given.get(harness.key()) {
        _ if to == To::Every => true,
        None => true,
        Some(given) if given.declared != declared => true,
        Some(given) => given.failed.is_some() && now.saturating_sub(given.at) >= TRIED_AGAIN_AFTER,
    };
    let targets: Vec<Harness> = harnesses.iter().copied().filter(due).collect();
    if targets.is_empty() {
        return Ok(None);
    }
    let done = give(&home()?, &declared, &targets, now);
    for (harness, failed) in &done.outcomes {
        let given = Given { at: now, declared: declared.clone(), failed: failed.clone() };
        record.given.insert(harness.key().to_string(), given);
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&file, serde_json::to_vec_pretty(&record)?)
        .with_context(|| format!("cannot write {}", file.display()))?;
    Ok(Some(done.json()))
}

/// What one give did.
struct Done {
    outcomes: Vec<(Harness, Option<String>)>,
    linked: Vec<Harness>,
    wired: Vec<Harness>,
    synced: Vec<String>,
}

impl Done {
    fn json(&self) -> Value {
        let failed: Vec<Value> = self
            .outcomes
            .iter()
            .filter_map(|(harness, failed)| {
                failed.as_ref().map(|why| json!({"harness": harness, "why": why}))
            })
            .collect();
        json!({
            "harnesses": self.outcomes.iter().map(|(harness, _)| harness).collect::<Vec<_>>(),
            "linked": self.linked,
            "wired": self.wired,
            "synced": self.synced,
            "failed": failed,
        })
    }
}

/// Gives `declared` to `targets` under `home`: links the instructions, then
/// opens the copies, installs every entry for the targets' agents at once,
/// removes what the list does not name, and closes the copies again however
/// that went.
fn give(home: &Path, declared: &Declared, targets: &[Harness], now: u64) -> Done {
    let mut failed: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut linked = vec![];
    if let Some(file) = &declared.instructions {
        for &harness in targets {
            match link(home, harness, file, now) {
                Ok(true) => linked.push(harness),
                Ok(false) => {}
                Err(error) => failed.entry(harness.key()).or_default().push(format!("{error:#}")),
            }
        }
    }
    let mut wired = vec![];
    if let Some(list) = &declared.guards {
        match read_guards(list) {
            Err(why) => {
                for &harness in targets.iter().filter(|harness| harness.user_hooks().is_some()) {
                    failed.entry(harness.key()).or_default().push(why.clone());
                }
            }
            Ok(guards) => {
                for &harness in targets {
                    match crate::agent_guards::wire(home, harness, &guards) {
                        Ok(true) => wired.push(harness),
                        Ok(false) => {}
                        Err(error) => {
                            failed.entry(harness.key()).or_default().push(format!("{error:#}"));
                        }
                    }
                }
            }
        }
    }
    let mut synced = vec![];
    if let Some(list) = &declared.skills {
        let agents: Vec<(Harness, &str)> = targets
            .iter()
            .filter_map(|&harness| harness.skills_agent().map(|agent| (harness, agent)))
            .collect();
        let synced_or = sync(home, list, &agents, &mut synced);
        if let Err(error) = synced_or {
            for (harness, _) in &agents {
                failed.entry(harness.key()).or_default().push(format!("{error:#}"));
            }
        }
    }
    let outcomes = targets
        .iter()
        .map(|harness| (*harness, failed.get(harness.key()).map(|why| why.join("; "))))
        .collect();
    Done { outcomes, linked, wired, synced }
}

/// The guards this host's settings declare: none where they name no guard
/// list; an error where the list they name cannot be read as one, so no
/// session is started without a guard its operator declared.
pub fn guards_of_host() -> Result<Vec<crate::agent_guards::Guard>> {
    let Some(list) = crate::host_settings::of_host()?.agent_guards else { return Ok(vec![]) };
    read_guards(&list).map_err(anyhow::Error::msg)
}

/// The guards the list at `list` declares; an error says why it cannot be
/// read as one.
fn read_guards(list: &Path) -> Result<Vec<crate::agent_guards::Guard>, String> {
    let text = std::fs::read_to_string(list)
        .map_err(|error| format!("the guard list {} cannot be read: {error}", list.display()))?;
    crate::agent_guards::parse(&text)
        .map_err(|why| format!("the guard list {} is wrong: {why}", list.display()))
}

/// Installs `list` for `agents`, and removes from each what it does not
/// name, with the copies open only while it does.
fn sync(
    home: &Path,
    list: &Path,
    agents: &[(Harness, &str)],
    synced: &mut Vec<String>,
) -> Result<()> {
    let text = std::fs::read_to_string(list)
        .with_context(|| format!("cannot read the skill list {}", list.display()))?;
    let entries = parse(&text)
        .map_err(|why| anyhow::anyhow!("the skill list {} is wrong: {why}", list.display()))?;
    if agents.is_empty() {
        return Ok(());
    }
    set_open_all(home, agents, true)?;
    let names: Vec<&str> = agents.iter().map(|(_, agent)| *agent).collect();
    let mut run = || -> Result<()> {
        for entry in &entries {
            let line = add_line(entry, &names);
            ask(&line, SYNC_TIMEOUT)?;
            synced.push(line);
        }
        for agent in names.iter().copied().collect::<BTreeSet<_>>() {
            let (_, extra) = drift(&entries, &listed(agent)?);
            if !extra.is_empty() {
                let line = remove_line(agent, &extra);
                ask(&line, SYNC_TIMEOUT)?;
                synced.push(line);
            }
        }
        Ok(())
    };
    let ran = run();
    let closed = set_open_all(home, agents, false);
    ran.and(closed)
}

/// Each held harness that differs from the agent setup, as a finding: its
/// instructions not linked, a declared skill not installed for it, one
/// installed for it that the list does not name, or its hook settings
/// missing a declared guard or holding one the list does not name. None with no setup declared.
pub fn findings() -> Vec<crate::doctor::Finding> {
    let Ok(settings) = crate::host_settings::of_host() else { return vec![] };
    let Some(declared) = Declared::of(&settings) else { return vec![] };
    let (Ok(kit), Ok(home)) = (crate::kit::load(), home()) else { return vec![] };
    let entries = declared.skills.as_ref().map(|list| {
        std::fs::read_to_string(list)
            .map_err(|error| format!("the skill list {} cannot be read: {error}", list.display()))
            .and_then(|text| parse(&text).map_err(|why| format!("the skill list is wrong: {why}")))
    });
    let guards = declared.guards.as_deref().map(read_guards);
    let mut asked: BTreeMap<&str, Result<Vec<Installed>, String>> = BTreeMap::new();
    let mut findings = vec![];
    for held in &kit.harnesses {
        let harness = held.harness;
        let mut said = vec![];
        if let Some(file) = &declared.instructions
            && let Some(why) = unlinked(&home, harness, file)
        {
            said.push(why);
        }
        match (&guards, harness.user_hooks()) {
            (Some(Err(why)), Some(_)) => said.push(why.clone()),
            (Some(Ok(guards)), Some(_)) => {
                said.extend(crate::agent_guards::unwired(&home, harness, guards));
            }
            _ => {}
        }
        match (&entries, harness.skills_agent()) {
            (Some(Err(why)), _) => said.push(why.clone()),
            (Some(Ok(entries)), Some(agent)) => {
                let installed = asked
                    .entry(agent)
                    .or_insert_with(|| listed(agent).map_err(|error| format!("{error:#}")));
                match installed {
                    Err(why) => said.push(why.clone()),
                    Ok(installed) => {
                        let (missing, extra) = drift(entries, installed);
                        if !missing.is_empty() {
                            said.push(format!("missing skills: {}", missing.join(", ")));
                        }
                        if !extra.is_empty() {
                            said.push(format!(
                                "skills the list does not name: {}",
                                extra.join(", ")
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
        if said.is_empty() {
            continue;
        }
        findings.push(crate::doctor::finding(
            crate::doctor::Kind::AgentSetupDrifted,
            json!(["agent-setup", harness]),
            format!("the harness `{}` differs from the agent setup", harness.key()),
            said.join("; "),
            FIX.into(),
        ));
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(name: &str, source: Option<&str>, given: bool) -> Installed {
        Installed {
            name: name.into(),
            source: source.map(str::to_string),
            agents: if given { vec!["Codex".into()] } else { vec![] },
        }
    }

    #[test]
    fn a_skill_list_names_sources_and_their_skills_or_every_one() {
        let text = "# the list\n\nmattpocock/skills *\nsunfmin/product-team complexity-review  single-source-of-truth\n";
        assert_eq!(
            parse(text).unwrap(),
            vec![
                Entry { source: "mattpocock/skills".into(), skills: Which::Every },
                Entry {
                    source: "sunfmin/product-team".into(),
                    skills: Which::Named(vec![
                        "complexity-review".into(),
                        "single-source-of-truth".into()
                    ]),
                },
            ]
        );
        assert!(parse("sunfmin/mypython\n").unwrap_err().contains("line 1"));
        assert!(parse("a/b x *\n").unwrap_err().contains("stands alone"));
    }

    #[test]
    fn drift_is_a_named_skill_not_given_and_a_tracked_skill_the_list_does_not_name() {
        let entries = parse("a/every *\nb/some one two\nc/none *\n").unwrap();
        let have = [
            installed("x", Some("a/every"), true),
            installed("one", Some("b/some"), true),
            installed("two", Some("b/some"), false),
            installed("three", Some("b/some"), true),
            installed("stray", Some("d/other"), true),
            installed("handmade", None, true),
            installed("not-for-it", Some("d/other"), false),
        ];
        let (missing, extra) = drift(&entries, &have);
        assert_eq!(missing, vec!["two".to_string(), "every skill of c/none".to_string()]);
        assert_eq!(extra, vec!["three".to_string(), "stray".to_string()]);
    }

    #[test]
    fn instructions_are_linked_and_what_stood_there_is_kept_aside() {
        let home = tempfile::tempdir().unwrap();
        let declared = home.path().join("dotfiles/CLAUDE.md");
        std::fs::create_dir_all(declared.parent().unwrap()).unwrap();
        std::fs::write(&declared, "principles\n").unwrap();
        let at = home.path().join(".codex/AGENTS.md");
        assert!(unlinked(home.path(), Harness::Codex, &declared).unwrap().contains("missing"));
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "mine\n").unwrap();
        assert!(unlinked(home.path(), Harness::Codex, &declared).unwrap().contains("not a link"));
        assert!(link(home.path(), Harness::Codex, &declared, 7).unwrap());
        assert_eq!(unlinked(home.path(), Harness::Codex, &declared), None);
        assert_eq!(std::fs::read_to_string(&at).unwrap(), "principles\n");
        let aside = home.path().join(".codex/AGENTS.md.before-agent-setup");
        assert_eq!(std::fs::read_to_string(aside).unwrap(), "mine\n");
        assert!(!link(home.path(), Harness::Codex, &declared, 8).unwrap(), "linked already");
        // a link elsewhere is moved aside too, beside the first kept
        let other = home.path().join("other.md");
        std::fs::write(&other, "other\n").unwrap();
        std::fs::remove_file(&at).unwrap();
        std::os::unix::fs::symlink(&other, &at).unwrap();
        assert!(unlinked(home.path(), Harness::Codex, &declared).unwrap().contains("elsewhere"));
        assert!(link(home.path(), Harness::Codex, &declared, 9).unwrap());
        assert!(home.path().join(".codex/AGENTS.md.before-agent-setup.9").exists());
    }

    #[test]
    fn copies_are_closed_to_every_write_and_opened_for_a_sync() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let copies = home.path().join(COPIES);
        let skill = copies.join("mypython");
        std::fs::create_dir_all(skill.join("references")).unwrap();
        std::fs::write(skill.join("SKILL.md"), "a skill\n").unwrap();
        std::fs::write(skill.join("references/more.md"), "more\n").unwrap();
        set_open(&copies, false).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for path in [&skill, &skill.join("SKILL.md"), &skill.join("references/more.md")] {
            assert_eq!(mode(path) & 0o222, 0, "{} is writable", path.display());
        }
        assert!(std::fs::write(skill.join("SKILL.md"), "edited\n").is_err());
        assert!(std::fs::write(skill.join("new.md"), "new\n").is_err());
        assert_ne!(mode(&copies) & 0o200, 0, "a sync still adds a copy");
        set_open(&copies, true).unwrap();
        std::fs::write(skill.join("SKILL.md"), "updated\n").unwrap();
        std::fs::remove_dir_all(&skill).unwrap();
    }

    #[test]
    fn each_line_quotes_what_it_names() {
        let entry = parse("mattpocock/skills *\n").unwrap().remove(0);
        assert_eq!(
            add_line(&entry, &["claude-code", "codex"]),
            "npx -y skills add 'mattpocock/skills' -g -y -a 'claude-code' 'codex' -s '*'"
        );
        assert_eq!(
            remove_line("codex", &["stray".into()]),
            "npx -y skills remove -g -y -a 'codex' -s 'stray'"
        );
    }
}

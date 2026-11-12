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
//! settings (`afkconvoy_host_kit::guards`).
//!
//! What each part does is the host kit crate's (`afkconvoy_host_kit`, which
//! Guild calls too, docs/adr/0062): this module says where afkconvoy keeps the
//! declaration and the record of what was given, and runs the crate's lines
//! in the login shell a worker gets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use afkconvoy_core::harness::Harness;
pub use afkconvoy_host_kit::Declared;
use afkconvoy_host_kit::Shell;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host_settings::Settings;

/// How long a tick waits before giving a harness the setup again after a
/// failed attempt.
const TRIED_AGAIN_AFTER: u64 = 3600;
/// The line that gives every held harness the agent setup again.
pub const FIX: &str = "afkconvoy kit";

/// The login shell a worker gets (`launch::ask_host`), in which the host kit
/// crate asks what this host has and runs what installs it.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkerShell;

impl Shell for WorkerShell {
    fn run(&self, line: &str, timeout: Duration) -> Result<Option<(bool, String)>> {
        crate::launch::ask_host(line, timeout)
    }
}

/// What `settings` declare of the agent setup; `None` where they say none
/// of it.
pub fn declared(settings: &Settings) -> Option<Declared> {
    let declared = Declared {
        instructions: settings.agent_instructions.clone(),
        skills: settings.agent_skills.clone(),
        guards: settings.agent_guards.clone(),
    };
    declared.any().then_some(declared)
}

/// The home of the host's user.
pub fn home() -> Result<PathBuf> {
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
    let Some(declared) = declared(&crate::host_settings::of_host()?) else {
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
    let done = afkconvoy_host_kit::give(&WorkerShell, &home()?, &declared, &targets, now);
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

/// The guards this host's settings declare: none where they name no guard
/// list; an error where the list they name cannot be read as one, so no
/// session is started without a guard its operator declared.
pub fn guards_of_host() -> Result<Vec<afkconvoy_host_kit::Guard>> {
    let Some(list) = crate::host_settings::of_host()?.agent_guards else { return Ok(vec![]) };
    afkconvoy_host_kit::read_guards(&list).map_err(anyhow::Error::msg)
}

/// Each held harness that differs from the agent setup, as a finding: its
/// instructions not linked, a declared skill not installed for it, one
/// installed for it that the list does not name, or its hook settings
/// missing a declared guard or holding one the list does not name. None with
/// no setup declared.
pub fn findings() -> Vec<crate::doctor::Finding> {
    let Ok(settings) = crate::host_settings::of_host() else { return vec![] };
    let Some(declared) = declared(&settings) else { return vec![] };
    let (Ok(kit), Ok(home)) = (crate::kit::load(), home()) else { return vec![] };
    let held: Vec<Harness> = kit.harnesses.iter().map(|held| held.harness).collect();
    afkconvoy_host_kit::differences(&WorkerShell, &home, &declared, &held)
        .into_iter()
        .map(|difference| {
            crate::doctor::finding(
                crate::doctor::Kind::AgentSetupDrifted,
                json!(["agent-setup", difference.harness]),
                format!("the harness `{}` differs from the agent setup", difference.harness.key()),
                difference.said.join("; "),
                FIX.into(),
            )
        })
        .collect()
}

//! The host kit's agent setup (sunfmin/control#528, #529, #753, #758;
//! docs/adr/0062), kept apart so that another program can
//! give a machine the operator's agent setup with no afkconvoy installed
//! there. Guild's `guild install` calls only its `tmux`: Guild's instances
//! are not given the agent setup (docs/adr/0066, sunfmin/control#780).
//!
//! What it does is what afkconvoy's host kit does, and nothing else:
//!
//! - find the harnesses on the machine (`found`);
//! - link the operator's instructions file at each harness's global
//!   instructions location (`instructions`);
//! - install the declared skill list for each harness's agent (`skills`);
//! - wire the guards into each harness's own hook settings, replacing only
//!   its own entries (`guards`);
//! - install what workers need: tmux on a Guild machine (`tmux`);
//! - say where the machine differs from the declaration (`differences`).
//!
//! The operator owns what is declared — an instructions file, a skill list
//! and a guard list, in files of their own (`Declared`); the crate owns where
//! each harness reads them, through the harness crate. How a line is run in
//! the login shell of the host's user is the embedder's to say (`Shell`);
//! `LoginShell` is a plain one. Nothing in its API is a fleet's: no team,
//! role, stage, issue, claim or patrol.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use afkconvoy_harness::Harness;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod found;
pub mod guards;
pub mod instructions;
pub mod shell;
pub mod skills;
pub mod tmux;

pub use guards::Guard;
pub use shell::{LoginShell, Shell};
pub use skills::Entry;
pub use tmux::Tmux;

/// Where the operator's agent setup is declared: the instructions file, the
/// skill list and the guard list, each where one is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declared {
    pub instructions: Option<PathBuf>,
    pub skills: Option<PathBuf>,
    /// The guard list (sunfmin/control#529); a record of an older release
    /// says none.
    #[serde(default)]
    pub guards: Option<PathBuf>,
}

impl Declared {
    /// Whether it declares anything: with nothing declared, nothing is given
    /// and nothing differs.
    pub fn any(&self) -> bool {
        self.instructions.is_some() || self.skills.is_some() || self.guards.is_some()
    }
}

/// The skills the list at `list` declares; an error says why it cannot be
/// read as one.
pub fn read_skills(list: &Path) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(list)
        .map_err(|error| format!("the skill list {} cannot be read: {error}", list.display()))?;
    skills::parse(&text).map_err(|why| format!("the skill list {} is wrong: {why}", list.display()))
}

/// The guards the list at `list` declares; an error says why it cannot be
/// read as one.
pub fn read_guards(list: &Path) -> Result<Vec<Guard>, String> {
    let text = std::fs::read_to_string(list)
        .map_err(|error| format!("the guard list {} cannot be read: {error}", list.display()))?;
    guards::parse(&text).map_err(|why| format!("the guard list {} is wrong: {why}", list.display()))
}

/// What one give did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Done {
    /// Each harness given the setup, with why it was not, where it was not.
    pub outcomes: Vec<(Harness, Option<String>)>,
    /// Those whose instructions were linked anew.
    pub linked: Vec<Harness>,
    /// Those whose hook settings were wired anew.
    pub wired: Vec<Harness>,
    /// Each `npx skills` line that was run.
    pub synced: Vec<String>,
}

impl Done {
    /// What it did, as afkconvoy's `kit` answers it:
    /// `{harnesses, linked, wired, synced, failed: [{harness, why}]}`.
    pub fn json(&self) -> Value {
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

/// Gives `declared` to `targets` under `home`: links the instructions, wires
/// the guards, then opens the skill copies, installs every entry for the
/// targets' agents at once through `shell`, removes what the list does not
/// name, and closes the copies again however that went. What failed for a
/// harness is said in its outcome, and stops nothing given to another.
/// `now` names what was kept aside of a location linked anew.
pub fn give(
    shell: &dyn Shell,
    home: &Path,
    declared: &Declared,
    targets: &[Harness],
    now: u64,
) -> Done {
    let mut failed: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut linked = vec![];
    if let Some(file) = &declared.instructions {
        for &harness in targets {
            match instructions::link(home, harness, file, now) {
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
                    match guards::wire(home, harness, &guards) {
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
        let synced_or = read_skills(list)
            .map_err(anyhow::Error::msg)
            .and_then(|entries| skills::sync(shell, home, &entries, &agents, &mut synced));
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

/// How one harness differs from the declared setup: each thing said of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub harness: Harness,
    pub said: Vec<String>,
}

/// Each of `harnesses` that differs from `declared` under `home`: its
/// instructions not linked, a declared skill not installed for it, one
/// installed for it that the list does not name, or its hook settings missing
/// a declared guard or holding one the list does not name. What is installed
/// for an agent is asked of `shell` once, however many harnesses share it.
pub fn differences(
    shell: &dyn Shell,
    home: &Path,
    declared: &Declared,
    harnesses: &[Harness],
) -> Vec<Difference> {
    let entries = declared.skills.as_deref().map(read_skills);
    let guards = declared.guards.as_deref().map(read_guards);
    let mut asked: BTreeMap<&str, Result<Vec<skills::Installed>, String>> = BTreeMap::new();
    let mut differences = vec![];
    for &harness in harnesses {
        let mut said = vec![];
        if let Some(file) = &declared.instructions
            && let Some(why) = instructions::unlinked(home, harness, file)
        {
            said.push(why);
        }
        match (&guards, harness.user_hooks()) {
            (Some(Err(why)), Some(_)) => said.push(why.clone()),
            (Some(Ok(guards)), Some(_)) => said.extend(guards::unwired(home, harness, guards)),
            _ => {}
        }
        match (&entries, harness.skills_agent()) {
            (Some(Err(why)), _) => said.push(why.clone()),
            (Some(Ok(entries)), Some(agent)) => {
                let installed = asked.entry(agent).or_insert_with(|| {
                    skills::listed(shell, agent).map_err(|error| format!("{error:#}"))
                });
                match installed {
                    Err(why) => said.push(why.clone()),
                    Ok(installed) => {
                        let (missing, extra) = skills::drift(entries, installed);
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
        if !said.is_empty() {
            differences.push(Difference { harness, said });
        }
    }
    differences
}

/// What giving a machine its setup did (`machine`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// The harnesses found on it.
    pub harnesses: Vec<Harness>,
    /// What was given them; `None` with nothing declared.
    pub done: Option<Done>,
    /// What was done of its tmux; `None` where nothing was to be done.
    pub tmux: Option<Tmux>,
}

/// Gives the machine whose user's home is `home` the declared setup, in one
/// call: finds its harnesses, gives each of them `declared`, and installs
/// tmux where it is a Guild machine with none. What a program with no host
/// kit of its own calls to give a machine the agent setup; `guild install`
/// calls `tmux::give` alone.
pub fn machine<S: Shell + Sync>(shell: &S, home: &Path, declared: &Declared, now: u64) -> Machine {
    let harnesses = found::harnesses(shell);
    let done = declared.any().then(|| give(shell, home, declared, &harnesses, now));
    let tmux = tmux::give(shell, home);
    Machine { harnesses, done, tmux }
}

/// How the machine whose user's home is `home` differs from `declared`: each
/// harness found on it that differs, and why it has no tmux where it is a
/// Guild machine with none.
pub fn report<S: Shell + Sync>(
    shell: &S,
    home: &Path,
    declared: &Declared,
) -> (Vec<Difference>, Option<String>) {
    let harnesses = found::harnesses(shell);
    let differ =
        if declared.any() { differences(shell, home, declared, &harnesses) } else { vec![] };
    (differ, tmux::missing(shell, home))
}

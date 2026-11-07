//! The host kit (docs/adr/0020): the harnesses this host can start, in order
//! of preference — each one's worker launch command and listed models.
//! This file belongs to the host user, outside every checkout. `afkconvoy kit`
//! and `run` refresh it with no human; `kit --remove` records the operator's
//! explicit choice, and `switch --harness` reverses it.
use std::collections::BTreeSet;
use std::fs::{OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use afkconvoy_core::harness::Harness;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Kit {
    /// An old host kit may still say `cast`. It is read and ignored: a write
    /// drops it, and it does not change a start.
    #[serde(default, skip_serializing)]
    pub cast: bool,
    /// The harnesses this host can start, in order of preference: the first
    /// is the host kit's first.
    pub harnesses: Vec<Held>,
    /// Harnesses the operator removed. Refresh and launch discovery keep them
    /// out until `switch --harness` restores them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<Harness>,
    /// Old kit choices are accepted on read and retired on the next write.
    #[serde(default, rename = "stock_consult", alias = "stock_inspector", skip_serializing)]
    pub _retired_consult: Option<Value>,
}

/// One harness the host kit holds.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Held {
    #[serde(deserialize_with = "harness")]
    pub harness: Harness,
    /// Its worker launch command.
    #[serde(default)]
    pub command: String,
    /// Its listed and researched models, in order of preference.
    #[serde(default)]
    pub models: Vec<Seen>,
}

impl Held {
    /// The models a worker may be started on, in order (`Seen::placed`).
    /// With none, a worker runs its login's default model, with no model flag.
    pub fn placed(&self) -> impl Iterator<Item = &Seen> {
        self.models.iter().filter(|seen| seen.placed())
    }
}

/// A model one harness lists, or a human named, and what research says of it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Seen {
    /// Its id, exactly as its harness lists it; a worker is started on it.
    pub model: String,
    /// Its display names and the other names research ties to it with a dated
    /// source. Any of them matches it (`casting::Held::model`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// Older kits recorded probe evidence here. It is read and forgotten.
    #[serde(default, skip_serializing)]
    probed: Option<Value>,
    /// What the sourced research says it is good at — what a caster is told
    /// of it — or `research pending` or `not researched`; none of one named
    /// with no research.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub good_at: Option<String>,
    /// Research's sourced reason this listed identity is unsuited to workers.
    /// Retained while listed, just like a placement; never a coding model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<String>,
    /// The dated URLs supporting the placement or settlement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<crate::kit_research::Source>,
    /// When the research succeeded, in epoch seconds; never refreshed by age.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub researched_at: Option<u64>,
}

impl Seen {
    /// Whether a worker may be started on it: research placed it with a dated
    /// source, or a human or an older release named it, with no research yet.
    /// Pending, not researched and settled models are never started on.
    pub fn placed(&self) -> bool {
        use crate::kit_research::{PENDING, UNKNOWN};
        self.settled.is_none() && !matches!(self.good_at.as_deref(), Some(PENDING | UNKNOWN))
    }

    /// Whether research has yet to answer for it: it is `research pending`,
    /// or held with no dated research. The watch supplies its research.
    pub fn awaits_research(&self) -> bool {
        if self.settled.is_some() {
            return false;
        }
        let sourced = !self.sources.is_empty() && self.researched_at.is_some();
        match self.good_at.as_deref() {
            Some(crate::kit_research::UNKNOWN | crate::kit_research::PENDING) | None => true,
            Some(good_at) => good_at.trim().is_empty() || !sourced,
        }
    }

    /// It as a range holds it: its id and its other names.
    pub fn as_model(&self) -> afkconvoy_core::casting::Model {
        afkconvoy_core::casting::Model { id: self.model.clone(), names: self.names.clone() }
    }
}

/// A harness and, where one is named, its model.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    #[serde(deserialize_with = "harness")]
    pub harness: Harness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub(crate) fn harness<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Harness, D::Error> {
    let key = String::deserialize(deserializer)?;
    Harness::read(&key).ok_or_else(|| serde::de::Error::custom(Harness::unknown(&key)))
}

impl Kit {
    /// The worker launch command of `harness`, where the host kit holds it.
    pub fn command(&self, harness: Harness) -> Option<&str> {
        self.held(harness).map(|held| held.command.as_str())
    }

    /// Every harness held with its command, in order of preference.
    pub fn commands(&self) -> impl Iterator<Item = (Harness, &str)> {
        self.harnesses.iter().map(|held| (held.harness, held.command.as_str()))
    }

    fn held(&self, harness: Harness) -> Option<&Held> {
        self.harnesses.iter().find(|held| held.harness == harness)
    }

    /// Whether the operator removed `harness` from the host kit.
    pub fn is_removed(&self, harness: Harness) -> bool {
        self.removed.contains(&harness)
    }

    /// Every operator removal, reported like any other harness left out.
    fn operator_left_out(&self) -> Vec<LeftOut> {
        self.removed.iter().map(|&harness| LeftOut::of(harness, None, REMOVED)).collect()
    }

    /// The host kit's first: its first harness, with that harness's first
    /// placed model — none, its default, while it has none placed. `None` of
    /// a host kit that holds no harness.
    pub fn first(&self) -> Option<Named> {
        let held = self.harnesses.first()?;
        Some(Named { harness: held.harness, model: held.placed().next().map(|m| m.model.clone()) })
    }

    /// The host kit as a range (docs/adr/0020): each harness in order, with
    /// its placed models in order — none, its default, while none is placed.
    pub fn range(&self) -> Vec<afkconvoy_core::casting::Held> {
        let held = |held: &Held| afkconvoy_core::casting::Held {
            harness: held.harness,
            models: held.placed().map(Seen::as_model).collect(),
        };
        self.harnesses.iter().map(held).collect()
    }

    /// The host kit as this host can start it now: without the harnesses
    /// `paused` on it (docs/adr/0020).
    pub fn without(&self, paused: &[afkconvoy_core::casting::Paused]) -> Kit {
        let is_paused = |harness: Harness| paused.iter().any(|pause| pause.harness == harness);
        Kit {
            cast: self.cast,
            harnesses: self
                .harnesses
                .iter()
                .filter(|held| !is_paused(held.harness))
                .cloned()
                .collect(),
            removed: self.removed.clone(),
            _retired_consult: None,
        }
    }

    /// Why this is no host kit, when it is not.
    fn fault(&self) -> Option<String> {
        let mut seen = BTreeSet::new();
        for held in &self.harnesses {
            if !seen.insert(held.harness.key()) {
                return Some(format!("it holds the harness `{}` twice", held.harness.key()));
            }
            if self.is_removed(held.harness) {
                return Some(format!(
                    "it holds `{}` and records it as removed by the operator",
                    held.harness.key()
                ));
            }
        }
        let mut removed = BTreeSet::new();
        for harness in &self.removed {
            if !removed.insert(harness.key()) {
                return Some(format!("it records `{}` as removed twice", harness.key()));
            }
        }
        None
    }
}

pub fn path() -> Result<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context(
            "cannot find the host kit: set HOME or XDG_CONFIG_HOME, then run `afkconvoy kit`",
        )?;
    Ok(root.join("afkconvoy/kit.json"))
}

pub fn load() -> Result<Kit> {
    load_at(&path()?)
}

/// Merge one discovered command into the latest kit. A concurrent explicit
/// command wins; models, research, preference order and the consult stay put.
pub fn discover(harness: Harness, command: &str) -> Result<String> {
    discover_at(&path()?, harness, command)
}

fn writing(file: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(file.parent().context("the host kit needs a parent directory")?)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(file.with_extension("refresh"))?;
    lock.lock()?;
    Ok(lock)
}

fn discover_at(file: &Path, harness: Harness, command: &str) -> Result<String> {
    let _writing = writing(file)?;
    let mut kit = if file.exists() { load_at(file)? } else { Kit::default() };
    if kit.is_removed(harness) {
        bail!(
            "the harness `{}` was removed by the operator; restore it with `afkconvoy switch --harness {}`",
            harness.key(),
            harness.key()
        );
    }
    if let Some(explicit) = kit.command(harness).filter(|command| !command.trim().is_empty()) {
        return Ok(explicit.to_string());
    }
    match kit.harnesses.iter_mut().find(|held| held.harness == harness) {
        Some(held) => held.command = command.to_string(),
        None => kit.harnesses.push(Held { harness, command: command.to_string(), models: vec![] }),
    }
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    store(file, &kit, now)?;
    Ok(command.to_string())
}

/// The host kit kept in `file`.
fn load_at(file: &Path) -> Result<Kit> {
    let unreadable =
        || format!("cannot read the host kit at {}: run `afkconvoy kit`", file.display());
    let text = std::fs::read_to_string(file).with_context(unreadable)?;
    let kit: Kit = serde_json::from_str(&text).with_context(unreadable)?;
    if let Some(fault) = kit.fault() {
        bail!("cannot read the host kit at {}: {fault}: run `afkconvoy kit`", file.display());
    }
    Ok(kit)
}

/// The host kit as this host can start it now: its file without the
/// harnesses paused on it (`Kit::without`), with them.
pub fn running() -> Result<(Kit, Vec<afkconvoy_core::casting::Paused>)> {
    let paused = crate::paused::read();
    Ok((crate::health::available(load()?.without(&paused))?, paused))
}

/// The host kit as `plan` shows it, including research notes and sources.
pub fn show(now: u64) -> Value {
    show_with_research(now, true)
}

/// The host kit as a look needs it: identities, commands and research state,
/// without the research prose that no finding reads.
pub fn show_for_look(now: u64) -> Value {
    show_with_research(now, false)
}

fn show_with_research(now: u64, research: bool) -> Value {
    let file = path().map(|path| path.display().to_string()).ok();
    let paused = crate::paused::read();
    let paused_said: Vec<Value> = paused
        .iter()
        .map(|pause| {
            json!({"harness": pause.harness, "since": pause.since, "why": pause.why,
                "probed": pause.probed, "said": crate::paused::said(pause, now)})
        })
        .collect();
    match load() {
        Ok(kit) => {
            let harnesses: Vec<Value> = kit
                .harnesses
                .iter()
                .map(|held| {
                    let mut shown = serde_json::to_value(held).expect("a held harness serializes");
                    if !research {
                        for model in shown["models"].as_array_mut().into_iter().flatten() {
                            if let Some(model) = model.as_object_mut() {
                                model.retain(|key, _| ["model", "names"].contains(&key.as_str()));
                            }
                        }
                    }
                    shown["default_model"] = json!(held.placed().next().is_none());
                    shown["research_pending"] =
                        json!(held.models.iter().any(Seen::awaits_research));
                    shown
                })
                .collect();
            json!({
                "path": file,
                "refreshed": refreshed(),
                "harnesses": harnesses,
                "left_out": kit.operator_left_out(),
                "paused": paused_said, "worker_health": crate::health::json(&crate::health::show()),
            })
        }
        Err(error) => {
            json!({"path": file, "error": format!("{error:#}"), "paused": paused_said})
        }
    }
}

/// Writes the host kit for `run` where this host has none (`write`), saying
/// so on stderr; then reads it. One thread of this process at a time, from
/// the check to the write: fleet instances started at once on a host with no
/// host kit write it once.
pub fn ensure(now: u64) -> Result<()> {
    static WRITING: Mutex<()> = Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !path()?.exists() {
        write(now)?;
    }
    load().map(|_| ())
}

/// Writes the host kit with no human (docs/adr/0020): everything this host
/// can start now (`compose`), whoever runs it and wherever. Says on stderr
/// where it is, what it holds and what was left out, and answers
/// `{path, written, kit, left_out}`.
pub fn write(now: u64) -> Result<Value> {
    let host = Host::here(now)?;
    let file = path()?;
    let _writing = writing(&file)?;
    let current = if file.exists() { load()? } else { Kit::default() };
    let (kit, left_out) = compose(&current, &host, Writing::Whole);
    store(&file, &kit, now)?;
    eprint!("{}", report(&file.display().to_string(), &kit, &left_out));
    drop(_writing);
    let harnesses: Vec<Harness> = kit.harnesses.iter().map(|held| held.harness).collect();
    let agent_setup = give_agent_setup(&harnesses, crate::agent_setup::To::Every, now);
    Ok(json!({"path": file, "written": true, "kit": kit, "left_out": left_out,
        "agent_setup": agent_setup}))
}

/// Gives the agent setup to `harnesses` (`agent_setup::converge`) → what it
/// did, null where nothing was to be done. What failed is said on stderr and
/// in the answer, and fails no write of the host kit: `doctor` names each
/// harness it left drifted.
fn give_agent_setup(harnesses: &[Harness], to: crate::agent_setup::To, now: u64) -> Value {
    match crate::agent_setup::converge(harnesses, to, now) {
        Ok(None) => Value::Null,
        Ok(Some(done)) => {
            for failed in done["failed"].as_array().into_iter().flatten() {
                eprintln!(
                    "the agent setup was not given to {}: {}",
                    failed["harness"].as_str().unwrap_or_default(),
                    failed["why"].as_str().unwrap_or_default()
                );
            }
            done
        }
        Err(error) => {
            eprintln!("the agent setup was not given: {error:#}");
            json!({"error": format!("{error:#}")})
        }
    }
}

/// Gives the agent setup to each harness the host kit holds that has not
/// been given it as it is declared now: a tick's, after its refresh of the
/// host kit, so a harness held since — by a refresh, a switch or a launch
/// that found it — gets it with no step by the operator.
/// What it did is a line of the run log of `repo`.
pub fn give_new_agent_setup(
    repo: &crate::git::Repo,
    instance: &str,
    host: &str,
    now: u64,
) -> Result<()> {
    let kit = load()?;
    let harnesses: Vec<Harness> = kit.harnesses.iter().map(|held| held.harness).collect();
    let done = give_agent_setup(&harnesses, crate::agent_setup::To::New, now);
    if done.is_null() {
        return Ok(());
    }
    let failed = done.get("error").is_some()
        || done["failed"].as_array().is_some_and(|failed| !failed.is_empty());
    let entry = crate::log::Entry {
        at: now,
        instance: instance.to_string(),
        host: host.to_string(),
        what: crate::log::What::Command {
            command: "agent_setup".into(),
            on: None,
            by: "fleet".into(),
            outcome: format!("the agent setup was given: {done}"),
            failed,
        },
    };
    crate::log::Log::of(repo)?.append(&entry)
}

/// Why an operator-removed harness stays out of the host kit.
pub const REMOVED: &str = "removed by the operator";

/// Takes `harness`, its command and its models out of the host kit and records
/// the operator's removal. Live fleet workers on it refuse the removal and are
/// left alone. Only `switch --harness` restores it.
pub fn remove(harness: Harness, now: u64) -> Result<Value> {
    let file = path()?;
    let _writing = writing(&file)?;
    let _mutation = mutation_lock(&file)?;
    let mut kit = if file.exists() { load_at(&file)? } else { Kit::default() };
    if !kit.is_removed(harness) {
        let mut running = vec![];
        for on in crate::orca::Worktree::on_host()?.into_iter().filter(|on| on.held) {
            let worktree = crate::orca::Worktree::at(on.path.clone(), String::new());
            if worktree.holds_a_worker() && worktree.harness()? == Some(harness) {
                running.push(on.issue.map_or_else(
                    || on.path.display().to_string(),
                    |issue| format!("issue #{issue} ({})", on.path.display()),
                ));
            }
        }
        if !running.is_empty() {
            let alternative = kit.harnesses.iter().find(|held| held.harness != harness);
            let switch = alternative.map_or_else(
                || "afkconvoy switch --all --harness <harness>".to_string(),
                |held| format!("afkconvoy switch --all --harness {}", held.harness.key()),
            );
            bail!(
                "cannot remove `{}`: workers are still running on it: {}; move them first with `{switch}`; no worker was ended",
                harness.key(),
                running.join(", ")
            );
        }
    }
    kit.harnesses.retain(|held| held.harness != harness);
    push_new(&mut kit.removed, [harness]);
    store_unlocked(&file, &kit, now)?;
    let left_out = kit.operator_left_out();
    eprint!("{}", report(&file.display().to_string(), &kit, &left_out));
    Ok(json!({"path": file, "written": true, "removed": harness, "kit": kit, "left_out": left_out}))
}

/// Applies the watch's research against the latest kit under both write locks,
/// so listing, casting and a switch cannot lose each other's changes. A bad
/// file never reaches the atomic write, nor changes the kit's refresh time.
pub fn apply_research(research: &Path, now: u64) -> Result<Value> {
    let text = std::fs::read_to_string(research)
        .with_context(|| format!("cannot read research file {}", research.display()))?;
    let file = path()?;
    let _writing = writing(&file)?;
    let _mutation = mutation_lock(&file)?;
    let mut kit = load_at(&file)?;
    let applied = crate::kit_research::apply(&mut kit, &text, now).with_context(|| {
        format!(
            "invalid research file {}; correct it and run `afkconvoy kit --research <file>`",
            research.display()
        )
    })?;
    let refreshed = written(&file).unwrap_or(now);
    store_unlocked(&file, &kit, refreshed)?;
    Ok(json!({"path": file, "written": true, "placed": applied.placed,
        "settled": applied.settled, "kit": kit}))
}

/// Lists only `harness`, merging the result into the latest
/// kit without changing its order. A harness added by a switch stays
/// command-only when listing fails, so its start fault remains visible.
pub fn write_one(harness: Harness, now: u64) -> Result<Value> {
    let host = Host::here(now)?;
    let file = path()?;
    let _writing = writing(&file)?;
    let mut current = load_at(&file)?;
    let found = |candidate| {
        if candidate == harness {
            (host.found)(candidate)
        } else {
            Err("not part of this targeted refresh".into())
        }
    };
    let only = Host { found: &found, ..host };
    let before = Kit {
        cast: current.cast,
        harnesses: current
            .harnesses
            .iter()
            .filter(|held| held.harness == harness)
            .cloned()
            .collect(),
        removed: current.removed.clone(),
        _retired_consult: None,
    };
    let (mut repaired, left_out) = compose(&before, &only, Writing::Refresh);
    let listed = repaired.harnesses.first().is_some_and(|held| !held.models.is_empty());
    if listed {
        if let Some(held) = current.harnesses.iter_mut().find(|held| held.harness == harness) {
            *held = repaired.harnesses.remove(0);
        }
        store(&file, &current, now)?;
    }
    eprint!("{}", report(&file.display().to_string(), &current, &left_out));
    drop(_writing);
    let agent_setup = give_agent_setup(&[harness], crate::agent_setup::To::New, now);
    Ok(json!({"path": file, "written": listed, "kit": current, "left_out": left_out,
        "agent_setup": agent_setup}))
}

/// Gives the agent setup to every harness the host kit holds: a `run` that
/// changed what the host settings declare of it.
pub fn give_every_agent_setup(now: u64) -> Value {
    let Ok(kit) = load() else { return Value::Null };
    let harnesses: Vec<Harness> = kit.harnesses.iter().map(|held| held.harness).collect();
    give_agent_setup(&harnesses, crate::agent_setup::To::Every, now)
}

/// Runs a targeted refresh in a process detached from the switch's answer.
/// The caller invokes this only after writing that answer to stdout.
pub fn refresh_after_answer(harness: Harness) -> Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args(["kit", "--harness", harness.key()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::process::spawn(&mut command)?;
    Ok(())
}

/// Resolves a switch before any mutation. A model must identify exactly one
/// harness as well as one model within it; all its usual names match.
pub fn switch_target(harness: Option<Harness>, model: Option<&str>) -> Result<Named> {
    if let Some(harness) = harness {
        return Ok(Named { harness, model: None });
    }
    let name = model.context("name a kit harness with --harness or a model with --model")?;
    let range = load()?.range();
    let mut matches = vec![];
    let mut faults = vec![];
    for held in &range {
        match held.model(name) {
            Ok(model) => {
                matches.push(Named { harness: held.harness, model: Some(model.id.clone()) })
            }
            Err(why) => faults.push(why.said(name, held.harness)),
        }
    }
    if matches.len() == 1
        && !range.iter().any(|held| {
            matches!(held.model(name), Err(afkconvoy_core::casting::Unmatched::Two(..)))
        })
    {
        return Ok(matches.remove(0));
    }
    let candidates: Vec<_> = matches.iter().map(named_said).collect();
    bail!(
        "cannot switch to model `{name}`; name one model by its id{}: {}",
        if candidates.is_empty() {
            String::new()
        } else {
            format!("; candidates: {}", candidates.join(", "))
        },
        faults.join("; ")
    )
}

/// Promotes a harness and optionally one of its models, preserving all
/// contents, other relative orders and probe age under one lock. An installed
/// harness not held yet, including one the operator removed, is added first on
/// its stock command with no model and starts on its login's default while its
/// targeted refresh runs afterwards.
pub fn prefer(harness: Harness, model: Option<&str>) -> Result<Value> {
    let file = path()?;
    let _lock = mutation_lock(&file)?;
    let mut kit = load_at(&file)?;
    let before: Vec<_> = kit.harnesses.iter().map(|held| held.harness).collect();
    let restored = kit.is_removed(harness);
    kit.removed.retain(|removed| *removed != harness);
    let at = kit.harnesses.iter().position(|held| held.harness == harness);
    let model_at = model
        .map(|name| {
            let at = at.context("a model switch names an existing kit harness")?;
            kit.harnesses[at].models.iter().position(|seen| seen.model == name).with_context(|| {
                format!(
                    "the host kit no longer holds model `{name}` of {}; run switch again",
                    harness.key()
                )
            })
        })
        .transpose()?;
    let added = at.is_none();
    let changed = restored || added || at != Some(0) || model_at.is_some_and(|at| at != 0);
    if changed {
        let held = match at {
            Some(at) => kit.harnesses.remove(at),
            None => Held { harness, command: harness.stock().to_string(), models: vec![] },
        };
        kit.harnesses.insert(0, held);
        let mut raw: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
        if let Some(removed) = raw.get_mut("removed").and_then(Value::as_array_mut) {
            removed.retain(|removed| removed != &json!(harness));
            if removed.is_empty() {
                raw.as_object_mut().context("kit must be an object")?.remove("removed");
            }
        }
        let entries = raw["harnesses"].as_array_mut().context("kit harnesses must be an array")?;
        let held = match at {
            Some(at) => entries.remove(at),
            None => json!({"harness": harness, "command": harness.stock(), "models": []}),
        };
        entries.insert(0, held);
        if let Some(at) = model_at {
            let models =
                entries[0]["models"].as_array_mut().context("kit models must be an array")?;
            let model = models.remove(at);
            models.insert(0, model);
        }
        let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(&raw)?)?;
        stamp(&temporary, written(&file).unwrap_or_else(crate::clock))?;
        std::fs::rename(&temporary, &file)?;
    }
    Ok(json!({"harness": harness, "model": model, "path": file, "order_before": before,
        "order": kit.harnesses.iter().map(|held| held.harness).collect::<Vec<_>>(),
        "changed": changed, "added": added, "restored": restored}))
}

fn mutation_lock(file: &Path) -> Result<std::fs::File> {
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(file.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

/// Writes `kit` to `file` at once, as written `now`: the time its file
/// keeps as when it was last modified, which says when the host kit was last
/// written (`written`).
fn store(file: &Path, kit: &Kit, now: u64) -> Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = mutation_lock(file)?;
    // Promotion may have happened while listing ran.
    let mut kit = kit.clone();
    if let Ok(latest) = load_at(file) {
        kit.cast = latest.cast;
        kit.removed = latest.removed.clone();
        kit.harnesses.retain(|held| !latest.is_removed(held.harness));
        kit.harnesses.sort_by_key(|held| {
            latest
                .harnesses
                .iter()
                .position(|old| old.harness == held.harness)
                .unwrap_or(usize::MAX)
        });
        retain_model_order(&latest, &mut kit);
    }
    store_unlocked(file, &kit, now)
}

/// Established model order is user priority, whether researched or switched:
/// the order of the models placed before. New discoveries, and models only
/// now placed, follow them in the order they come, never displacing the first.
pub(super) fn retain_model_order(before: &Kit, kit: &mut Kit) {
    for held in &mut kit.harnesses {
        if let Some(old) = before.held(held.harness) {
            held.models.sort_by_key(|model| {
                old.placed().position(|seen| seen.model == model.model).unwrap_or(usize::MAX)
            });
        }
    }
}

fn store_unlocked(file: &Path, kit: &Kit, now: u64) -> Result<()> {
    let parent = file.parent().context("the host kit needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(kit)?)?;
    stamp(&temporary, now)?;
    std::fs::rename(&temporary, file)?;
    Ok(())
}

/// Has `file` say it was last modified `now`.
fn stamp(file: &Path, now: u64) -> Result<()> {
    let at = UNIX_EPOCH + Duration::from_secs(now);
    OpenOptions::new()
        .write(true)
        .open(file)
        .and_then(|opened| opened.set_modified(at))
        .with_context(|| format!("cannot stamp {}", file.display()))
}

/// When `file` was last modified, in epoch seconds; `None` of one that is
/// not there.
fn written(file: &Path) -> Option<u64> {
    let modified = std::fs::metadata(file).and_then(|meta| meta.modified()).ok()?;
    Some(modified.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs()))
}

/// How long the host kit stands before a tick writes it again.
pub const REFRESHED_EVERY: u64 = 86_400;

/// How long after a refresh that could not be made the next is tried.
const TRIED_AGAIN_AFTER: u64 = 3_600;

/// Whether the host kit last written `written` is due to be written again
/// `now`, its last refresh having been tried `tried`.
fn due(written: Option<u64>, tried: Option<u64>, now: u64) -> bool {
    let older = |at: Option<u64>, than: u64| at.is_none_or(|at| now.saturating_sub(at) >= than);
    written.is_some() && older(written, REFRESHED_EVERY) && older(tried, TRIED_AGAIN_AFTER)
}

/// What a refresh of the host kit changed: each model, or harness, added
/// and dropped, as `harness model`.
#[derive(Debug, Default, PartialEq)]
pub struct Changed {
    pub added: Vec<String>,
    pub dropped: Vec<String>,
}

impl Changed {
    fn between(before: &Kit, after: &Kit) -> Changed {
        let held = |kit: &Kit| -> Vec<String> {
            let each = |held: &Held| -> Vec<String> {
                let named =
                    |seen: &Seen| Named { harness: held.harness, model: Some(seen.model.clone()) };
                held.models.iter().map(|seen| named_said(&named(seen))).collect()
            };
            kit.harnesses.iter().flat_map(each).collect()
        };
        let (before, after) = (held(before), held(after));
        let missing = |from: &[String], of: &[String]| -> Vec<String> {
            of.iter().filter(|said| !from.contains(said)).cloned().collect()
        };
        Changed { added: missing(&before, &after), dropped: missing(&after, &before) }
    }

    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.dropped.is_empty()
    }

    /// In words, as the run log says it.
    pub fn said(&self) -> String {
        let mut said = vec![];
        if !self.added.is_empty() {
            said.push(format!("added {}", self.added.join(", ")));
        }
        if !self.dropped.is_empty() {
            said.push(format!("dropped {}", self.dropped.join(", ")));
        }
        format!("the host kit was written again: {}", said.join("; "))
    }
}

/// Writes the host kit in `file` again where it is due `now` (`due`), with
/// what `host` finds (`Writing::Refresh`) → the host kit before and after;
/// `None` where it was not due, there is no host kit, or another process of
/// the host is writing it again. Every fleet of the host shares the one file, and the
/// lock beside it: one of them writes it again, once a day, and the others
/// read what it wrote.
fn refresh_at<'a>(
    file: &Path,
    now: u64,
    host: impl FnOnce() -> Result<Host<'a>>,
) -> Result<Option<(Kit, Kit)>> {
    // The lock holds when a refresh was last tried, in epoch seconds.
    let lock = file.with_extension("refresh");
    let tried = || std::fs::read_to_string(&lock).ok().and_then(|text| text.trim().parse().ok());
    // Command discovery used to leave Codex with no models until the daily
    // refresh. Repair those entries without waiting a day or restarting a
    // runner, retaining the ordinary backoff when discovery cannot answer.
    let is_due = || {
        let incomplete = load_at(file)
            .ok()
            .is_some_and(|kit| kit.held(Harness::Codex).is_some_and(|held| held.models.is_empty()));
        due(written(file), tried(), now)
            || (incomplete && tried().is_none_or(|at| now.saturating_sub(at) >= TRIED_AGAIN_AFTER))
    };
    if !is_due() {
        return Ok(None);
    }
    let held = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .with_context(|| format!("cannot open {}", lock.display()))?;
    match held.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(error)) => {
            return Err(error).with_context(|| format!("cannot lock {}", lock.display()));
        }
    }
    // Another process may have written it again since it was found due.
    if !is_due() {
        return Ok(None);
    }
    std::fs::write(&lock, now.to_string())
        .with_context(|| format!("cannot write {}", lock.display()))?;
    let current = load_at(file)?;
    let host = host()?;
    if !due(written(file), None, now) {
        // An early repair is only about Codex. In particular it must not
        // normalize, drop or reorder any other command-only kit entry.
        let found = |harness| match harness {
            Harness::Codex => (host.found)(harness),
            _ => Err("not part of the Codex repair".into()),
        };
        let only = Host { found: &found, ..host };
        let before = Kit {
            cast: current.cast,
            harnesses: current.held(Harness::Codex).cloned().into_iter().collect(),
            removed: current.removed.clone(),
            _retired_consult: None,
        };
        let (repaired, _) = compose(&before, &only, Writing::Refresh);
        if repaired.held(Harness::Codex).is_none_or(|held| held.models.is_empty()) {
            return Ok(Some((current.clone(), current)));
        }

        let mut kit = current.clone();
        if let Some(held) = kit.harnesses.iter_mut().find(|held| held.harness == Harness::Codex)
            && let Some(repaired) = repaired.held(Harness::Codex)
        {
            *held = repaired.clone();
        }
        store(file, &kit, now)?;
        return Ok(Some((current, kit)));
    }
    let (kit, _) = compose(&current, &host, Writing::Refresh);
    store(file, &kit, now)?;
    drop(held);
    Ok(Some((current, kit)))
}

/// Writes this host's host kit again once a day (`refresh_at`), as a tick of
/// `instance` on `host` in `repo` does before anything else, and appends a
/// line to the run log of `repo` where that changed it, naming what was added
/// and dropped, or where it failed. No model is sent a prompt.
pub fn refresh(repo: &crate::git::Repo, instance: &str, host: &str, now: u64) -> Result<()> {
    let refreshed = refresh_at(&path()?, now, || Host::here(now))
        .map(|refreshed| refreshed.map(|(before, after)| Changed::between(&before, &after)));
    let (outcome, failed) = match refreshed {
        Ok(None) => return Ok(()),
        Ok(Some(changed)) if changed.is_empty() => return Ok(()),
        Ok(Some(changed)) => (changed.said(), false),
        Err(error) => (
            format!("the host kit could not be written again: {error:#}; tried again in an hour"),
            true,
        ),
    };
    let entry = crate::log::Entry {
        at: now,
        instance: instance.to_string(),
        host: host.to_string(),
        what: crate::log::What::Command {
            command: "refresh_kit".into(),
            on: None,
            by: "fleet".into(),
            outcome,
            failed,
        },
    };
    crate::log::Log::of(repo)?.append(&entry)
}

/// When this host's host kit was last written — by `kit`, by `run` on a host
/// without one, or by a tick's daily refresh — in epoch seconds.
pub fn refreshed() -> Option<u64> {
    written(&path().ok()?)
}

/// One model a harness lists: the id it lists it by, and the name it shows
/// it by, where it shows one (`Harness::shown`).
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub name: String,
    pub shown: Option<String>,
}

/// The models a harness offers through a command; why it offers none.
type Listing = dyn Fn(Harness, &str) -> Result<Vec<Listed>, String> + Sync;

/// What the host kit is written from: this host, as the login shell a worker
/// gets finds it. No terminal is opened.
pub struct Host<'a> {
    /// Whether a harness's program is found; why it is taken as not.
    found: &'a (dyn Fn(Harness) -> Result<(), String> + Sync),
    /// The models a harness offers through a command, in its order; why it
    /// offers none.
    listed: &'a Listing,
}

impl Host<'static> {
    /// This host, as the login shell a worker gets finds it.
    fn here(_now: u64) -> Result<Host<'static>> {
        Ok(Host { found: &crate::launch::found, listed: &crate::launch::models })
    }
}

/// How the host kit held is written again.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Writing {
    /// Whole: every harness is found and listed again; a failure leaves it out.
    Whole,
    /// A tick's daily refresh: newly listed models wait for research and ones
    /// no longer listed are dropped. A harness that cannot be found or listed
    /// is held as it was, for a pause to leave out (docs/adr/0020).
    Refresh,
}

/// A harness, or one model of it, the host kit does not hold, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LeftOut {
    pub harness: Harness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub why: String,
}

impl LeftOut {
    fn of(harness: Harness, model: Option<&str>, why: impl Into<String>) -> LeftOut {
        LeftOut { harness, model: model.map(str::to_string), why: why.into() }
    }
}

/// The host kit this host can start now, from `current`, with no model sent
/// a prompt (docs/adr/0020) — and what was left out of it, and why. Every
/// listed id is held at once on the default model until research places it.
/// Established harness and model order and research are
/// retained. A refresh drops ids no longer listed, but keeps a harness whose
/// listing fails so worker health, not refresh, judges its availability.
fn compose(current: &Kit, host: &Host, writing: Writing) -> (Kit, Vec<LeftOut>) {
    let mut order: Vec<Harness> = current.harnesses.iter().map(|held| held.harness).collect();
    push_new(&mut order, Harness::ALL);
    order.retain(|harness| !current.is_removed(*harness));
    let command = |harness: Harness| {
        current.command(harness).map_or_else(|| harness.stock().to_string(), str::to_string)
    };
    let (found, listing) = (host.found, host.listed);
    let listed: Vec<(Harness, String, Result<Vec<Listed>, String>)> = std::thread::scope(|scope| {
        let asking: Vec<_> = order
            .iter()
            .map(|&harness| {
                let command = command(harness);
                scope.spawn(move || {
                    let listed = found(harness)
                        .map_err(|why| format!("not installed: {why}"))
                        .and_then(|()| {
                            listing(harness, &command)
                                .map_err(|why| format!("listing failed: {why}"))
                        });
                    (harness, command, listed)
                })
            })
            .collect();
        asking
            .into_iter()
            .zip(&order)
            .map(|(asked, &harness)| {
                asked.join().unwrap_or_else(|_| {
                    (harness, command(harness), Err("listing failed: it panicked".to_string()))
                })
            })
            .collect()
    });

    let mut left_out = current.operator_left_out();
    let mut harnesses: Vec<Held> = vec![];
    let refresh = writing == Writing::Refresh;
    for (harness, command, listed) in listed {
        let listed = match (listed, current.held(harness)) {
            (Ok(listed), _) => listed,
            (Err(_), Some(held)) if refresh => {
                harnesses.push(held.clone());
                continue;
            }
            (Err(why), _) => {
                left_out.push(LeftOut::of(harness, None, why));
                continue;
            }
        };
        let mut models: Vec<Seen> = vec![];
        for each in listed {
            if models.iter().any(|seen| seen.model == each.name) {
                continue;
            }
            let before = current
                .held(harness)
                .and_then(|held| held.models.iter().find(|seen| seen.model == each.name));
            // Research answered for its identity is kept, from any harness;
            // else it is as held here, or `research pending` where it is new.
            let researched = previous(current, &each.name);
            let pending = || Some(crate::kit_research::PENDING.to_string());
            let mut seen = Seen {
                model: each.name,
                names: before.map(|seen| seen.names.clone()).unwrap_or_default(),
                probed: None,
                // A legacy named model keeps running, but unsourced words
                // must not become casting guidance while the watch researches it.
                good_at: researched.or(before).map_or_else(pending, |seen| {
                    if seen.awaits_research() && seen.placed() {
                        None
                    } else {
                        seen.good_at.clone()
                    }
                }),
                settled: researched.and_then(|seen| seen.settled.clone()),
                sources: researched.map(|seen| seen.sources.clone()).unwrap_or_default(),
                researched_at: researched.and_then(|seen| seen.researched_at),
            };
            if let Some(display) = each.shown {
                push_new(&mut seen.names, [display]);
            }
            seen.names.retain(|name| name != &seen.model);
            models.push(seen);
        }
        if let Some(before) = current.held(harness) {
            for seen in &before.models {
                if !models.iter().any(|kept| kept.model == seen.model) {
                    left_out.push(LeftOut::of(harness, Some(&seen.model), "no longer listed"));
                }
            }
        }
        harnesses.push(Held { harness, command, models });
    }

    let mut kit = Kit {
        cast: current.cast,
        harnesses,
        removed: current.removed.clone(),
        _retired_consult: None,
    };
    retain_model_order(current, &mut kit);
    (kit, left_out)
}

/// The same listed identity held before that research answered for,
/// including on another harness, sourced research first. Aliases are
/// excluded: research ties them to a listed id instead.
fn previous<'a>(current: &'a Kit, id: &str) -> Option<&'a Seen> {
    use crate::kit_research::identity;
    current
        .harnesses
        .iter()
        .flat_map(|held| &held.models)
        .filter(|before| identity(&before.model) == identity(id) && !before.awaits_research())
        .max_by_key(|before| (before.researched_at.is_some(), !before.sources.is_empty()))
}

/// Adds to `into` each of `items` it does not hold yet.
fn push_new<T: PartialEq>(into: &mut Vec<T>, items: impl IntoIterator<Item = T>) {
    for item in items {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

/// What writing the host kit at `path` says on stderr: where it is, what it
/// holds in order, and what was left out.
fn report(path: &str, kit: &Kit, left_out: &[LeftOut]) -> String {
    let mut said = format!("afkconvoy: wrote the host kit at {path}\n");
    for held in &kit.harnesses {
        let models = |of: &dyn Fn(&Seen) -> bool| -> Vec<&str> {
            held.models.iter().filter(|seen| of(seen)).map(|seen| seen.model.as_str()).collect()
        };
        let placed = models(&Seen::placed);
        let mut line = match placed.is_empty() {
            true => "its default model".to_string(),
            false => placed.join(" "),
        };
        for (said, good_at) in [
            ("research pending", crate::kit_research::PENDING),
            ("not researched", crate::kit_research::UNKNOWN),
        ] {
            let these = models(&|seen| seen.good_at.as_deref() == Some(good_at));
            if !these.is_empty() {
                line.push_str(&format!("; {said}: {}", these.join(" ")));
            }
        }
        let settled = models(&|seen| seen.settled.is_some());
        if !settled.is_empty() {
            line.push_str(&format!("; settled: {}", settled.join(" ")));
        }
        said.push_str(&format!("  {} `{}`: {line}\n", held.harness.key(), held.command));
    }
    if kit.harnesses.is_empty() {
        said.push_str("  it holds no harness: no worker can be started\n");
    }
    for out in left_out {
        let named = Named { harness: out.harness, model: out.model.clone() };
        said.push_str(&format!("  left out: {}: {}\n", named_said(&named), out.why));
    }
    said
}

/// A harness and model as the report says them.
fn named_said(named: &Named) -> String {
    match &named.model {
        Some(model) => format!("{} {model}", named.harness.key()),
        None => named.harness.key().to_string(),
    }
}

/// What `afkconvoy kit --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When a harness is installed or removed, a login changes, or its models are to be listed again. Research comes from the watch, through `afkconvoy kit --research <file>`. `run` writes the host kit itself on a host that has none, and a tick writes it again once a day (below). It needs no repo and changes no remote.

## Who runs it

The operator, or a machine in their place, on the host that starts the workers: it asks nothing, at a terminal or not, and does the same whoever runs it.

## What it reads and writes

The host kit is $XDG_CONFIG_HOME/afkconvoy/kit.json, or $HOME/.config/afkconvoy/kit.json when XDG_CONFIG_HOME is unset. It is kept on this host and never pushed. It is the only source of worker launch commands. It holds the harnesses this host can start, in order of preference, each with its worker launch command and listed models. Former `stock_consult` kit choices are accepted on read and dropped on write. It is written automatically, with no confirmation:

1. Harnesses. Each known harness whose program `type` finds in the login shell a worker gets — `$SHELL -lic`, or `/bin/zsh` where `SHELL` is unset, with its rc files, PATH and logins, started from the environment a fresh login brings in and not from this process's — on the command the host kit holds for it, or else its stock worker launch command. No terminal is opened.
2. Models. Each harness found is asked for the models its own login runs, in that same login shell, and no terminal is opened. Claude Code is asked through its command with an `initialize` request and no prompt; Codex through its command with `debug models`, its server's list; pi through its command with `--list-models`; Kimi Code through its command with `provider list --json`, under the fleet's isolated home; Grok through its command with `models`, under the fleet's isolated home and with no prompt; DSH through `dsh headless`, whose listing plugin asks `https://api.deepseek.com/models` with DSH's account token from `$DSH_HOME/.credentials.yaml` (default `~/.dsh`), or its `DEEPSEEK_API_KEY` when no account token is held. The server's ids and display names are used, never a bundled catalog. This is the sole credential the fleet reads, through DSH's credential service, only for listing: never printed or stored, and sent only to DeepSeek. Not signed in or a server refusal leaves DSH out with the reason.
3. Default first. No model is sent a prompt. A harness found and listed is held at once, each model it newly lists `research pending`, which no worker is started on. A harness with no placed model starts its workers on its own login's default model, with no model flag. Codex automatic workers instead read the user's current model preference from `~/.codex/config.toml`, unless an issue has a model label; with no preference they use Codex's native default. `plan` and `doctor` report the kit's `default_model` and `research_pending` independently of that preference.
4. Names. A model's id is exactly the id its harness lists, and a worker is started on it. Its `names` are the names its listing shows it by (Claude Code's `Opus 5.5`) and the aliases research ties to it with a dated source.
5. Order. Established harness and model order is retained, including switches made while listing or research run. New harnesses follow in this fixed order: claude-code, pi, codex, dsh, kimi, grok. New models follow established models; research may rank discoveries without displacing established priority. Harness order is this list. A worker's model is its issue's one `model:` label of the selected harness, where that label is followed, otherwise that harness's user default. A harness with no placed model starts with no model flag.
6. Research. The watch researches listed identities using its web tools, once across harnesses, and writes a JSON file through `afkconvoy kit --research <file>`. The kit itself calls no model. Unplaced models are a doctor finding: a watch started for other work takes them too; research alone starts one at most once a day. A harness with no placed model stays on its user default. Sourced research is kept as good_at, names, sources ({url, date}) and researched_at (epoch seconds), and is never refreshed on a schedule. `afkconvoy draft` copies good_at into a roster.

`afkconvoy kit --remove <harness>` takes the harness, its command and its models out and records `removed: [<harness>]` in the host kit. No worker, patrol, landing turn or watch later starts on it, including as a fallback. Relisting, the daily refresh and worker-launch discovery leave it out even while its program remains installed. Only `afkconvoy switch --harness <harness>` clears the removal and puts it back first. Removing a harness with live fleet workers is refused: the error names their issues or worktrees and gives the `afkconvoy switch --all --harness <harness>` line that moves them first. Removal ends no worker.

A model named by a roster entry or a `model:` label matches a placed model the host kit holds for its harness when it is its id or one of its names, whatever the case; with the provider prefix left off where that leaves one match (`gpt-6-sol` for pi's `openai-codex/gpt-6-sol`). A name with two matches matches neither, and says which two; one with none says the nearest names.

`afkconvoy kit --cast` is refused: automatic casting is gone. An old host-kit `cast` key is ignored, changes no start, and is dropped the next time the kit is written. It does not fail `kit` or `check`. A worker's model is its one `model:` label of the selected harness, or that harness's user default (`afkconvoy cast --help`).

Without --cast, --research or --remove it says on stderr where the file is, what it holds — each harness's placed models, or its default model, and those `research pending`, `not researched` or settled — and what was left out.

## What it answers

With --cast: an error, automatic casting is gone. With --research: {path, written, placed, settled, kit}, the host kit file, true, each model placed or settled as {harness, model} in its respective array, and the resulting kit. With --remove: {path, written, removed, kit, left_out}, with removed the harness key and the kit without it. Otherwise {path, written, kit, left_out}: the file; true; the kit written; and each harness or model not kept, as {harness, model, why} — model absent for a whole harness — why `not installed`, `listing failed`, `no longer listed`, or `removed by the operator`. `plan` and `doctor` show operator removals in `host_kit.left_out` with that same reason. Run twice with nothing changed, it writes the same kit. The written kit has no `cast` key. Errors: the login shell did not answer within 20 seconds; a kit that cannot be read or written; or a removal refused because live workers are still on the harness.

A harness with `models` [] or no placed model starts on its default. Automatic command discovery can append a harness with no models unless the operator removed it; one written by hand or by an older release is also read as it is: a model named in it with no research is started on until research answers for it, when the watch supplies its research. Neither the kit nor `left_out` carries probe evidence any more: an older kit's `probed` fields are read and not written again.

## The research file

{const:KIT_RESEARCH_STATES}

`afkconvoy kit --research research.json` reads the existing host kit, checks the whole file, and atomically writes the placements and settlements. It needs neither a checkout nor orca. Read `afkconvoy doctor --all` for current ids and names. To place a model, use:

```json
{"models":[{"harness":"claude-code","model":"listed-id","good_at":"wide refactors; slower on small fixes","names":["sourced alias"],"sources":[{"url":"https://vendor.example/model-card","date":"2026-10-26"}]}]}
```

To settle an unsuitable model, replace good_at and names with a nonempty settled reason:

```json
{"models":[{"harness":"codex","model":"codex-auto-review","settled":"approval reviewer, unsuited to workers","sources":[{"url":"https://vendor.example/reviewer","date":"2026-10-26"}]}]}
```

Each entry names an exact listed id of a held harness. Name each identity once across harnesses: the same id without its provider prefix shares research, including settlements. Each entry supplies either nonempty good_at describing coding strengths and weaknesses, with optional nonempty sourced names, or a settled reason with no names. Every entry needs at least one HTTP or HTTPS source URL and its publication or update date as a calendar date YYYY-MM-DD. Use primary and high-trust sources, with live web search. The models array ranks newly placed models, after established model priority; harness priority, commands and existing dated research stay as held. Models omitted stay as held for a later watch.

Refusals name the file and fault and write nothing: malformed JSON, unknown fields, unknown harnesses or ids, repeated identities, conflicting outcomes, empty good_at, settled reason or names, missing or malformed dated sources, or a file answering no model awaiting research. Correct the file and run the command again. The answer lists every placement and settlement, including identities shared by another harness. Research does not change when the models were last listed (`refreshed`).

## What trips you up

A harness or model left out is gone from the file: commands and research are kept only for what is kept. At a worker launch, a known harness with no command is discovered in the worker's login shell and its checked stock command is merged into the kit, without model probes or research, unless the operator removed it. An explicit command always wins. `--worker-command` is gone: a harness's command is the host kit's. A command is an opaque shell string; an unbound repo starts it under nice with its worktree's TMPDIR, adding its harness's model flag only where a model label is followed, and its harness's worktree-trust setup. With no followed label the worker starts with no model flag, on that harness's user default. Codex also receives a session-private user directory and the effort and explicit service tier read from the host user's own Codex config at each launch; no saved worker preference selects them. A bound repo adds its role flags ([`afkconvoy help worker-launch`](help:worker-launch)).

Writing the kit sends no prompt to any listed model. A listed model that fails when its worker starts is found by worker health, classed `model` at model scope and left out; its hourly recovery probe brings it back. A harness refusing every prompt is paused and has its own hourly recovery probe. Those health probes are the only model probes; kit writes, switches and refreshes make none. A recovery probe asks that same login shell and opens no terminal. A pause is running state and recovers; an operator removal is a choice and no probe undoes it.

## It stays current

A tick — of `run`, or `afkconvoy tick` — writes the host kit again once its file was last written a day ago or more, before anything else: a newly listed model waits for the watch, and a model no longer listed is dropped. A harness that cannot be found or listed is kept, and a pause leaves it out, as above. An operator removal survives this refresh and every other relisting. A harness merely left out by a hand edit is read as absent, but only `kit --remove` records the removal that survives a refresh; the next relisting may add a hand-omitted installed harness back. The file's modification time is when it was last written — by `kit`, by `run` on a host without one, or by a tick — and `plan` and `doctor` answer it as `host_kit.refreshed`. Every fleet of the host shares the file and the lock beside it (kit.refresh): one of them writes it again, once a day, and the others read what it wrote. A refresh that changed the host kit is a line of the run log, `refresh_kit (fleet): the host kit was written again: added claude-code sonnet; dropped claude-code opus`; one that failed is one too, and is tried again an hour later. The pause is the host's running state, in its cache: this file is not changed. `plan`, `doctor` and the panel say which is paused, since when and why.

## The agent setup

The operator's global instructions, skills and guards, declared once in the operator's own files and named by three host settings, `run --agent-instructions <FILE>`, `run --agent-skills <FILE>` and `run --agent-guards <FILE>` (`afkconvoy run --help`). The skill list holds one line per source, `<source> <skill>...`, or `<source> *` for every skill the source ships; blank and `#` lines say nothing. Writing the host kit gives every harness it holds the setup: the instructions file is linked at the harness's global instructions location (Claude Code `~/.claude/CLAUDE.md`, Codex `~/.codex/AGENTS.md`, pi `~/.pi/agent/AGENTS.md`, Kimi Code `~/.kimi-code/AGENTS.md`, Grok `~/.grok/rules/AGENTS.md`, DSH `~/.dsh/AGENTS.md`), with what stood there and was no link to it kept beside it as `<name>.before-agent-setup`; the list is installed through `npx skills add <source> -g -y -a <agent>...` for each harness's `npx skills` agent (DSH has none, and reads the shared copies), and a skill installed for one of those agents that the list does not name is removed. The copies, in the shared `~/.agents/skills`, are read-only between syncs: no harness and no shell command edits one in place; a sync opens them for its own writes and closes them again. The guard list holds one line per guard, `shell <command>` or `edits <command>`; blank and `#` lines say nothing. Each guard is wired before the tool calls it applies to into the hook settings of every held harness that has some: Claude Code `~/.claude/settings.json` (matchers `Bash`, and `Edit|Write|MultiEdit|NotebookEdit`), Codex `~/.codex/hooks.json` (`Bash`, and `apply_patch`); pi, Kimi Code, Grok and DSH are given none. Wiring merges and owns nothing else: an entry of the kit's is one whose command opens with `: afkconvoy-agent-guard;`; those are replaced by the declared ones, after every other entry at `PreToolUse`, and every other key and entry is written back as it was; a file already as declared is not written. Codex asks its user to review a new hook once (`/hooks`). A Codex session outside a role, started with a user directory the host keeps rather than `~/.codex`, is given the guards in that directory's `hooks.json`, which holds them alone and nothing else of `~/.codex/hooks.json`; it starts with `--dangerously-bypass-hook-trust`, so nothing is asked. A worker in a role is given none of them. A guard reads its harness's call as JSON on standard input: a shell call is `tool_name` `Bash` with the line in `tool_input.command` on both harnesses, with `cwd`; an edit is Claude Code's `Edit`, `Write`, `MultiEdit` or `NotebookEdit` with `tool_input.file_path` (or `notebook_path`), and Codex's `apply_patch` with the patch in `tool_input.command`, whose `*** Add File: `, `*** Update File: ` and `*** Delete File: ` lines name the files; `AFKCONVOY_HARNESS` names the harness. It refuses by exiting 2 with why on standard error, or by printing `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"<why>"}}`: both harnesses take either. A tick gives the setup to a harness held since it was last given — found by a refresh, a switch or a launch — with no step by the operator; one whose giving failed is tried again an hour later. A `run` that changes either setting gives it to every held harness again. What was given is recorded beside the host kit, in `agent-setup.json`. `doctor` names each held harness that differs from the declaration (`differs from the agent setup`), with `afkconvoy kit` as its fix. With none of the settings said, nothing is given and nothing is reported."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn former_consult_choices_are_accepted_and_dropped_when_a_kit_is_written() {
        for key in ["stock_consult", "stock_inspector"] {
            let mut legacy = json!({"harnesses": []});
            legacy[key] = json!({"harness": "pi", "model": "some/model"});
            let kit: Kit = serde_json::from_value(legacy).unwrap();
            assert_eq!(kit.first(), None);
            let written = serde_json::to_value(kit).unwrap();
            assert!(written.get("stock_consult").is_none());
            assert!(written.get("stock_inspector").is_none());
        }
    }

    #[test]
    fn a_former_cast_key_is_accepted_and_dropped_when_a_kit_is_written() {
        let legacy = json!({"cast": true, "harnesses": []});
        let kit: Kit = serde_json::from_value(legacy).unwrap();
        assert!(kit.cast);
        assert!(serde_json::to_value(kit).unwrap().get("cast").is_none());
    }

    #[test]
    fn a_written_kit_drops_a_cast_key_and_keeps_unresearched_model_priority() {
        let current = Kit {
            cast: true,
            harnesses: vec![held(Harness::ClaudeCode, "claude", &["sonnet", "default"])],
            removed: vec![],
            _retired_consult: None,
        };
        for writing in [Writing::Whole, Writing::Refresh] {
            let (kit, _) = composed_as(&current, writing);
            assert_eq!(
                kit.harnesses[0]
                    .models
                    .iter()
                    .map(|model| model.model.as_str())
                    .collect::<Vec<_>>(),
                ["sonnet", "default", "opus"]
            );
        }
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("kit.json");
        store(&file, &current, 100).unwrap();
        assert!(!std::fs::read_to_string(&file).unwrap().contains("\"cast\""));
        let stale = Kit {
            cast: false,
            harnesses: vec![held(Harness::ClaudeCode, "claude", &["new", "default", "sonnet"])],
            removed: vec![],
            _retired_consult: None,
        };
        store(&file, &stale, 200).unwrap();
        let latest = load_at(&file).unwrap();
        assert!(!latest.cast);
        assert_eq!(
            latest.harnesses[0].models.iter().map(|model| model.model.as_str()).collect::<Vec<_>>(),
            ["sonnet", "default", "new"]
        );
        assert!(!serde_json::from_str::<Kit>(r#"{"harnesses":[]}"#).unwrap().cast);
    }

    #[test]
    fn concurrent_command_discovery_and_kit_edits_preserve_the_latest_kit() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("kit.json");
        let original = Kit {
            cast: false,
            harnesses: vec![held(Harness::Pi, "pi --custom", &["provider/model"])],
            removed: vec![],
            _retired_consult: None,
        };
        store(&file, &original, 100).unwrap();
        std::thread::scope(|scope| {
            for harness in [Harness::Codex, Harness::ClaudeCode, Harness::Codex, Harness::Dsh] {
                let file = &file;
                scope.spawn(move || discover_at(file, harness, harness.stock()).unwrap());
            }
            let file = &file;
            scope.spawn(move || {
                let _writing = writing(file).unwrap();
                let mut kit = load_at(file).unwrap();
                kit.harnesses[0].models[0].good_at = Some("concurrent research".into());
                store(file, &kit, 101).unwrap();
            });
        });
        let kit = load_at(&file).unwrap();
        assert_eq!(kit.harnesses.len(), 4);
        assert_eq!(kit.harnesses[0].harness, Harness::Pi);
        assert_eq!(kit.harnesses[0].command, "pi --custom");
        assert_eq!(kit.harnesses[0].models[0].model, "provider/model");
        assert_eq!(kit.harnesses[0].models[0].good_at.as_deref(), Some("concurrent research"));
        for held in &kit.harnesses[1..] {
            assert!(held.models.is_empty());
        }
        assert_eq!(discover_at(&file, Harness::Pi, Harness::Pi.stock()).unwrap(), "pi --custom");
        assert_eq!(load_at(&file).unwrap(), kit);
    }

    #[test]
    fn dsh_server_models_are_listed_and_held_without_the_credentials() {
        let node = std::process::Command::new("node").arg("--version").output();
        if !node.is_ok_and(|out| out.status.success()) {
            eprintln!("skipped: no `node` on PATH to run the DSH plugin with");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("models.mjs"),
            include_str!("../../core/src/harness/dsh/models.mjs"),
        )
        .unwrap();
        std::fs::write(
            root.path().join("test.mjs"),
            include_str!("../../core/src/harness/dsh/models-test.mjs"),
        )
        .unwrap();
        std::fs::write(
            root.path().join("probe.mjs"),
            include_str!("../../core/src/harness/dsh/probe.mjs"),
        )
        .unwrap();
        for mode in ["list", "key", "refused", "truncated", "none", "malformed", "redirect"] {
            let run = crate::process::output(
                std::process::Command::new("node").arg(root.path().join("test.mjs")).arg(mode),
            )
            .unwrap();
            assert!(run.status.success(), "{mode}: {}", String::from_utf8_lossy(&run.stderr));
            let printed = String::from_utf8(run.stdout).unwrap();
            let found = |harness| {
                if harness == Harness::Dsh { Ok(()) } else { Err("not installed".into()) }
            };
            let answer = printed.clone();
            let listing = move |_: Harness, _: &str| {
                let ids = Harness::Dsh
                    .models(&answer)
                    .ok_or_else(|| Harness::Dsh.listing_failure(&answer).unwrap())?;
                let shown = Harness::Dsh.shown(&answer);
                Ok(ids
                    .into_iter()
                    .map(|id| {
                        let name = shown
                            .iter()
                            .find(|(model, _)| model == &id)
                            .map(|(_, name)| name.clone());
                        Listed { name: id, shown: name }
                    })
                    .collect())
            };
            let host = Host { found: &found, listed: &listing };
            let (kit, left_out) = compose(&Kit::default(), &host, Writing::Whole);
            if matches!(mode, "list" | "key") {
                let models = &kit.harnesses[0].models;
                assert_eq!(models.len(), 2);
                assert_eq!(models[0].model, "deepseek-flash");
                assert_eq!(models[0].names, ["DeepSeek V4.1 Flash"]);
            } else {
                assert!(kit.harnesses.is_empty());
                let why = &left_out.iter().find(|left| left.harness == Harness::Dsh).unwrap().why;
                let expected = match mode {
                    "refused" | "truncated" => "Authentication Fails",
                    "none" => "not signed in",
                    "malformed" => "server answered",
                    _ => "could not read DSH credentials or ask the model server",
                };
                assert!(why.contains(expected), "{mode}: {why}");
            }
            let recorded = format!(
                "{printed}{}{}{}",
                String::from_utf8_lossy(&run.stderr),
                serde_json::to_string(&kit).unwrap(),
                serde_json::to_string(&left_out).unwrap()
            );
            for secret in ["account-secret-sentinel-396", "api-key-secret-sentinel-396"] {
                assert!(!recorded.contains(secret), "credential leaked in {mode}");
            }
        }
    }

    fn listed(name: &str, shown: Option<&str>) -> Listed {
        Listed { name: name.into(), shown: shown.map(str::to_string) }
    }

    /// `seen` as research places it, with a dated source.
    fn sourced(seen: Seen) -> Seen {
        let url = format!("https://vendor.example/{}", seen.model);
        Seen {
            good_at: Some("hard work".into()),
            sources: vec![crate::kit_research::Source { url, date: "2026-10-24".into() }],
            researched_at: Some(1),
            ..seen
        }
    }

    fn held(harness: Harness, command: &str, models: &[&str]) -> Held {
        let models = models
            .iter()
            .map(|model| Seen {
                model: model.to_string(),
                names: vec![],
                probed: None,
                good_at: None,
                settled: None,
                sources: vec![],
                researched_at: None,
            })
            .collect();
        Held { harness, command: command.into(), models }
    }

    /// A host with Claude Code, Codex, pi and Kimi Code found, DSH absent;
    /// Claude Code lists three ids and Codex lists three, one duplicated.
    fn composed(current: &Kit) -> (Kit, Vec<LeftOut>) {
        composed_as(current, Writing::Whole)
    }

    fn composed_as(current: &Kit, writing: Writing) -> (Kit, Vec<LeftOut>) {
        let found = |harness: Harness| match harness {
            Harness::Dsh => Err("`type -- 'dsh-tui'` found nothing".to_string()),
            _ => Ok(()),
        };
        let listed = |harness: Harness, command: &str| match harness {
            Harness::ClaudeCode => Ok(vec![
                listed("default", Some("Default")),
                listed("opus", Some("Opus 5.5")),
                listed("sonnet", Some("Sonnet 5.5")),
            ]),
            Harness::Codex => Ok(["gpt-a", "bad", "gpt-b", "gpt-a"]
                .into_iter()
                .map(|name| listed(name, None))
                .collect()),
            Harness::Kimi => Err(format!("it listed none: {command} provider list --json")),
            Harness::Grok => Err(format!("it listed none: {command} models")),
            _ => Err(format!("it listed none: {command} --list-models")),
        };
        let host = Host { found: &found, listed: &listed };
        compose(current, &host, writing)
    }

    fn summary(kit: &Kit) -> Vec<(Harness, &str, Vec<&str>)> {
        kit.harnesses
            .iter()
            .map(|h| {
                (h.harness, h.command.as_str(), h.models.iter().map(|m| m.model.as_str()).collect())
            })
            .collect()
    }

    #[test]
    fn with_no_host_kit_each_harness_found_is_held_with_every_listed_id() {
        let (kit, left_out) = composed(&Kit::default());
        assert_eq!(
            summary(&kit),
            [
                (
                    Harness::ClaudeCode,
                    Harness::ClaudeCode.stock(),
                    vec!["default", "opus", "sonnet"]
                ),
                (Harness::Codex, Harness::Codex.stock(), vec!["gpt-a", "bad", "gpt-b"]),
            ]
        );
        let names = |held: &Held| -> Vec<Vec<String>> {
            held.models.iter().map(|seen| seen.names.clone()).collect()
        };
        assert_eq!(
            names(&kit.harnesses[0]),
            [vec!["Default"], vec!["Opus 5.5"], vec!["Sonnet 5.5"]]
        );
        assert_eq!(names(&kit.harnesses[1]), [vec![], vec![], vec![]] as [Vec<String>; 3]);
        // Every listed id waits for research, and no worker starts on it:
        // each harness runs its default model meanwhile.
        for seen in kit.harnesses.iter().flat_map(|held| &held.models) {
            assert_eq!(seen.good_at.as_deref(), Some(crate::kit_research::PENDING));
        }
        assert!(kit.range().iter().all(|held| held.models.is_empty()));
        assert_eq!(kit.first(), Some(Named { harness: Harness::ClaudeCode, model: None }));
        assert!(!serde_json::to_string(&kit).unwrap().contains("probed"));
        let said = report("/kit.json", &kit, &left_out);
        assert!(
            said.contains("its default model; research pending: default opus sonnet"),
            "{said}"
        );
        let said: Vec<(Harness, Option<&str>, &str)> = left_out
            .iter()
            .map(|out| (out.harness, out.model.as_deref(), out.why.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (Harness::Pi, None, "listing failed: it listed none: pi --list-models"),
                (Harness::Dsh, None, "not installed: `type -- 'dsh-tui'` found nothing"),
                (
                    Harness::Kimi,
                    None,
                    "listing failed: it listed none: kimi --auto provider list --json"
                ),
                (
                    Harness::Grok,
                    None,
                    "listing failed: it listed none: grok --always-approve models"
                ),
            ]
        );
    }

    #[test]
    fn a_host_kit_written_again_keeps_its_order_commands_and_good_at() {
        let mut claude = held(Harness::ClaudeCode, "old", &["bad", "default", "gone"]);
        claude.models[1].names = vec!["claude-opus-5-5".into()];
        claude.models[1] = sourced(claude.models[1].clone());
        let current = Kit {
            cast: false,
            harnesses: vec![held(Harness::Codex, "mycodex --yolo", &["gpt-a"]), claude],
            removed: vec![],
            _retired_consult: None,
        };
        let (kit, left_out) = composed(&current);
        assert_eq!(
            summary(&kit),
            [
                (Harness::Codex, "mycodex --yolo", vec!["gpt-a", "bad", "gpt-b"]),
                (Harness::ClaudeCode, "old", vec!["default", "opus", "sonnet"]),
            ]
        );
        assert_eq!(kit.harnesses[1].models[0].good_at.as_deref(), Some("hard work"));
        assert_eq!(kit.harnesses[1].models[0].sources, current.harnesses[1].models[1].sources);
        assert_eq!(kit.harnesses[1].models[0].researched_at, Some(1));
        assert_eq!(kit.harnesses[1].models[0].names, ["claude-opus-5-5", "Default"]);
        let stock = Named { harness: Harness::Codex, model: Some("gpt-a".into()) };
        assert_eq!(kit.first(), Some(stock));
        for model in ["gone", "bad"] {
            assert!(
                left_out
                    .iter()
                    .any(|out| out.model.as_deref() == Some(model) && out.why == "no longer listed")
            );
        }
        let (again, _) = composed(&kit);
        assert_eq!(again, kit);
    }

    #[test]
    fn a_harness_no_longer_found_is_left_out() {
        let current = Kit {
            cast: false,
            harnesses: vec![held(Harness::Pi, "pi", &["a/b"])],
            removed: vec![],
            _retired_consult: None,
        };
        let host = Host {
            found: &|_| Ok(()),
            listed: &|harness, _| match harness {
                Harness::Codex => Ok(vec![listed("listed", None)]),
                _ => Err("it failed".into()),
            },
        };
        let (kit, left_out) = compose(&current, &host, Writing::Whole);
        assert_eq!(summary(&kit), [(Harness::Codex, Harness::Codex.stock(), vec!["listed"])]);
        assert!(left_out.iter().any(|out| out.harness == Harness::Pi));
        let said = report("/kit.json", &kit, &left_out);
        assert!(said.contains("its default model; research pending: listed"), "{said}");
        assert!(!said.contains("probe"), "{said}");
    }

    #[test]
    fn a_refresh_adds_new_listings_drops_missing_ids_and_keeps_failed_listings() {
        let mut claude = held(Harness::ClaudeCode, "claude", &["default", "gone"]);
        claude.models[0] = sourced(claude.models[0].clone());
        let current = Kit {
            cast: false,
            harnesses: vec![
                claude,
                held(Harness::Codex, "codex", &["gpt-a"]),
                held(Harness::Pi, "pi", &["a/b"]),
                held(Harness::Dsh, "dsh", &["d"]),
            ],
            removed: vec![],
            _retired_consult: None,
        };
        let (kit, left_out) = composed_as(&current, Writing::Refresh);
        assert_eq!(
            summary(&kit),
            [
                (Harness::ClaudeCode, "claude", vec!["default", "opus", "sonnet"]),
                (Harness::Codex, "codex", vec!["gpt-a", "bad", "gpt-b"]),
                (Harness::Pi, "pi", vec!["a/b"]),
                (Harness::Dsh, "dsh", vec!["d"]),
            ]
        );
        assert_eq!(kit.harnesses[2], current.harnesses[2]);
        assert_eq!(kit.harnesses[3], current.harnesses[3]);
        assert_eq!(kit.harnesses[0].models[0].good_at, current.harnesses[0].models[0].good_at);
        assert!(
            left_out
                .iter()
                .any(|out| out.model.as_deref() == Some("gone") && out.why == "no longer listed")
        );
        let changed = Changed::between(&current, &kit);
        assert_eq!(
            changed,
            Changed {
                added: vec![
                    "claude-code opus".into(),
                    "claude-code sonnet".into(),
                    "codex bad".into(),
                    "codex gpt-b".into(),
                ],
                dropped: vec!["claude-code gone".into()],
            }
        );
        let (again, _) = composed_as(&kit, Writing::Refresh);
        assert_eq!(again, kit);
        assert!(Changed::between(&kit, &again).is_empty());
    }

    #[test]
    fn a_harness_held_with_no_model_is_written_again_with_its_listed_ids() {
        let current = Kit {
            cast: false,
            harnesses: vec![
                held(Harness::ClaudeCode, "claude", &[]),
                held(Harness::Codex, "codex", &[]),
                held(Harness::Pi, "pi", &[]),
            ],
            removed: vec![],
            _retired_consult: None,
        };
        assert_eq!(current.first(), Some(Named { harness: Harness::ClaudeCode, model: None }));
        for writing in [Writing::Whole, Writing::Refresh] {
            let (kit, left_out) = composed_as(&current, writing);
            let mut expected = vec![
                (Harness::ClaudeCode, "claude", vec!["default", "opus", "sonnet"]),
                (Harness::Codex, "codex", vec!["gpt-a", "bad", "gpt-b"]),
            ];
            if writing == Writing::Refresh {
                expected.push((Harness::Pi, "pi", vec![]));
            } else {
                assert!(
                    left_out.iter().any(|out| out.harness == Harness::Pi && out.model.is_none())
                );
            }
            assert_eq!(summary(&kit), expected, "{writing:?}");
            // Listed, its models wait for research: it runs its default.
            let stock = Named { harness: Harness::ClaudeCode, model: None };
            assert_eq!(kit.first(), Some(stock), "{writing:?}");
        }
    }

    #[test]
    fn a_refresh_keeps_each_listed_id_distinct_and_adds_its_display_name() {
        let current = Kit {
            cast: false,
            harnesses: vec![held(Harness::ClaudeCode, "claude", &["old-id"])],
            removed: vec![],
            _retired_consult: None,
        };
        let found =
            |harness| if harness == Harness::ClaudeCode { Ok(()) } else { Err("absent".into()) };
        let listed =
            |_: Harness, _: &str| Ok(vec![listed("opus", Some("Opus")), listed("old-id", None)]);
        let host = Host { found: &found, listed: &listed };
        let (mut kit, _) = compose(&current, &host, Writing::Refresh);
        assert_eq!(kit.harnesses[0].models.len(), 2);
        assert_eq!(kit.range()[0].model("old-id").unwrap().id, "old-id");
        // A newly listed id matches nothing until research places it.
        assert!(kit.range()[0].model("Opus").is_err());
        let opus = kit.harnesses[0].models.iter_mut().find(|seen| seen.model == "opus").unwrap();
        assert_eq!(opus.names, ["Opus"]);
        *opus = sourced(opus.clone());
        assert_eq!(kit.range()[0].model("opus").unwrap().id, "opus");
        assert_eq!(kit.range()[0].model("Opus").unwrap().id, "opus");
    }

    #[test]
    fn a_host_kit_is_due_a_day_after_it_was_written_and_an_hour_after_a_refresh_was_tried() {
        let day = REFRESHED_EVERY;
        assert!(!due(None, None, 10 * day), "no host kit: `run` writes it");
        assert!(!due(Some(day), None, 2 * day - 1));
        assert!(due(Some(day), None, 2 * day));
        assert!(!due(Some(day), Some(2 * day), 2 * day + 3599), "tried within the hour");
        assert!(due(Some(day), Some(2 * day), 2 * day + 3600));
        assert!(!due(Some(3 * day), None, day), "written after now");
    }

    #[test]
    fn a_command_only_codex_entry_is_listed_without_waiting_for_the_daily_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("kit.json");
        let current = Kit {
            cast: true,
            harnesses: vec![held(Harness::Codex, "codex", &[])],
            removed: vec![],
            _retired_consult: None,
        };
        store(&file, &current, 1000).unwrap();
        let found = |harness| if harness == Harness::Codex { Ok(()) } else { Err("absent".into()) };
        let listed = |_: Harness, _: &str| Ok(vec![listed("gpt-first", None)]);
        let host = || Ok(Host { found: &found, listed: &listed });
        let (_, after) = refresh_at(&file, 1001, host).unwrap().unwrap();
        assert!(!after.cast, "a write drops the cast key");
        assert_eq!(after.harnesses[0].models[0].model, "gpt-first");
        assert!(
            refresh_at(&file, 1002, || panic!("the repaired kit is current")).unwrap().is_none()
        );
    }

    #[test]
    fn an_unsuccessful_codex_repair_preserves_the_kit_and_waits_an_hour() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("kit.json");
        let current = Kit {
            cast: true,
            harnesses: vec![held(Harness::Codex, "codex", &[]), held(Harness::Dsh, "dsh-tui", &[])],
            removed: vec![],
            _retired_consult: None,
        };
        store(&file, &current, 1000).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        let host = || {
            Ok(Host {
                found: &|_| Err("not answering".into()),
                listed: &|_, _| panic!("nothing was found"),
            })
        };
        let (before, after) = refresh_at(&file, 1001, host).unwrap().unwrap();
        assert_eq!(before, after);
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
        assert!(refresh_at(&file, 1002, || panic!("backoff")).unwrap().is_none());
    }

    #[test]
    fn one_process_refreshes_the_host_kit_and_the_others_read_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("afkconvoy/kit.json");
        let mut pi = held(Harness::Pi, "pi", &["a/b"]);
        pi.models[0] = sourced(pi.models[0].clone());
        let current =
            Kit { cast: false, harnesses: vec![pi], removed: vec![], _retired_consult: None };
        store(&file, &current, 1000).unwrap();
        assert_eq!(written(&file), Some(1000));
        let listed = |_: Harness, _: &str| Ok(vec![listed("a/b", None), listed("c/d", None)]);
        let found = |harness: Harness| match harness {
            Harness::Pi => Ok(()),
            _ => Err("not here".to_string()),
        };
        let host = || Host { found: &found, listed: &listed };
        assert!(refresh_at(&file, 1000 + REFRESHED_EVERY - 1, || Ok(host())).unwrap().is_none());
        let now = 1000 + REFRESHED_EVERY;
        // While one process writes it again, another finds it locked and
        // reads the file as it is.
        let (writing, written_again) = (std::sync::Barrier::new(2), std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                refresh_at(&file, now, || {
                    writing.wait();
                    written_again.wait();
                    Ok(host())
                })
            });
            writing.wait();
            let other = refresh_at(&file, now, || panic!("it is not this process's to write"));
            assert!(other.unwrap().is_none());
            written_again.wait();
            let (before, after) = first.join().unwrap().unwrap().unwrap();
            assert_eq!(before, current);
            assert_eq!(summary(&after), [(Harness::Pi, "pi", vec!["a/b", "c/d"])]);
            // c/d waits for the watch, across every later listing.
            assert_eq!(after.first().unwrap().model.as_deref(), Some("a/b"));
            assert!(after.harnesses[0].models[1].awaits_research());
        });
        // Once written, it is not due again for a day, in any process.
        assert_eq!(written(&file), Some(now));
        assert!(refresh_at(&file, now + 60, || panic!("refreshed already")).unwrap().is_none());
        // A refresh that fails is tried again an hour later, not every tick.
        let later = now + REFRESHED_EVERY;
        assert!(refresh_at(&file, later, || bail!("orca is not running")).is_err());
        assert!(refresh_at(&file, later + 60, || panic!("tried a minute ago")).unwrap().is_none());
        let (_, again) = refresh_at(&file, later + 3600, || Ok(host())).unwrap().unwrap();
        assert_eq!(again.range()[0].models.len(), 1, "c/d awaits the watch");
        let last = later + 3600 + REFRESHED_EVERY;
        assert!(refresh_at(&file, last, || Ok(host())).unwrap().is_some());
    }

    #[test]
    fn a_kit_reads_as_it_is_written_and_forgets_legacy_probe_evidence() {
        let kit = Kit {
            cast: false,
            harnesses: vec![held(Harness::ClaudeCode, "claude", &["opus"])],
            removed: vec![],
            _retired_consult: None,
        };
        let text = serde_json::to_string(&kit).unwrap();
        assert_eq!(serde_json::from_str::<Kit>(&text).unwrap(), kit);
        let legacy = r#"{"harnesses":[{"harness":"claude-code","command":"claude","models":[{"model":"opus","probed":{"at":1,"seconds":2.0}}]}]}"#;
        let older: Kit = serde_json::from_str(legacy).unwrap();
        assert_eq!(older.harnesses[0].models[0].model, "opus");
        assert!(!serde_json::to_string(&older).unwrap().contains("probed"));
        assert!(
            serde_json::from_str::<Kit>(r#"{"harnesses":[{"harness":"x","command":"x"}]}"#)
                .unwrap_err()
                .to_string()
                .contains("is no harness")
        );
    }

    #[test]
    fn relisting_keeps_an_operator_removed_harness_out_even_when_it_is_installed() {
        let current = Kit {
            cast: false,
            harnesses: vec![held(Harness::ClaudeCode, "claude", &[])],
            removed: vec![Harness::Pi],
            _retired_consult: None,
        };
        for writing in [Writing::Whole, Writing::Refresh] {
            let (kit, left_out) = composed_as(&current, writing);
            assert!(!kit.harnesses.iter().any(|held| held.harness == Harness::Pi));
            assert_eq!(kit.removed, [Harness::Pi]);
            let pi: Vec<_> = left_out.iter().filter(|out| out.harness == Harness::Pi).collect();
            assert_eq!(pi.len(), 1);
            assert_eq!(pi[0].why, REMOVED);
        }
    }

    #[test]
    fn worker_launch_discovery_does_not_restore_an_operator_removed_harness() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("kit.json");
        let current = Kit {
            cast: false,
            harnesses: vec![held(Harness::ClaudeCode, "claude", &[])],
            removed: vec![Harness::Pi],
            _retired_consult: None,
        };
        store(&file, &current, 100).unwrap();
        let before = std::fs::read(&file).unwrap();
        let error = discover_at(&file, Harness::Pi, Harness::Pi.stock()).unwrap_err();
        assert!(error.to_string().contains("switch --harness pi"), "{error:#}");
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }
}

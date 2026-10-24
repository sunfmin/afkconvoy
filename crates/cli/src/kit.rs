//! The host kit (docs/adr/0020): the harnesses this host can start, in order
//! of preference — each one's worker launch command and the models it was
//! seen to answer with — and the stock consult's harness and model. This file
//! belongs to the host user, outside every checkout, and is written with no
//! human: `afkconvoy kit`, or `run` on a host without one.
use std::collections::BTreeSet;
use std::fs::{OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use afkconvoy_core::harness::Harness;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Kit {
    /// The harnesses this host can start, in order of preference: the first
    /// is the host kit's first.
    pub harnesses: Vec<Held>,
    /// The stock consult's harness and model, where the human named them;
    /// with none named it runs the host kit's first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock_consult: Option<Named>,
}

/// One harness the host kit holds.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Held {
    #[serde(deserialize_with = "harness")]
    pub harness: Harness,
    /// Its worker launch command.
    pub command: String,
    /// The models it was seen to answer with, in order of preference.
    #[serde(default)]
    pub models: Vec<Seen>,
}

/// A model one harness was seen to answer with.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Seen {
    /// Its id: the model its probe answered as, by its harness's own name
    /// for it, which a worker is started on.
    pub model: String,
    /// The other names it goes by on its harness, in the order listed: each
    /// name its harness lists that answered as it, and the name its harness
    /// shows it by. Any of them matches it (`casting::Held::model`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// Its last probe, which it answered. `None` of a model a host kit
    /// written before every harness was probed holds as the human named it.
    #[serde(default)]
    pub probed: Option<Probed>,
    /// What the sourced research says it is good at, or `not researched`:
    /// what a caster is told of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub good_at: Option<String>,
    /// The dated URLs supporting good_at, absent before sourced research.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<crate::kit_research::Source>,
    /// When the research succeeded, in epoch seconds; never refreshed by age.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub researched_at: Option<u64>,
}

impl Seen {
    /// It as a range holds it: its id and its other names.
    pub fn as_model(&self) -> afkconvoy_core::casting::Model {
        afkconvoy_core::casting::Model { id: self.model.clone(), names: self.names.clone() }
    }
}

/// A probe a model answered.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Probed {
    /// When, in epoch seconds.
    pub at: u64,
    /// How long it took to answer.
    pub seconds: f64,
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

fn harness<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Harness, D::Error> {
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

    /// The host kit's first: its first harness, with that harness's first
    /// model. `None` of a host kit that holds no harness.
    pub fn first(&self) -> Option<Named> {
        let held = self.harnesses.first()?;
        Some(Named { harness: held.harness, model: held.models.first().map(|m| m.model.clone()) })
    }

    /// The host kit as a range (docs/adr/0020): each harness in order, with
    /// its models in order.
    pub fn range(&self) -> Vec<afkconvoy_core::casting::Held> {
        let held = |held: &Held| afkconvoy_core::casting::Held {
            harness: held.harness,
            models: held.models.iter().map(Seen::as_model).collect(),
        };
        self.harnesses.iter().map(held).collect()
    }

    /// The host kit as this host can start it now: without the harnesses
    /// `paused` on it (docs/adr/0020), and with no stock consult named on
    /// one of them — it then runs the first of the rest.
    pub fn without(&self, paused: &[afkconvoy_core::casting::Paused]) -> Kit {
        let is_paused = |harness: Harness| paused.iter().any(|pause| pause.harness == harness);
        Kit {
            harnesses: self
                .harnesses
                .iter()
                .filter(|held| !is_paused(held.harness))
                .cloned()
                .collect(),
            stock_consult: self.stock_consult.clone().filter(|named| !is_paused(named.harness)),
        }
    }

    /// What the stock consult runs: the harness and model named for it — the
    /// id of the model that name matches (`casting::Held::model`) — or the
    /// host kit's first.
    pub fn stock_consult(&self) -> Option<Named> {
        let Some(named) = self.stock_consult.clone().filter(|_| self.stock_unmatched().is_none())
        else {
            return self.first();
        };
        let model = named.model.as_deref().map(|model| match self.matched(named.harness, model) {
            Ok(id) => id,
            Err(_) => model.to_string(),
        });
        Some(Named { harness: named.harness, model })
    }

    /// The id of the model of `harness` that `name` names here
    /// (`casting::Held::model`); why it names none.
    fn matched(&self, harness: Harness, name: &str) -> Result<String, String> {
        let held = self.range().into_iter().find(|held| held.harness == harness);
        let held = held.ok_or_else(|| format!("it holds no `{}`", harness.key()))?;
        held.model(name).map(|model| model.id.clone()).map_err(|why| why.said(name, harness))
    }

    pub fn stock_unmatched(&self) -> Option<String> {
        self.stock_fault()
    }

    /// Why this is no host kit, when it is not.
    fn fault(&self) -> Option<String> {
        let mut seen = BTreeSet::new();
        for held in &self.harnesses {
            if !seen.insert(held.harness.key()) {
                return Some(format!("it holds the harness `{}` twice", held.harness.key()));
            }
        }
        None
    }

    /// Why what this host kit names for the stock consult is not held by it,
    /// when it is not: what `afkconvoy kit` refuses to write.
    fn stock_fault(&self) -> Option<String> {
        let named = self.stock_consult.as_ref()?;
        if self.held(named.harness).is_none() {
            return Some(format!(
                "its stock consult runs the harness `{}`, which it does not hold",
                named.harness.key()
            ));
        }
        let model = named.model.as_deref()?;
        let why = self.matched(named.harness, model).err()?;
        Some(format!("its stock consult runs the model `{model}`: {why}"))
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
    Ok((load()?.without(&paused), paused))
}

/// The harness and model the stock consult runs, as this host's host kit
/// says (`Kit::stock_consult`).
pub fn stock_consult() -> Result<Named> {
    load()?.stock_consult().context("the host kit holds no harness: run `afkconvoy kit`")
}

/// The host kit as `doctor` and `plan` show it: where it is kept, each
/// harness in order with its command and its models in order, each with when
/// it was last probed, what the stock consult runs, and each harness paused
/// on the host, since when and why — which is left out of it until a probe
/// finds it working.
pub fn show(now: u64) -> Value {
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
            let running = kit.without(&paused);
            json!({
                "path": file,
                "refreshed": refreshed(),
                "harnesses": kit.harnesses,
                "stock_consult": running.stock_consult().map(|named| json!({
                    "harness": named.harness,
                    "model": named.model,
                    "named": running.stock_consult.is_some(),
                    "unmatched": running.stock_unmatched(),
                })),
                "paused": paused_said,
            })
        }
        Err(error) => json!({"path": file, "error": format!("{error:#}"), "paused": paused_said}),
    }
}

/// Writes the host kit for `run` where this host has none (`write`), saying
/// so on stderr; then reads it.
pub fn ensure(now: u64) -> Result<()> {
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
    let current = if file.exists() { load()? } else { Kit::default() };
    let (mut kit, left_out) = compose(&current, &host, Writing::Whole);
    crate::kit_research::research(&current, &mut kit, now, host.research);
    store(&file, &kit, now)?;
    resume_answered(&kit, now)?;
    eprint!("{}", report(&file.display().to_string(), &kit, &left_out));
    Ok(json!({"path": file, "written": true, "kit": kit, "left_out": left_out}))
}

/// Writes `kit` to `file` at once, as written `now`: the time its file
/// keeps as when it was last modified, which says when the host kit was last
/// written (`written`).
fn store(file: &Path, kit: &Kit, now: u64) -> Result<()> {
    let parent = file.parent().context("the host kit needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(kit)?)?;
    stamp(&temporary, now)?;
    std::fs::rename(&temporary, file)?;
    Ok(())
}

/// Has each harness paused on the host that `kit` holds a model of that
/// answered its probe `now` back.
fn resume_answered(kit: &Kit, now: u64) -> Result<()> {
    let pauses = crate::paused::Pauses::here()?;
    for held in &kit.harnesses {
        if held.models.iter().any(|seen| seen.probed.is_some_and(|probed| probed.at == now)) {
            pauses.resume(held.harness)?;
        }
    }
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
    if !due(written(file), tried(), now) {
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
    if !due(written(file), tried(), now) {
        return Ok(None);
    }
    std::fs::write(&lock, now.to_string())
        .with_context(|| format!("cannot write {}", lock.display()))?;
    let current = load_at(file)?;
    let host = host()?;
    let (mut kit, _) = compose(&current, &host, Writing::Refresh);
    crate::kit_research::research(&current, &mut kit, now, host.research);
    store(file, &kit, now)?;
    drop(held);
    Ok(Some((current, kit)))
}

/// Writes this host's host kit again once a day (`refresh_at`), as a tick of
/// `instance` on `host` in `repo` does before anything else, and appends a
/// line to the run log of `repo` where that changed it, naming what was added
/// and dropped, or where it failed. Its probes spend a little of each login,
/// and only of a model it did not hold.
pub fn refresh(repo: &crate::git::Repo, instance: &str, host: &str, now: u64) -> Result<()> {
    let refreshed = refresh_at(&path()?, now, || Host::here(now)).and_then(|refreshed| {
        let Some((before, after)) = refreshed else { return Ok(None) };
        resume_answered(&after, now)?;
        Ok(Some(Changed::between(&before, &after)))
    });
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

/// How many probes run at once.
const PROBES_AT_ONCE: usize = 8;

/// Sends every one of `asked` its one-line prompt, a few at a time.
fn probe_all(asked: &[Asked]) -> Vec<Answer> {
    let mut answers = vec![];
    for batch in asked.chunks(PROBES_AT_ONCE) {
        std::thread::scope(|scope| {
            let running: Vec<_> = batch
                .iter()
                .map(|(harness, command, model)| {
                    scope.spawn(move || crate::launch::probe(*harness, command, Some(model)))
                })
                .collect();
            answers.extend(running.into_iter().map(|run| {
                run.join().unwrap_or_else(|_| Answer::Unasked("its probe panicked".to_string()))
            }));
        });
    }
    answers
}

/// A model to probe: its harness, that harness's worker launch command, and
/// the name it is listed by.
type Asked = (Harness, String, String);

/// What a model said to its probe.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// It answered, in this many seconds, as the model of this id, where its
    /// answer says which (`Harness::answered_as`).
    Answered(f64, Option<String>),
    /// It said nothing within the time limit, or failed.
    Silent,
    /// It was never asked, for this reason: no terminal could be made for its
    /// probe. It counts as silent.
    Unasked(String),
}

/// One model a harness lists: the name it lists it by, and the name it
/// shows it by, where it shows one (`Harness::shown`).
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub name: String,
    pub shown: Option<String>,
}

/// The models a harness offers through a command; why it offers none.
type Listing = dyn Fn(Harness, &str) -> Result<Vec<Listed>, String> + Sync;

/// What the host kit is written from: this host, as an orca terminal finds it.
pub struct Host<'a> {
    /// Whether a harness's program is found; why it is taken as not.
    found: &'a (dyn Fn(Harness) -> Result<(), String> + Sync),
    /// The models a harness offers through a command, in its order; why it
    /// offers none.
    listed: &'a Listing,
    probe: &'a dyn Fn(&[Asked]) -> Vec<Answer>,
    research: &'a dyn Fn(&Kit, &str) -> Result<String>,
    now: u64,
}

impl Host<'static> {
    /// This host `now`, as an orca terminal finds it.
    fn here(now: u64) -> Result<Host<'static>> {
        // Every harness is looked for, asked and probed in an orca terminal, the
        // shell a worker gets.
        if !crate::orca::running() {
            bail!(
                "the host kit looks for each harness, asks it for its models and probes them in \
                 an orca terminal, and orca is not running: {}",
                afkconvoy_core::tools::START_ORCA
            );
        }
        Ok(Host {
            found: &crate::launch::found,
            listed: &crate::launch::models,
            probe: &probe_all,
            research: &crate::kit_research::session,
            now,
        })
    }
}

/// How the host kit held is written again.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Writing {
    /// Whole: every harness and model is found, listed and probed again, and
    /// what is not is left out.
    Whole,
    /// A tick's daily refresh: a model its harness now lists is added once it
    /// answers its probe, and one it no longer lists dropped; the rest is
    /// held as it was, unprobed — a harness that cannot be found or listed
    /// too, which a pause, not the refresh, leaves out (docs/adr/0020).
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

/// The host kit this host can start now, from `current`, with no one asked
/// (docs/adr/0020) — and what was left out of it, and why:
///
/// - each known harness whose program `host` finds, on the command `current`
///   holds for it or its stock one;
/// - each with the models it lists, each name probed, of which only those
///   that answered are kept, with when and how fast; a harness none answered
///   for is left out;
/// - the names of one harness that answered as the same model held once, by
///   that model's id, with the others and the names the harness shows them
///   by as its names — a name its probe says nothing of being its own id;
/// - the harnesses `current` holds in its order, then new ones in the order
///   afkconvoy knows them; each one's models in the order it lists them;
/// - the stock consult `current` names while it still holds it, else none:
///   the host kit's first; and research held for the same answered identity,
///   including on another harness with the provider prefix left off.
///
/// A refresh keeps listed ids and probes new names and aliases again, so
/// an alias that moves is held under the model it now answers as.
fn compose(current: &Kit, host: &Host, writing: Writing) -> (Kit, Vec<LeftOut>) {
    let mut order: Vec<Harness> = current.harnesses.iter().map(|held| held.harness).collect();
    push_new(&mut order, Harness::ALL);
    let command = |harness: Harness| {
        current.command(harness).map_or_else(|| harness.stock().to_string(), str::to_string)
    };
    // Each harness is looked for and asked for its models side by side.
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

    let mut left_out = vec![];
    let mut harnesses: Vec<Held> = vec![];
    let mut asked: Vec<Asked> = vec![];
    let mut shown: Vec<(Harness, String, String)> = vec![];
    let mut names: Vec<(Harness, String)> = vec![];
    // Each harness's models in the order it lists them.
    let mut lists: Vec<(Harness, Vec<String>)> = vec![];
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
        let mut models: Vec<String> = vec![];
        for each in listed {
            if let Some(display) = each.shown {
                shown.push((harness, each.name.clone(), display));
            }
            push_new(&mut models, [each.name]);
        }
        names.extend(models.iter().map(|model| (harness, model.clone())));
        let kept: Vec<Seen> = if refresh {
            current
                .held(harness)
                .into_iter()
                .flat_map(|held| &held.models)
                .filter(|seen| models.contains(&seen.model))
                .cloned()
                .map(|mut seen| {
                    seen.names.clear();
                    seen
                })
                .collect()
        } else {
            vec![]
        };
        // Only literal ids are kept without a probe. Listed aliases may move.
        let new = models.iter().filter(|name| !kept.iter().any(|seen| &seen.model == *name));
        asked.extend(new.map(|name| (harness, command.clone(), name.clone())));
        harnesses.push(Held { harness, command, models: kept });
        lists.push((harness, models));
    }

    let answers = (host.probe)(&asked);
    for ((harness, _, name), answer) in asked.into_iter().zip(answers) {
        let (seconds, answered_as) = match answer {
            Answer::Answered(seconds, answered_as) => (seconds, answered_as),
            Answer::Silent => {
                left_out.push(LeftOut::of(harness, Some(&name), "silent to its probe"));
                continue;
            }
            Answer::Unasked(why) => {
                let why = format!("silent to its probe: it could not be asked: {why}");
                left_out.push(LeftOut::of(harness, Some(&name), why));
                continue;
            }
        };
        let Some(held) = harnesses.iter_mut().find(|held| held.harness == harness) else {
            continue;
        };
        let id = answered_as.unwrap_or_else(|| name.clone());
        let seen = match held.models.iter().position(|seen| seen.model == id) {
            Some(at) => &mut held.models[at],
            None => {
                let probed = Some(Probed { at: host.now, seconds });
                held.models.push(Seen {
                    model: id.clone(),
                    names: vec![],
                    probed,
                    good_at: None,
                    sources: vec![],
                    researched_at: None,
                });
                held.models.last_mut().expect("just pushed")
            }
        };
        if name != id && !seen.names.contains(&name) {
            seen.names.push(name);
        }
    }
    for held in &mut harnesses {
        for seen in &mut held.models {
            let display: Vec<String> = shown
                .iter()
                .filter(|(harness, named, _)| {
                    *harness == held.harness && (named == &seen.model || seen.names.contains(named))
                })
                .map(|(_, _, display)| display.clone())
                .collect();
            for display in display {
                push_new(&mut seen.names, [display]);
            }
            seen.names.retain(|name| name != &seen.model);
            if let Some(before) = previous(current, &seen.model) {
                seen.good_at = before.good_at.clone();
                seen.sources = before.sources.clone();
                seen.researched_at = before.researched_at;
            }
        }
    }
    // A model held before that is neither listed nor answered as now is gone.
    for before in &current.harnesses {
        let now = harnesses.iter().find(|held| held.harness == before.harness);
        let Some(now) = now else { continue };
        for seen in &before.models {
            let listed = names.iter().any(|(h, name)| *h == before.harness && name == &seen.model);
            if !listed && !now.models.iter().any(|kept| kept.model == seen.model) {
                left_out.push(LeftOut::of(before.harness, Some(&seen.model), "no longer listed"));
            }
        }
    }
    for (harness, listed) in &lists {
        if let Some(held) = harnesses.iter_mut().find(|held| held.harness == *harness) {
            held.models.sort_by_key(|seen| {
                listed.iter().position(|model| *model == seen.model || seen.names.contains(model))
            });
        }
    }
    for held in &mut harnesses {
        if let Some(before) = current.held(held.harness) {
            let mut ranked: Vec<_> = held
                .models
                .iter()
                .filter(|m| before.models.iter().any(|old| old.model == m.model))
                .cloned()
                .collect();
            ranked.sort_by_key(|m| before.models.iter().position(|old| old.model == m.model));
            // Only a successful research established a preference to preserve.
            if before.models.iter().any(|m| m.researched_at.is_some()) {
                let mut ranked = ranked.into_iter();
                for model in &mut held.models {
                    if before.models.iter().any(|old| old.model == model.model) {
                        *model = ranked.next().expect("one replacement per held slot");
                    }
                }
            }
        }
    }
    harnesses.retain(|held| {
        let kept = !held.models.is_empty();
        if !kept {
            left_out.push(LeftOut::of(held.harness, None, "no model answered its probe"));
        }
        kept
    });

    let mut kit = Kit { harnesses, stock_consult: None };
    kit.stock_consult = current.stock_consult.clone().filter(|named| {
        Kit { stock_consult: Some(named.clone()), ..kit.clone() }.stock_fault().is_none()
    });
    (kit, left_out)
}

/// The same answered identity held before, including on another harness.
/// Aliases are deliberately excluded: they may now answer as a different model.
fn previous<'a>(current: &'a Kit, id: &str) -> Option<&'a Seen> {
    let bare = |id: &str| id.rsplit('/').next().unwrap_or(id).to_lowercase();
    current
        .harnesses
        .iter()
        .flat_map(|held| &held.models)
        .filter(|before| bare(&before.model) == bare(id))
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
/// holds in order, what the stock consult runs, and what was left out.
fn report(path: &str, kit: &Kit, left_out: &[LeftOut]) -> String {
    let mut said = format!("afkconvoy: wrote the host kit at {path}\n");
    for held in &kit.harnesses {
        let models: Vec<&str> = held.models.iter().map(|seen| seen.model.as_str()).collect();
        said.push_str(&format!(
            "  {} `{}`: {}\n",
            held.harness.key(),
            held.command,
            models.join(" ")
        ));
    }
    match kit.stock_consult() {
        Some(named) => said.push_str(&format!("  the stock consult runs {}\n", named_said(&named))),
        None => said.push_str("  it holds no harness: no worker can be started\n"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(name: &str, shown: Option<&str>) -> Listed {
        Listed { name: name.into(), shown: shown.map(str::to_string) }
    }

    fn held(harness: Harness, command: &str, models: &[&str]) -> Held {
        let models = models
            .iter()
            .map(|model| Seen {
                model: model.to_string(),
                names: vec![],
                probed: Some(Probed { at: 1, seconds: 1.0 }),
                good_at: None,
                sources: vec![],
                researched_at: None,
            })
            .collect();
        Held { harness, command: command.into(), models }
    }

    /// A host with Claude Code, Codex and pi installed, DSH not; Claude Code
    /// lists `default opus sonnet`, shown as `Default`, `Opus 5.5` and `Sonnet
    /// 5.5`, of which `default` and `opus` answer as `claude-opus-5-5` and
    /// `sonnet` as `claude-sonnet-5-5`; Codex lists `gpt-a gpt-b`, and says
    /// nothing of what answered; pi fails to list; every model answers its
    /// probe in two seconds but `bad`, which is silent, and `gpt-b`, which no
    /// terminal could be made for.
    fn composed(current: &Kit) -> (Kit, Vec<LeftOut>, Vec<Asked>) {
        composed_as(current, Writing::Whole)
    }

    fn composed_as(current: &Kit, writing: Writing) -> (Kit, Vec<LeftOut>, Vec<Asked>) {
        let probed = std::sync::Mutex::new(vec![]);
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
            _ => Err(format!("it listed none: {command} --list-models")),
        };
        let probe = |asked: &[Asked]| {
            probed.lock().unwrap().extend(asked.iter().cloned());
            asked
                .iter()
                .map(|(_, _, model)| match model.as_str() {
                    "bad" => Answer::Silent,
                    "gpt-b" => Answer::Unasked("orca made no terminal".into()),
                    "default" | "opus" => Answer::Answered(2.0, Some("claude-opus-5-5".into())),
                    "sonnet" => Answer::Answered(2.0, Some("claude-sonnet-5-5".into())),
                    _ => Answer::Answered(2.0, None),
                })
                .collect()
        };
        let host = Host {
            research: &|_, _| bail!("fake research unavailable"),
            found: &found,
            listed: &listed,
            probe: &probe,
            now: 77,
        };
        let (kit, left_out) = compose(current, &host, writing);
        (kit, left_out, probed.into_inner().unwrap())
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
    fn with_no_host_kit_each_harness_found_is_held_on_its_stock_command_with_the_models_that_answered()
     {
        let (kit, left_out, probed) = composed(&Kit::default());
        assert_eq!(
            summary(&kit),
            [
                (
                    Harness::ClaudeCode,
                    Harness::ClaudeCode.stock(),
                    vec!["claude-opus-5-5", "claude-sonnet-5-5"]
                ),
                (Harness::Codex, Harness::Codex.stock(), vec!["gpt-a"]),
            ]
        );
        // The names that answered as one model are held once, by its id, with
        // the names its harness shows them by; one whose probe says nothing
        // of what answered is its own id.
        let names = |held: &Held| -> Vec<Vec<String>> {
            held.models.iter().map(|seen| seen.names.clone()).collect()
        };
        assert_eq!(
            names(&kit.harnesses[0]),
            [vec!["default", "opus", "Default", "Opus 5.5"], vec!["sonnet", "Sonnet 5.5"]]
        );
        assert_eq!(names(&kit.harnesses[1]), [Vec::<String>::new()]);
        let range = kit.range();
        for name in ["opus", "DEFAULT", "Opus 5.5", "claude-opus-5-5"] {
            assert_eq!(range[0].model(name).map(|model| model.id.as_str()), Ok("claude-opus-5-5"));
        }
        // Every model kept was probed and answered, and is held with when.
        assert_eq!(probed.len(), 6, "{probed:?}");
        for seen in kit.harnesses.iter().flat_map(|held| &held.models) {
            assert_eq!(seen.probed, Some(Probed { at: 77, seconds: 2.0 }));
        }
        assert_eq!(kit.stock_consult, None);
        let said: Vec<(Harness, Option<&str>, &str)> = left_out
            .iter()
            .map(|out| (out.harness, out.model.as_deref(), out.why.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (Harness::Pi, None, "listing failed: it listed none: pi --list-models"),
                (Harness::Dsh, None, "not installed: `type -- 'dsh-tui'` found nothing"),
                (Harness::Codex, Some("bad"), "silent to its probe"),
                (
                    Harness::Codex,
                    Some("gpt-b"),
                    "silent to its probe: it could not be asked: orca made no terminal"
                ),
            ]
        );
    }

    #[test]
    fn a_host_kit_written_again_keeps_its_order_commands_good_at_and_stock_consult() {
        let mut opus = held(Harness::ClaudeCode, "old", &["bad", "opus", "gone"]);
        opus.models[1].model = "claude-opus-5-5".into();
        opus.models[1].names = vec!["opus".into()];
        opus.models[1].good_at = Some("hard work".into());
        let current = Kit {
            harnesses: vec![held(Harness::Codex, "mycodex --yolo", &["gpt-a"]), opus],
            stock_consult: Some(Named { harness: Harness::ClaudeCode, model: Some("opus".into()) }),
        };
        let (kit, left_out, probed) = composed(&current);
        // Held harnesses keep their order and command; each one's models are
        // in the order it lists them.
        assert_eq!(
            summary(&kit),
            [
                (Harness::Codex, "mycodex --yolo", vec!["gpt-a"]),
                (Harness::ClaudeCode, "old", vec!["claude-opus-5-5", "claude-sonnet-5-5"]),
            ]
        );
        assert!(probed.iter().all(|(h, c, _)| *h != Harness::Codex || c == "mycodex --yolo"));
        // Research is kept by answered identity, never by a moving alias.
        assert_eq!(kit.harnesses[1].models[0].good_at.as_deref(), Some("hard work"));
        assert_eq!(kit.harnesses[1].models[1].good_at, None);
        // A stock consult named by a name runs the model's id.
        assert_eq!(kit.stock_consult, current.stock_consult);
        let stock = Named { harness: Harness::ClaudeCode, model: Some("claude-opus-5-5".into()) };
        assert_eq!(kit.stock_consult(), Some(stock));
        // A model gone from its harness's list is left out, and said so.
        assert!(
            left_out
                .iter()
                .any(|out| out.model.as_deref() == Some("gone") && out.why == "no longer listed")
        );
        assert!(
            left_out
                .iter()
                .any(|out| out.model.as_deref() == Some("bad") && out.why == "no longer listed")
        );
        // Written twice with nothing changed, it is the same.
        let (again, _, _) = composed(&kit);
        assert_eq!(again, kit);
    }

    #[test]
    fn a_stock_consult_no_longer_held_runs_the_host_kit_s_first_and_a_silent_harness_is_left_out() {
        let current = Kit {
            harnesses: vec![held(Harness::Pi, "pi", &["a/b"])],
            stock_consult: Some(Named { harness: Harness::Pi, model: None }),
        };
        let probe = |asked: &[Asked]| asked.iter().map(|_| Answer::Silent).collect();
        let host = Host {
            research: &|_, _| bail!("fake research unavailable"),
            found: &|_| Ok(()),
            listed: &|harness, _| match harness {
                Harness::Codex => Ok(vec![listed("bad", None)]),
                _ => Err("it failed".into()),
            },
            probe: &probe,
            now: 5,
        };
        let (kit, left_out) = compose(&current, &host, Writing::Whole);
        assert_eq!(kit, Kit::default());
        assert!(
            left_out.iter().any(|out| out.harness == Harness::Codex
                && out.model.is_none()
                && out.why == "no model answered its probe"),
            "{left_out:?}"
        );
        let said = report("/kit.json", &kit, &left_out);
        assert!(said.contains("no worker can be started"), "{said}");
        assert!(said.contains("left out: codex bad: silent to its probe"), "{said}");
    }

    #[test]
    fn a_refresh_adds_a_model_newly_listed_that_answers_drops_one_no_longer_listed_and_keeps_the_rest()
     {
        let mut claude = held(Harness::ClaudeCode, "claude", &["claude-opus-5-5", "gone"]);
        claude.models[0].good_at = Some("hard work".into());
        let current = Kit {
            harnesses: vec![
                claude,
                held(Harness::Codex, "codex", &["gpt-a"]),
                held(Harness::Pi, "pi", &["a/b"]),
                held(Harness::Dsh, "dsh", &["d"]),
            ],
            stock_consult: None,
        };
        let (kit, left_out, probed) = composed_as(&current, Writing::Refresh);
        // Pi fails to list and DSH is not found: each stops answering, and is
        // kept as it was, for a pause to leave out.
        assert_eq!(
            summary(&kit),
            [
                (Harness::ClaudeCode, "claude", vec!["claude-opus-5-5", "claude-sonnet-5-5"]),
                (Harness::Codex, "codex", vec!["gpt-a"]),
                (Harness::Pi, "pi", vec!["a/b"]),
                (Harness::Dsh, "dsh", vec!["d"]),
            ]
        );
        assert_eq!(kit.harnesses[2], current.harnesses[2]);
        assert_eq!(kit.harnesses[3], current.harnesses[3]);
        // Only a model it did not hold is probed; one held keeps its probe.
        let asked: Vec<&str> = probed.iter().map(|(_, _, model)| model.as_str()).collect();
        assert_eq!(asked, ["default", "opus", "sonnet", "bad", "gpt-b"]);
        let claude = &kit.harnesses[0].models;
        assert_eq!(claude[0].probed, Some(Probed { at: 77, seconds: 2.0 }));
        assert_eq!(claude[0].good_at, current.harnesses[0].models[0].good_at);
        assert!(
            left_out
                .iter()
                .any(|out| out.model.as_deref() == Some("gone") && out.why == "no longer listed")
        );
        let changed = Changed::between(&current, &kit);
        assert_eq!(
            changed,
            Changed {
                added: vec!["claude-code claude-sonnet-5-5".into()],
                dropped: vec!["claude-code gone".into()],
            }
        );
        assert_eq!(
            changed.said(),
            "the host kit was written again: added claude-code claude-sonnet-5-5; \
             dropped claude-code gone"
        );
        // Refreshed again with nothing changed, it is the same.
        let (again, _, probed) = composed_as(&kit, Writing::Refresh);
        assert_eq!(again, kit);
        assert!(Changed::between(&kit, &again).is_empty());
        assert_eq!(probed.len(), 5, "aliases and the two silent ones are asked again: {probed:?}");
    }

    #[test]
    fn a_refresh_follows_an_alias_that_moves_and_keeps_a_still_listed_id() {
        let mut claude = held(Harness::ClaudeCode, "claude", &["old-id"]);
        claude.models[0].names = vec!["opus".into()];
        let current = Kit { harnesses: vec![claude], stock_consult: None };
        let found =
            |harness| if harness == Harness::ClaudeCode { Ok(()) } else { Err("absent".into()) };
        let listed =
            |_: Harness, _: &str| Ok(vec![listed("opus", Some("Opus")), listed("old-id", None)]);
        let probe = |asked: &[Asked]| {
            assert_eq!(
                asked.iter().map(|(_, _, name)| name.as_str()).collect::<Vec<_>>(),
                ["opus"]
            );
            vec![Answer::Answered(1.0, Some("new-id".into()))]
        };
        let host = Host {
            research: &|_, _| bail!("fake research unavailable"),
            found: &found,
            listed: &listed,
            probe: &probe,
            now: 100,
        };
        let (kit, _) = compose(&current, &host, Writing::Refresh);
        assert_eq!(kit.matched(Harness::ClaudeCode, "opus").unwrap(), "new-id");
        assert_eq!(kit.matched(Harness::ClaudeCode, "Opus").unwrap(), "new-id");
        assert_eq!(kit.matched(Harness::ClaudeCode, "old-id").unwrap(), "old-id");
        assert_eq!(kit.harnesses[0].models.len(), 2);
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
    fn one_process_refreshes_the_host_kit_and_the_others_read_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("afkconvoy/kit.json");
        let current =
            Kit { harnesses: vec![held(Harness::Pi, "pi", &["a/b"])], stock_consult: None };
        store(&file, &current, 1000).unwrap();
        assert_eq!(written(&file), Some(1000));
        let probes = std::sync::atomic::AtomicUsize::new(0);
        let sessions = std::sync::atomic::AtomicUsize::new(0);
        let research = |_: &Kit, prompt: &str| {
            sessions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let targets = prompt
                .split("one entry per identity): ")
                .nth(1)
                .unwrap()
                .split(".\nContext")
                .next()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(targets).unwrap(),
                json!([{"harness":"pi", "model":"c/d"}])
            );
            bail!("fake session gave no answer")
        };
        let listed = |_: Harness, _: &str| Ok(vec![listed("a/b", None), listed("c/d", None)]);
        let probe = |asked: &[Asked]| {
            probes.fetch_add(asked.len(), std::sync::atomic::Ordering::SeqCst);
            asked.iter().map(|_| Answer::Answered(1.0, None)).collect()
        };
        let found = |harness: Harness| match harness {
            Harness::Pi => Ok(()),
            _ => Err("not here".to_string()),
        };
        let host =
            |now| Host { research: &research, found: &found, listed: &listed, probe: &probe, now };
        assert!(refresh_at(&file, 1000 + REFRESHED_EVERY - 1, || Ok(host(0))).unwrap().is_none());
        let now = 1000 + REFRESHED_EVERY;
        // While one process writes it again, another finds it locked and
        // reads the file as it is.
        let (writing, written_again) = (std::sync::Barrier::new(2), std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                refresh_at(&file, now, || {
                    writing.wait();
                    written_again.wait();
                    Ok(host(now))
                })
            });
            writing.wait();
            let other = refresh_at(&file, now, || panic!("it is not this process's to write"));
            assert!(other.unwrap().is_none());
            written_again.wait();
            let (before, after) = first.join().unwrap().unwrap().unwrap();
            assert_eq!(before, current);
            assert_eq!(summary(&after), [(Harness::Pi, "pi", vec!["a/b", "c/d"])]);
        });
        // Once written, it is not due again for a day, in any process.
        assert_eq!(written(&file), Some(now));
        assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(sessions.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(refresh_at(&file, now + 60, || panic!("refreshed already")).unwrap().is_none());
        // A refresh that fails is tried again an hour later, not every tick.
        let later = now + REFRESHED_EVERY;
        assert!(refresh_at(&file, later, || bail!("orca is not running")).is_err());
        assert!(refresh_at(&file, later + 60, || panic!("tried a minute ago")).unwrap().is_none());
        let again = refresh_at(&file, later + 3600, || Ok(host(later + 3600))).unwrap();
        assert!(again.is_some());
        assert_eq!(
            sessions.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "no new identity: no session on the next refresh"
        );
    }

    #[test]
    fn the_stock_consult_runs_what_is_named_or_the_host_kit_s_first() {
        let mut kit = Kit {
            harnesses: vec![
                held(Harness::Pi, "pi", &["a/b", "c/d"]),
                held(Harness::Codex, "codex", &[]),
            ],
            stock_consult: None,
        };
        let first = Named { harness: Harness::Pi, model: Some("a/b".into()) };
        assert_eq!(kit.stock_consult(), Some(first.clone()));
        let named = Named { harness: Harness::Codex, model: None };
        kit.stock_consult = Some(named.clone());
        assert_eq!(kit.fault(), None);
        assert_eq!(kit.stock_consult(), Some(named));
        // One the host kit does not hold: the host kit's first, and said.
        kit.stock_consult = Some(Named { harness: Harness::Dsh, model: None });
        assert_eq!(kit.fault(), None);
        assert!(kit.stock_fault().unwrap().contains("does not hold"));
        assert!(kit.stock_unmatched().unwrap().contains("`dsh`"));
        assert_eq!(kit.stock_consult(), Some(first.clone()));
        kit.stock_consult = Some(Named { harness: Harness::Pi, model: Some("x/y".into()) });
        assert!(kit.stock_unmatched().unwrap().contains("`x/y`"));
        assert_eq!(kit.stock_consult(), Some(first));
        assert_eq!(Kit::default().stock_consult(), None);
    }

    #[test]
    fn a_kit_reads_as_it_is_written() {
        let kit = Kit {
            harnesses: vec![held(Harness::ClaudeCode, "claude", &["opus"])],
            stock_consult: Some(Named { harness: Harness::ClaudeCode, model: None }),
        };
        let text = serde_json::to_string(&kit).unwrap();
        assert_eq!(serde_json::from_str::<Kit>(&text).unwrap(), kit);
        assert!(
            serde_json::from_str::<Kit>(r#"{"harnesses":[{"harness":"x","command":"x"}]}"#)
                .unwrap_err()
                .to_string()
                .contains("is no harness")
        );
    }
}

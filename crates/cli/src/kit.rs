//! The host kit (docs/adr/0020): the harnesses this host can start, in order
//! of preference — each one's worker launch command and the models it was
//! seen to answer with — and the stock consult's harness and model. This file
//! belongs to the host user, outside every checkout, and is written with no
//! human: `afkconvoy kit`, or `run` on a host without one.
use std::collections::BTreeSet;
use std::path::PathBuf;

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
    /// What it is good at, as the human wrote it in the host kit: what a
    /// caster is told of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub good_at: Option<String>,
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
        let Some(named) = self.stock_consult.clone() else { return self.first() };
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

    /// Why this is no host kit, when it is not.
    fn fault(&self) -> Option<String> {
        let mut seen = BTreeSet::new();
        for held in &self.harnesses {
            if !seen.insert(held.harness.key()) {
                return Some(format!("it holds the harness `{}` twice", held.harness.key()));
            }
        }
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
    let file = path()?;
    let unreadable =
        || format!("cannot read the host kit at {}: run `afkconvoy kit`", file.display());
    let text = std::fs::read_to_string(&file).with_context(unreadable)?;
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
                "harnesses": kit.harnesses,
                "stock_consult": running.stock_consult().map(|named| json!({
                    "harness": named.harness,
                    "model": named.model,
                    "named": running.stock_consult.is_some(),
                })),
                "paused": paused_said,
            })
        }
        Err(error) => json!({"path": file, "error": format!("{error:#}"), "paused": paused_said}),
    }
}

/// Writes the host kit for `run` where this host has none (`write`), saying
/// so on stderr; then reads it.
pub fn ensure() -> Result<()> {
    if !path()?.exists() {
        write()?;
    }
    load().map(|_| ())
}

/// Writes the host kit with no human (docs/adr/0020): everything this host
/// can start now (`compose`), whoever runs it and wherever. Says on stderr
/// where it is, what it holds and what was left out, and answers
/// `{path, written, kit, left_out}`.
pub fn write() -> Result<Value> {
    // Every harness is looked for, asked and probed in an orca terminal, the
    // shell a worker gets.
    if !crate::orca::running() {
        bail!(
            "the host kit looks for each harness, asks it for its models and probes them in an \
             orca terminal, and orca is not running: {}",
            afkconvoy_core::tools::START_ORCA
        );
    }
    let file = path()?;
    let current = if file.exists() { load()? } else { Kit::default() };
    let host = Host {
        found: &crate::launch::found,
        listed: &crate::launch::models,
        probe: &probe_all,
        now: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    };
    let (kit, left_out) = compose(&current, &host);
    let parent = file.parent().context("the host kit needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&kit)?)?;
    std::fs::rename(&temporary, &file)?;
    // A harness paused on the host that answered its probe here is back.
    let pauses = crate::paused::Pauses::here()?;
    for held in &kit.harnesses {
        pauses.resume(held.harness)?;
    }
    eprint!("{}", report(&file.display().to_string(), &kit, &left_out));
    Ok(json!({"path": file, "written": true, "kit": kit, "left_out": left_out}))
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
struct Host<'a> {
    /// Whether a harness's program is found; why it is taken as not.
    found: &'a (dyn Fn(Harness) -> Result<(), String> + Sync),
    /// The models a harness offers through a command, in its order; why it
    /// offers none.
    listed: &'a Listing,
    probe: &'a dyn Fn(&[Asked]) -> Vec<Answer>,
    now: u64,
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
///   the host kit's first; and the `good_at` written beside a model kept —
///   of the same model on another harness where none is written beside it
///   on its own, the provider prefix left off.
fn compose(current: &Kit, host: &Host) -> (Kit, Vec<LeftOut>) {
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
    for (harness, command, listed) in listed {
        let listed = match listed {
            Ok(listed) => listed,
            Err(why) => {
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
        asked.extend(models.into_iter().map(|model| (harness, command.clone(), model)));
        harnesses.push(Held { harness, command, models: vec![] });
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
                held.models.push(Seen { model: id.clone(), names: vec![], probed, good_at: None });
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
            seen.good_at = good_at(current, held.harness, seen);
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
    harnesses.retain(|held| {
        let kept = !held.models.is_empty();
        if !kept {
            left_out.push(LeftOut::of(held.harness, None, "no model answered its probe"));
        }
        kept
    });

    let mut kit = Kit { harnesses, stock_consult: None };
    kit.stock_consult = current.stock_consult.clone().filter(|named| {
        Kit { stock_consult: Some(named.clone()), ..kit.clone() }.fault().is_none()
    });
    (kit, left_out)
}

/// What `current` says the model `seen` of `harness` is good at: as written
/// beside it there — under its id, or one of its names — else beside the
/// same model on another harness, its id the same with the provider prefix
/// left off. One model has one `good_at`.
fn good_at(current: &Kit, harness: Harness, seen: &Seen) -> Option<String> {
    let bare = |id: &str| id.rsplit('/').next().unwrap_or(id).to_lowercase();
    let models = |of: Harness| {
        current.held(of).into_iter().flat_map(|held| held.models.iter()).map(move |m| (of, m))
    };
    let all = current.harnesses.iter().flat_map(|held| models(held.harness));
    let own = models(harness).find(|(_, before)| {
        before.good_at.is_some()
            && (before.model == seen.model || seen.names.contains(&before.model))
    });
    let own = own.and_then(|(_, before)| before.good_at.clone());
    own.or_else(|| {
        let mut same = all.filter(|(_, before)| bare(&before.model) == bare(&seen.model));
        same.find_map(|(_, before)| before.good_at.clone())
    })
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
        let host = Host { found: &found, listed: &listed, probe: &probe, now: 77 };
        let (kit, left_out) = compose(current, &host);
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
        // What was written of `opus` is the model it answers as.
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
            found: &|_| Ok(()),
            listed: &|harness, _| match harness {
                Harness::Codex => Ok(vec![listed("bad", None)]),
                _ => Err("it failed".into()),
            },
            probe: &probe,
            now: 5,
        };
        let (kit, left_out) = compose(&current, &host);
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
    fn the_stock_consult_runs_what_is_named_or_the_host_kit_s_first() {
        let mut kit = Kit {
            harnesses: vec![
                held(Harness::Pi, "pi", &["a/b", "c/d"]),
                held(Harness::Codex, "codex", &[]),
            ],
            stock_consult: None,
        };
        let first = Named { harness: Harness::Pi, model: Some("a/b".into()) };
        assert_eq!(kit.stock_consult(), Some(first));
        let named = Named { harness: Harness::Codex, model: None };
        kit.stock_consult = Some(named.clone());
        assert_eq!(kit.fault(), None);
        assert_eq!(kit.stock_consult(), Some(named));
        kit.stock_consult = Some(Named { harness: Harness::Dsh, model: None });
        assert!(kit.fault().unwrap().contains("does not hold"));
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

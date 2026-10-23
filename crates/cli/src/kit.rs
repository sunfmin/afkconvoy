//! The host kit (docs/adr/0020): the harnesses this host can start, in order
//! of preference — each one's worker launch command and the models it was
//! seen to answer with — and the stock consult's harness and model. This file
//! belongs to the host user, outside every checkout; only an explicit
//! terminal confirmation writes it.
use std::collections::BTreeSet;
use std::io::{BufRead, IsTerminal, Write};
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
    /// Its harness's own name for it.
    pub model: String,
    /// Its last probe, which it answered. `None`: its harness can be sent no
    /// prompt but in a worker's terminal (`Harness::prompt_question`), and it
    /// is held as the human named it.
    #[serde(default)]
    pub probed: Option<Probed>,
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

    /// What the stock consult runs: the harness and model named for it, or
    /// the host kit's first.
    pub fn stock_consult(&self) -> Option<Named> {
        self.stock_consult.clone().or_else(|| self.first())
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
        let Some(held) = self.held(named.harness) else {
            return Some(format!(
                "its stock consult runs the harness `{}`, which it does not hold",
                named.harness.key()
            ));
        };
        match &named.model {
            Some(model) if !held.models.iter().any(|seen| &seen.model == model) => Some(format!(
                "its stock consult runs the model `{model}`, which it holds for no `{}`",
                named.harness.key()
            )),
            _ => None,
        }
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
    let unreadable = || format!("cannot read the host kit at {}: run `afkconvoy kit`", file.display());
    let text = std::fs::read_to_string(&file).with_context(unreadable)?;
    let kit: Kit = serde_json::from_str(&text).with_context(unreadable)?;
    if let Some(fault) = kit.fault() {
        bail!("invalid host kit at {}: {fault}: run `afkconvoy kit`", file.display());
    }
    Ok(kit)
}

/// The harness and model the stock consult runs, as this host's host kit
/// says (`Kit::stock_consult`).
pub fn stock_consult() -> Result<Named> {
    load()?.stock_consult().context("the host kit holds no harness: run `afkconvoy kit`")
}

/// The host kit as `doctor` and `plan` show it: where it is kept, each
/// harness in order with its command and its models in order, each with when
/// it was last probed, and what the stock consult runs.
pub fn show() -> Value {
    let file = path().map(|path| path.display().to_string()).ok();
    match load() {
        Ok(kit) => json!({
            "path": file,
            "harnesses": kit.harnesses,
            "stock_consult": kit.stock_consult().map(|named| json!({
                "harness": named.harness,
                "model": named.model,
                "named": kit.stock_consult.is_some(),
            })),
        }),
        Err(error) => json!({"path": file, "error": format!("{error:#}")}),
    }
}

/// Ask for the kit only for `run`, and only when none has been written.
pub fn ensure() -> Result<()> {
    if !path()?.exists() {
        configure()?;
    }
    load().map(|_| ())
}

pub fn configure() -> Result<Value> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!(
            "the host kit needs a terminal: run `afkconvoy kit` at a terminal before starting the fleet"
        );
    }
    let file = path()?;
    let current = if file.exists() { load()? } else { Kit::default() };
    // Discover only on the first run. An omitted harness in an existing kit
    // was deliberately dropped, and must not silently be added again.
    let discovered: Vec<Harness> = if file.exists() {
        vec![]
    } else {
        Harness::ALL.into_iter().filter(|h| crate::launch::on_path(h.program())).collect()
    };
    let host = Host {
        listed: &|harness, command| crate::launch::models(harness, command).map(Result::ok),
        probe: &probe_all,
        now: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    };
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stderr();
    let Some(kit) = edit(current, &discovered, &host, &mut input, &mut out)? else {
        return Ok(json!({"path": file, "written": false}));
    };
    let parent = file.parent().context("the host kit needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&kit)?)?;
    std::fs::rename(&temporary, &file)?;
    Ok(json!({"path": file, "written": true, "kit": kit}))
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
                    scope.spawn(move || crate::launch::probe(*harness, command, model))
                })
                .collect();
            answers.extend(running.into_iter().map(|run| run.join().unwrap_or(Answer::Silent)));
        });
    }
    answers
}

/// A model to probe: its harness, that harness's worker launch command, and
/// the model.
type Asked = (Harness, String, String);

/// What a model said to its probe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Answer {
    /// It answered, in this many seconds.
    Answered(f64),
    /// It said nothing within the time limit, or failed.
    Silent,
    /// Its harness can be sent no prompt but in a worker's terminal.
    Unsendable,
}

/// What the editor asks of the host, apart from the human.
struct Host<'a> {
    /// The models a harness offers through a command (`None`: it has no way
    /// to say; `Some(None)`: asking it failed).
    listed: &'a dyn Fn(Harness, &str) -> Option<Option<Vec<String>>>,
    probe: &'a dyn Fn(&[Asked]) -> Vec<Answer>,
    now: u64,
}

fn reply(input: &mut impl BufRead, out: &mut impl Write, prompt: &str) -> Result<Option<String>> {
    write!(out, "{prompt}")?;
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
}

/// Asks `prompt` until `read` takes the reply; `None` at end of input.
fn ask<T>(
    input: &mut impl BufRead,
    out: &mut impl Write,
    prompt: &str,
    read: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>> {
    loop {
        let Some(answer) = reply(input, out, prompt)? else { return Ok(None) };
        match read(answer.trim()) {
            Ok(taken) => return Ok(Some(taken)),
            Err(why) => writeln!(out, "{why}")?,
        }
    }
}

/// Adds to `into` each of `items` it does not hold yet.
fn push_new<T: PartialEq>(into: &mut Vec<T>, items: impl IntoIterator<Item = T>) {
    for item in items {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

/// `current` put in the order a human typed, as words of `typed`: the ones
/// named first, in that order, then the rest as they were. Empty keeps it.
fn ordered(current: &[String], typed: &str) -> Result<Vec<String>, String> {
    let mut order: Vec<String> = vec![];
    for word in typed.split_whitespace() {
        if !current.iter().any(|name| name == word) {
            return Err(format!("`{word}` is none of: {}", current.join(" ")));
        }
        push_new(&mut order, [word.to_string()]);
    }
    push_new(&mut order, current.iter().cloned());
    Ok(order)
}

/// Walks the human through the host kit: each harness's command, the models
/// to probe, the probes' answers, the order of harnesses and of models, and
/// the stock consult's — then writes nothing unless they confirm. Shows
/// every known harness, so a previously dropped one can be added, too.
fn edit(
    current: Kit,
    discovered: &[Harness],
    host: &Host,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<Option<Kit>> {
    writeln!(out, "Host kit: keep with Enter, type a command to edit/add, or - to drop.")?;
    let mut order: Vec<Harness> = current.harnesses.iter().map(|held| held.harness).collect();
    push_new(&mut order, Harness::ALL);
    let mut commands: Vec<(Harness, String)> = vec![];
    for harness in order {
        let had = current.command(harness).map(str::to_string).or_else(|| {
            discovered.contains(&harness).then(|| harness.stock().to_string())
        });
        let shown = had.as_deref().unwrap_or("absent");
        let Some(answer) = reply(input, out, &format!("{}: {shown}\n> ", harness.key()))? else {
            return Ok(None);
        };
        let command = match answer.trim() {
            "-" => None,
            "" => had,
            _ => Some(answer),
        };
        commands.extend(command.map(|command| (harness, command)));
    }

    // The models to probe, per harness: those it held, then those it offers.
    writeln!(out, "Models: Enter probes those shown, type names to probe instead, - for none.")?;
    let mut asked: Vec<Asked> = vec![];
    for (harness, command) in &commands {
        let mut candidates: Vec<String> = current
            .held(*harness)
            .into_iter()
            .flat_map(|held| held.models.iter().map(|seen| seen.model.clone()))
            .collect();
        match (host.listed)(*harness, command) {
            Some(Some(listed)) => push_new(&mut candidates, listed),
            Some(None) => writeln!(out, "{}: asking it for its models failed", harness.key())?,
            None => writeln!(
                out,
                "{}: the fleet has no way to ask it for its models; name them by hand",
                harness.key()
            )?,
        }
        let shown = if candidates.is_empty() { "none".to_string() } else { candidates.join(" ") };
        let Some(answer) = reply(input, out, &format!("{} models: {shown}\n> ", harness.key()))?
        else {
            return Ok(None);
        };
        let models: Vec<String> = match answer.trim() {
            "-" => vec![],
            "" => candidates,
            typed => typed.split_whitespace().map(str::to_string).collect(),
        };
        let mut unique: Vec<String> = vec![];
        push_new(&mut unique, models);
        asked.extend(unique.into_iter().map(|model| (*harness, command.clone(), model)));
    }

    if !asked.is_empty() {
        writeln!(out, "Probing {} models with a one-line prompt…", asked.len())?;
    }
    let answers = (host.probe)(&asked);
    let mut harnesses: Vec<Held> = commands
        .into_iter()
        .map(|(harness, command)| Held { harness, command, models: vec![] })
        .collect();
    for ((harness, _, model), answer) in asked.into_iter().zip(answers) {
        let said = match answer {
            Answer::Answered(seconds) => format!("answered in {seconds:.1}s"),
            Answer::Silent => "did not answer: left out".to_string(),
            Answer::Unsendable => "cannot be sent a prompt here: held unprobed".to_string(),
        };
        writeln!(out, "  {} {model}: {said}", harness.key())?;
        let probed = match answer {
            Answer::Answered(seconds) => Some(Probed { at: host.now, seconds }),
            Answer::Unsendable => None,
            Answer::Silent => continue,
        };
        if let Some(held) = harnesses.iter_mut().find(|held| held.harness == harness) {
            held.models.push(Seen { model, probed });
        }
    }

    // The order: of harnesses, then of each one's models.
    if harnesses.len() > 1 {
        let keys: Vec<String> = harnesses.iter().map(|h| h.harness.key().to_string()).collect();
        let prompt = format!("Order of harnesses, first first: {}\n> ", keys.join(" "));
        let Some(keys) = ask(input, out, &prompt, |typed| ordered(&keys, typed))? else {
            return Ok(None);
        };
        harnesses.sort_by_key(|held| keys.iter().position(|key| key == held.harness.key()));
    }
    for held in &mut harnesses {
        if held.models.len() < 2 {
            continue;
        }
        let names: Vec<String> = held.models.iter().map(|seen| seen.model.clone()).collect();
        let prompt =
            format!("Order of {}'s models, first first: {}\n> ", held.harness.key(), names.join(" "));
        let Some(names) = ask(input, out, &prompt, |typed| ordered(&names, typed))? else {
            return Ok(None);
        };
        held.models.sort_by_key(|seen| names.iter().position(|name| name == &seen.model));
    }

    let mut kit = Kit { harnesses, stock_consult: None };
    let kept = current.stock_consult.filter(|named| {
        let probe = Kit { stock_consult: Some(named.clone()), ..kit.clone() };
        probe.fault().is_none()
    });
    if !kit.harnesses.is_empty() {
        let first = kit.first().map(|first| said(&first)).unwrap_or_default();
        let shown = kept.as_ref().map_or(format!("the host kit's first ({first})"), said);
        let prompt = format!(
            "Stock consult: {shown}\n  Enter keeps it, type a harness and a model to name one, \
             - for the host kit's first\n> "
        );
        let Some(named) = ask(input, out, &prompt, |typed| match typed {
            "" => Ok(kept.clone()),
            "-" => Ok(None),
            typed => {
                let mut words = typed.split_whitespace();
                let key = words.next().unwrap_or_default();
                let harness = Harness::read(key).ok_or_else(|| Harness::unknown(key))?;
                let named = Named { harness, model: words.next().map(str::to_string) };
                let probe = Kit { stock_consult: Some(named.clone()), ..kit.clone() };
                match probe.fault() {
                    Some(fault) => Err(format!("the host kit would be wrong: {fault}")),
                    None => Ok(Some(named)),
                }
            }
        })?
        else {
            return Ok(None);
        };
        kit.stock_consult = named;
    }

    writeln!(out, "{}", serde_json::to_string_pretty(&kit)?)?;
    let confirmed = reply(input, out, "Write this host kit? [y/N] ")?;
    Ok(confirmed
        .filter(|answer| answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
        .map(|_| kit))
}

/// A harness and model as the editor shows them.
fn said(named: &Named) -> String {
    match &named.model {
        Some(model) => format!("{} {model}", named.harness.key()),
        None => named.harness.key().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(harness: Harness, command: &str, models: &[&str]) -> Held {
        let models = models
            .iter()
            .map(|model| Seen {
                model: model.to_string(),
                probed: Some(Probed { at: 1, seconds: 1.0 }),
            })
            .collect();
        Held { harness, command: command.into(), models }
    }

    /// Lists `gpt-a gpt-b` for Codex, nothing for the rest; every model
    /// answers but `bad`, in two seconds, and DSH's cannot be sent one.
    fn host(probed: &std::cell::RefCell<Vec<Asked>>) -> impl Fn(&[u8]) -> (Result<Option<Kit>>, String) {
        move |replies: &[u8]| {
            let listed = |harness: Harness, _: &str| match harness {
                Harness::Codex => Some(Some(vec!["gpt-a".into(), "gpt-b".into()])),
                Harness::Pi => Some(None),
                _ => None,
            };
            let probe = |asked: &[Asked]| {
                probed.borrow_mut().extend(asked.iter().cloned());
                asked
                    .iter()
                    .map(|(harness, _, model)| match (harness, model.as_str()) {
                        (Harness::Dsh, _) => Answer::Unsendable,
                        (_, "bad") => Answer::Silent,
                        _ => Answer::Answered(2.0),
                    })
                    .collect()
            };
            let host = Host { listed: &listed, probe: &probe, now: 77 };
            let current = Kit {
                harnesses: vec![
                    held(Harness::ClaudeCode, "old", &["opus"]),
                    held(Harness::Pi, "pi", &[]),
                ],
                stock_consult: None,
            };
            let mut shown = vec![];
            let kit = edit(current, &[], &host, &mut &replies[..], &mut shown);
            (kit, String::from_utf8(shown).unwrap())
        }
    }

    #[test]
    fn editing_probes_each_model_and_writes_only_those_that_answered_in_the_order_given() {
        let probed = std::cell::RefCell::new(vec![]);
        let replies = concat!(
            // commands: claude-code kept, pi dropped, codex added, dsh added
            "\n-\nmycodex --yolo\ndsh-tui\n",
            // models: claude-code's held plus one named by hand, codex's
            // listed, dsh's named by hand
            "opus bad sonnet\n\nds-1\n",
            // order of harnesses: a typo asked again, then codex first
            "nope\ncodex\n",
            // order of models: codex's kept, claude-code's reversed
            "\nsonnet\n",
            // the stock consult: a model not held refused, then one named
            "claude-code bad\nclaude-code sonnet\ny\n",
        );
        let (kit, shown) = host(&probed)(replies.as_bytes());
        let kit = kit.unwrap().unwrap();
        let summary: Vec<(Harness, &str, Vec<&str>)> = kit
            .harnesses
            .iter()
            .map(|h| (h.harness, h.command.as_str(), h.models.iter().map(|m| m.model.as_str()).collect()))
            .collect();
        assert_eq!(
            summary,
            [
                (Harness::Codex, "mycodex --yolo", vec!["gpt-a", "gpt-b"]),
                (Harness::ClaudeCode, "old", vec!["sonnet", "opus"]),
                (Harness::Dsh, "dsh-tui", vec!["ds-1"]),
            ]
        );
        assert_eq!(kit.harnesses[0].models[0].probed, Some(Probed { at: 77, seconds: 2.0 }));
        assert_eq!(kit.harnesses[2].models[0].probed, None, "held unprobed");
        assert_eq!(
            kit.stock_consult,
            Some(Named { harness: Harness::ClaudeCode, model: Some("sonnet".into()) })
        );
        // every model named was probed through its harness's command, once
        let asked: Vec<(Harness, String)> =
            probed.borrow().iter().map(|(h, _, m)| (*h, m.clone())).collect();
        assert_eq!(asked.len(), 6, "{asked:?}");
        assert!(probed.borrow().iter().any(|(h, c, _)| *h == Harness::Codex && c == "mycodex --yolo"));
        for said in [
            "dsh: the fleet has no way to ask it for its models",
            "claude-code bad: did not answer",
            "codex gpt-a: answered in 2.0s",
            "dsh ds-1: cannot be sent a prompt here",
            "`nope` is none of",
            "which it holds for no `claude-code`",
        ] {
            assert!(shown.contains(said), "{said}\n{shown}");
        }
    }

    #[test]
    fn eof_or_declining_writes_nothing() {
        let probed = std::cell::RefCell::new(vec![]);
        let run = host(&probed);
        // EOF at the first harness; at the models; declining at the end
        for replies in ["", "\n\n\n\n", "\n-\n-\n-\n-\n\nn\n"] {
            assert!(run(replies.as_bytes()).0.unwrap().is_none(), "{replies:?}");
        }
    }

    #[test]
    fn the_stock_consult_runs_what_is_named_or_the_host_kit_s_first() {
        let mut kit = Kit {
            harnesses: vec![held(Harness::Pi, "pi", &["a/b", "c/d"]), held(Harness::Codex, "codex", &[])],
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
        assert!(serde_json::from_str::<Kit>(r#"{"harnesses":[{"harness":"x","command":"x"}]}"#)
            .unwrap_err()
            .to_string()
            .contains("is no harness"));
    }
}

//! A session the harness keeps of its own: reading the user's config, a fresh
//! directory, and the launch line. Codex is that harness. The facts — the
//! file, the model, the effort, the directory, the watch line — are the
//! harness's (docs/adr/0013). Here they are only read and put in place.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::line::Line;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::orca::Worktree;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// None means the harness's own default: the host kit places no model yet.
    pub model: Option<String>,
    pub model_source: String,
    /// None means the model's own default, not a fleet default.
    pub effort: Option<String>,
    pub effort_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<PathBuf>,
}

/// What a worker start resolved, including the line it runs.
pub(crate) struct Prepared {
    pub model: Option<String>,
    pub command: String,
    pub settings: Option<Settings>,
}

fn user_file(harness: Harness) -> PathBuf {
    let config = harness.session().map(|session| session.config).unwrap_or_default();
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(config)
}

/// The user's config, or empty when they have none. The path is what an
/// error names.
fn read_config(harness: Harness) -> Result<(String, String)> {
    let file = user_file(harness);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", file.display()));
        }
    };
    Ok((text, file.display().to_string()))
}

fn listed(harness: Harness) -> Result<Vec<String>> {
    let kit = crate::kit::load()?;
    Ok(kit
        .harnesses
        .iter()
        .find(|held| held.harness == harness)
        .map(|held| held.models.iter().map(|seen| seen.model.clone()).collect())
        .unwrap_or_default())
}

/// The model an automatic start uses. `None`: this harness has no user
/// preference, and the fleet's selection stands. An explicit model is that
/// model.
pub(crate) fn automatic_model(
    harness: Harness,
    explicit: Option<&str>,
) -> Result<Option<(Option<String>, &'static str)>> {
    if harness.session().is_none() {
        return Ok(None);
    }
    // An explicit model is that model. The user's file is read only when the
    // issue names none, and again for the effort as the session starts.
    if let Some(model) = explicit {
        return harness
            .automatic_model(Some(model), "", "", &[])
            .context("the harness names no user model")?
            .map(Some)
            .map_err(anyhow::Error::msg);
    }
    let (text, file) = read_config(harness)?;
    let listed = listed(harness)?;
    harness
        .automatic_model(None, &text, &file, &listed)
        .context("the harness names no user model")?
        .map(Some)
        .map_err(anyhow::Error::msg)
}

/// The model a watch starts on. A harness with no user preference keeps
/// `fleet`, and names no source.
pub(crate) fn watch_selection(
    harness: Harness,
    fleet: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    match automatic_model(harness, None)? {
        Some((model, source)) => Ok((model, Some(source.to_string()))),
        None => Ok((fleet, None)),
    }
}

fn effort(harness: Harness) -> Result<(Option<String>, String)> {
    let (text, file) = read_config(harness)?;
    harness.effort(&text, &file).context("the harness names no effort")?.map_err(anyhow::Error::msg)
}

/// A fresh directory's root, under the worktree, when this harness keeps a
/// session apart. The directory the host keeps is made under it.
pub(crate) fn fresh_root(harness: Harness, worktree: &Path) -> Option<PathBuf> {
    let session = harness.session()?;
    Some(worktree.join(crate::worker_tmp::TMP_DIR).join(format!(
        "{}-{}",
        session.fresh,
        crate::unique()
    )))
}

/// Resolves the model and the effort and returns the line a worker runs.
/// A harness with no session of its own is started as `command`, on the
/// fleet's model.
pub(crate) fn prepare_worker(
    harness: Harness,
    worktree: &Worktree,
    model: Option<&str>,
    explicit: bool,
    command: &str,
    in_role: bool,
) -> Result<Prepared> {
    let Some((model, model_source)) =
        automatic_model(harness, explicit.then_some(model).flatten())?
    else {
        let command = if in_role {
            command.to_string()
        } else {
            harness.in_worktree(command, &worktree.path.display().to_string(), "")
        };
        return Ok(Prepared { model: model.map(str::to_string), command, settings: None });
    };
    let (command, settings) =
        prepare(harness, worktree, model.as_deref(), command, in_role, model_source)?;
    Ok(Prepared { model, command, settings: Some(settings) })
}

/// Adds the effort at the last point before the session starts. A bound
/// command already names its directory; an unbound command gets the same
/// trust setup and allowed user links, in a fresh directory.
fn prepare(
    harness: Harness,
    worktree: &Worktree,
    model: Option<&str>,
    command: &str,
    in_role: bool,
    model_source: &str,
) -> Result<(String, Settings)> {
    let session = harness.session().context("the harness keeps no session of its own")?;
    let (effort, effort_source) = effort(harness)?;
    let command = harness.ensure_model(command, model);
    let (line, home) = if in_role {
        let home = std::fs::read_to_string(worktree.note(session.note)?)?;
        (Line::new().launch(&command), PathBuf::from(home))
    } else {
        let (home, _) = crate::kept::for_worker(
            harness,
            true,
            &worktree.path,
            &worktree.path,
            &Default::default(),
            "",
        )?;
        let command = harness.in_worktree(&command, &worktree.path.display().to_string(), &home);
        (Line::new().launch(&command), PathBuf::from(home))
    };
    let settings = Settings {
        model: model.map(str::to_string),
        model_source: model_source.into(),
        effort: effort.clone(),
        effort_source,
        codex_home: Some(home),
    };
    Ok((harness.with_effort(&line.done(), effort.as_deref()), settings))
}

/// A watch's launch line. A harness with a session of its own gets a fresh
/// directory and its own line, search included. Every other harness is
/// `Harness::watch` in the directory the host keeps.
pub(crate) fn watch_line(
    harness: Harness,
    worktree: &Path,
    model: Option<&str>,
    command: &str,
) -> Result<String> {
    let (kept, effort) = match fresh_root(harness, worktree) {
        Some(root) => {
            let home = crate::kept::keep_under(
                &root,
                std::env::var_os("HOME").map(PathBuf::from).as_deref(),
                harness,
                true,
            )?;
            let (effort, _) = effort(harness)?;
            (home.display().to_string(), effort)
        }
        None => (crate::kept::keep(harness, true)?.display().to_string(), None),
    };
    Ok(harness.session_watch(
        command,
        &worktree.display().to_string(),
        &kept,
        model,
        effort.as_deref(),
    ))
}

pub(crate) fn record(worktree: &Worktree, harness: Harness, settings: &Settings) -> Result<()> {
    let Some(session) = harness.session() else { return Ok(()) };
    std::fs::write(worktree.note(session.record)?, serde_json::to_vec(settings)?)?;
    Ok(())
}

pub(crate) fn require_in_role(
    harness: Harness,
    model: &str,
    allowed: &[String],
    role: &str,
) -> Result<()> {
    if allowed.iter().any(|allowed| allowed == model) {
        return Ok(());
    }
    match harness.outside_role(model, role) {
        Some(why) => bail!(why),
        None => Ok(()),
    }
}

pub(crate) fn recorded(worktree: &Worktree) -> Option<Settings> {
    let harness = worktree.harness().ok().flatten()?;
    let session = harness.session()?;
    let mut settings: Settings =
        serde_json::from_slice(&std::fs::read(worktree.note(session.record).ok()?).ok()?).ok()?;
    // A launch recorded before a model label was called that still says so.
    if settings.model_source == "casting" {
        settings.model_source = "model_label".into();
    }
    if let Some(home) = &settings.codex_home {
        let file = home.join("config.toml");
        if let Ok(text) = std::fs::read_to_string(&file)
            && let Some(saved) = harness.saved(&file.display().to_string(), &text)
        {
            if let Some(model) = saved.model {
                settings.model = Some(model);
                settings.model_source = saved.model_source.unwrap_or(settings.model_source);
            }
            if let Some(effort) = saved.effort {
                settings.effort = Some(effort);
                settings.effort_source = saved.effort_source.unwrap_or(settings.effort_source);
            }
        }
    }
    Some(settings)
}

pub(crate) fn planned(harness: Harness, model: Option<&str>, explicit: bool) -> Value {
    if harness.session().is_none() {
        return Value::Null;
    }
    let selected = (|| -> Result<Settings> {
        let (text, file) = read_config(harness)?;
        let listed = listed(harness)?;
        let (model, model_source) = harness
            .automatic_model(explicit.then_some(model).flatten(), &text, &file, &listed)
            .context("the harness names no user model")?
            .map_err(anyhow::Error::msg)?;
        let (effort, effort_source) = harness
            .effort(&text, &file)
            .context("the harness names no effort")?
            .map_err(anyhow::Error::msg)?;
        Ok(Settings {
            model,
            model_source: model_source.into(),
            effort,
            effort_source,
            codex_home: None,
        })
    })();
    match selected {
        Ok(settings) => json!(settings),
        Err(error) => json!({
            "model": model,
            "model_source": if explicit { "model_label" } else { "user_selection" },
            "error": format!("{error:#}"),
        }),
    }
}

/// The launch snapshots of this checkout's workers. Reading the user's
/// settings again here would describe a future launch, not these sessions.
pub(crate) fn workers(repo: &crate::git::Repo) -> Vec<Value> {
    let Some(workers) =
        crate::orca::Source::of(repo).and_then(|source| Worktree::of_sessions(&source)).ok()
    else {
        return vec![];
    };
    workers
        .into_iter()
        .filter_map(|(issue, worktree)| {
            let harness = worktree.harness().ok().flatten()?;
            let settings = recorded(&worktree)?;
            let mut value = json!(settings);
            value["issue"] = json!(issue);
            value["worktree"] = json!(worktree.path);
            value["harness"] = json!(harness.key());
            Some(value)
        })
        .collect()
}

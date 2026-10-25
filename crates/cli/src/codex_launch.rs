//! Codex model and effort precedence, and launch evidence. Each session has
//! its own user directory: Codex may save a /model choice there, but no later
//! worker reads it. Only the host user's effort is read at every launch.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::line::Line;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::orca::Worktree;

const RECORD: &str = "convoy-codex-launch.json";
pub(crate) const HOME_NOTE: &str = "convoy-codex-home";
const EFFORT: &str = "model_reasoning_effort";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Settings {
    pub model: String,
    pub model_source: String,
    /// None means the model's own Codex default, not a fleet default.
    pub effort: Option<String>,
    pub effort_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<PathBuf>,
}

fn user_config() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".codex/config.toml")
}

fn config(file: &Path) -> Result<toml::Table> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(toml::Table::new()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", file.display())),
    };
    text.parse().with_context(|| format!("cannot parse {}; fix its TOML", file.display()))
}

fn settings(model: &str, config: &toml::Table, file: &Path) -> Result<Settings> {
    let effort = match config.get(EFFORT) {
        Some(toml::Value::String(effort)) if !effort.trim().is_empty() => Some(effort.clone()),
        None => None,
        _ => bail!(
            "{}: {EFFORT} must be a nonempty string; fix it or remove it for Codex's default",
            file.display()
        ),
    };
    let effort_source = match effort {
        Some(_) => format!("{}:{EFFORT}", file.display()),
        None => "codex_default".into(),
    };
    Ok(Settings {
        model: model.into(),
        model_source: "fleet_selection".into(),
        effort,
        effort_source,
        codex_home: None,
    })
}

/// A fresh launch's directory, under the worktree so it goes with the work.
pub(crate) fn directory(worktree: &Path) -> PathBuf {
    worktree.join(crate::worker_tmp::TMP_DIR).join(format!("codex-{}", crate::unique()))
}

/// An unbound session keeps the user's other settings and files, as before,
/// but never writes preferences back to that user's configuration. The
/// model and effort have one source each and are supplied on the command.
fn user_directory(worktree: &Path, mut config: toml::Table, file: &Path) -> Result<PathBuf> {
    let dir = directory(worktree);
    std::fs::create_dir_all(&dir)?;
    if let Some(source) = file.parent()
        && source.is_dir()
    {
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            if entry.file_name() != "config.toml" {
                std::os::unix::fs::symlink(entry.path(), dir.join(entry.file_name()))?;
            }
        }
    }
    remove_preferences(&mut config);
    if let Some(profiles) = config.get_mut("profiles").and_then(toml::Value::as_table_mut) {
        for profile in profiles.iter_mut().filter_map(|(_, value)| value.as_table_mut()) {
            remove_preferences(profile);
        }
    }
    // A config can contain credentials of a custom provider. Give the
    // snapshot the same private permissions as a login, regardless of umask.
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("config.toml"))?;
    out.write_all(toml::to_string(&config)?.as_bytes())?;
    Ok(dir)
}

fn remove_preferences(config: &mut toml::Table) {
    config.remove("model");
    config.remove(EFFORT);
}

/// Validate before ending a worker that a continuation would replace.
pub(crate) fn selected(harness: Harness, model: Option<&str>) -> Result<()> {
    if harness == Harness::Codex && model.is_none_or(str::is_empty) {
        bail!(
            "the host kit holds no selected Codex model: run `afkconvoy kit`; no Codex worker was started"
        );
    }
    Ok(())
}

/// Adds authoritative effort at the last point before a Codex session
/// starts. A bound command already names its isolated role directory; an
/// unbound command uses a fresh snapshot of the user's other settings.
pub(crate) fn prepare(
    worktree: &Worktree,
    model: Option<&str>,
    command: &str,
    in_role: bool,
) -> Result<(String, Settings)> {
    selected(Harness::Codex, model)?;
    let model = model.context("selected Codex model")?;
    let file = user_config();
    let config = config(&file)?;
    let mut settings = settings(model, &config, &file)?;
    let (line, home) = if in_role {
        let home = std::fs::read_to_string(worktree.note(HOME_NOTE)?)?;
        (Line::new().launch(command), PathBuf::from(home))
    } else {
        let dir = user_directory(&worktree.path, config, &file)?;
        let line = Line::new()
            .fixed("export CODEX_HOME=")
            .word(&dir.display().to_string())
            .fixed(" && ")
            .launch(command);
        (line, dir)
    };
    settings.codex_home = Some(home);
    let line = match &settings.effort {
        Some(effort) => line
            .fixed(" --config ")
            .word(&format!("{EFFORT}={}", toml::Value::String(effort.clone()))),
        None => line,
    };
    Ok((line.done(), settings))
}

pub(crate) fn record(worktree: &Worktree, settings: &Settings) -> Result<()> {
    std::fs::write(worktree.note(RECORD)?, serde_json::to_vec(settings)?)?;
    Ok(())
}

/// A caster is started by a shell-only auxiliary, then makes one headless
/// Codex call. It gets the same fresh directory and user effort as an
/// interactive worker, and records the call in its own worktree.
pub(crate) fn caster_question(
    worktree: &Worktree,
    command: &str,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    let (kept, _) = crate::kept::for_worker(Harness::Codex, false, &worktree.path, &worktree.path)?;
    let question = Harness::Codex.prompt_question(command, &kept, model, prompt);
    let (question, settings) = prepare(worktree, model, &question, true)?;
    // A human may also run `cast` in an existing worker's worktree. That
    // helper call must not overwrite the existing worker's launch evidence.
    if worktree.caster()?.is_some() {
        record(worktree, &settings)?;
    }
    Ok(question)
}

pub(crate) fn recorded(worktree: &Worktree) -> Option<Settings> {
    (worktree.harness().ok().flatten()? == Harness::Codex).then_some(())?;
    let mut settings: Settings =
        serde_json::from_slice(&std::fs::read(worktree.note(RECORD).ok()?).ok()?).ok()?;
    // /model saves to this session's private config. Surface that choice
    // when present; editing the user's original config still affects only
    // future launches. An incomplete write leaves the launch evidence intact.
    if let Some(home) = &settings.codex_home {
        let file = home.join("config.toml");
        if let Ok(saved) = config(&file) {
            if let Some(model) = saved.get("model").and_then(toml::Value::as_str) {
                settings.model = model.into();
                settings.model_source = format!("session:{}:model", file.display());
            }
            if let Some(effort) = saved.get(EFFORT).and_then(toml::Value::as_str) {
                settings.effort = Some(effort.into());
                settings.effort_source = format!("session:{}:{EFFORT}", file.display());
            }
        }
    }
    Some(settings)
}

pub(crate) fn planned(model: &str) -> Value {
    let file = user_config();
    match config(&file).and_then(|config| settings(model, &config, &file)) {
        Ok(settings) => json!(settings),
        Err(error) => {
            json!({"model":model,"model_source":"fleet_selection","error":format!("{error:#}")})
        }
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
            let settings = recorded(&worktree)?;
            let mut value = json!(settings);
            value["issue"] = json!(issue);
            value["worktree"] = json!(worktree.path);
            value["harness"] = json!("codex");
            Some(value)
        })
        .collect()
}

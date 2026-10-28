//! Codex model and effort precedence, and launch evidence. Each session has
//! its own user directory. Automatic starts deliberately read the user's
//! model and effort preferences, while session-local changes stay local.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::line::Line;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::orca::Worktree;

const RECORD: &str = "convoy-codex-launch.json";

/// The note beside a worktree's git directory whose text is this session's home.
pub(crate) fn home_note() -> &'static str {
    Harness::Codex.noted_home().expect("Codex keeps a noted session home")
}
const EFFORT: &str = "model_reasoning_effort";
const MODEL: &str = "model";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// None means Codex's own default: the host kit places no Codex model yet.
    pub model: Option<String>,
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

fn settings(
    model: Option<&str>,
    source: &str,
    config: &toml::Table,
    file: &Path,
) -> Result<Settings> {
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
        model: model.map(str::to_string),
        model_source: source.into(),
        effort,
        effort_source,
        codex_home: None,
    })
}

/// Resolve an automatic model from the user's current Codex preference. A
/// model label on an issue is explicit and keeps its existing casting.
pub(crate) fn automatic_model(explicit: Option<&str>) -> Result<(Option<String>, &'static str)> {
    if let Some(model) = explicit {
        return Ok((Some(model.to_string()), "casting"));
    }
    let file = user_config();
    let config = config(&file)?;
    match config.get(MODEL) {
        Some(toml::Value::String(model)) if !model.trim().is_empty() => {
            let kit = crate::kit::load()?;
            let listed = kit
                .harnesses
                .iter()
                .find(|held| held.harness == Harness::Codex)
                .is_some_and(|held| held.models.iter().any(|listed| listed.model == *model));
            if !listed {
                bail!(
                    "Codex user selection `{model}` from {}:{MODEL} is not listed by the host kit; confirm the model is available to this Codex login and refresh `afkconvoy kit`",
                    file.display()
                );
            }
            Ok((Some(model.clone()), "user_selection"))
        }
        None => Ok((None, "codex_default")),
        _ => bail!(
            "{}: {MODEL} must be a nonempty string; fix it or remove it for Codex's default",
            file.display()
        ),
    }
}

pub(crate) fn require_role_model(model: &str, allowed: &[String], role: &str) -> Result<()> {
    if !allowed.iter().any(|allowed| allowed == model) {
        bail!(
            "Codex user selection `{model}` from ~/.codex/config.toml is outside role `{role}`'s range; update the role's roster to allow it or choose a model the role allows"
        );
    }
    Ok(())
}

/// A fresh launch's directory, under the worktree so it goes with the work.
pub(crate) fn directory(worktree: &Path) -> PathBuf {
    worktree.join(crate::worker_tmp::TMP_DIR).join(format!("codex-{}", crate::unique()))
}

/// Adds authoritative effort at the last point before a Codex session
/// starts. A bound command already names its isolated role directory; an
/// unbound command gets the same trust setup and allowed user links, in a
/// fresh directory, as the fleet's worktree launch contract requires.
pub(crate) fn prepare(
    worktree: &Worktree,
    model: Option<&str>,
    command: &str,
    in_role: bool,
    model_source: &str,
) -> Result<(String, Settings)> {
    let file = user_config();
    let config = config(&file)?;
    let mut settings = settings(model, model_source, &config, &file)?;
    let command = match model {
        Some(model) if !command.contains("--model") => Harness::Codex.with_model(command, model),
        Some(_) => command.to_string(),
        None => command.to_string(),
    };
    let (line, home) = if in_role {
        let home = std::fs::read_to_string(worktree.note(home_note())?)?;
        (Line::new().launch(&command), PathBuf::from(home))
    } else {
        let (home, _) = crate::kept::for_worker(
            Harness::Codex,
            true,
            &worktree.path,
            &worktree.path,
            &Default::default(),
            "",
        )?;
        let command =
            Harness::Codex.in_worktree(&command, &worktree.path.display().to_string(), &home);
        (Line::new().launch(&command), PathBuf::from(home))
    };
    settings.codex_home = Some(home);
    Ok((with_effort(line, &settings), settings))
}

fn with_effort(line: Line, settings: &Settings) -> String {
    let line = match &settings.effort {
        Some(effort) => line
            .fixed(" --config ")
            .word(&format!("{EFFORT}={}", toml::Value::String(effort.clone()))),
        None => line,
    };
    line.done()
}

/// A watch's directory need not be git. Its fresh Codex home and user effort
/// follow the worker contract without writing a worktree's launch notes.
pub(crate) fn watch(
    worktree: &Path,
    model: Option<&str>,
    source: &str,
    command: &str,
) -> Result<String> {
    let file = user_config();
    let settings = settings(model, source, &config(&file)?, &file)?;
    let home = crate::kept::keep_under(
        &directory(worktree),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        Harness::Codex,
        true,
    )?;
    let command = match model {
        Some(model) => Harness::Codex.with_model(command, model),
        None => command.to_string(),
    };
    let line = Harness::Codex.in_worktree(
        &command,
        &worktree.display().to_string(),
        &home.display().to_string(),
    );
    Ok(with_effort(Line::new().launch(&line), &settings))
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
    let (kept, _) = crate::kept::for_worker(
        Harness::Codex,
        false,
        &worktree.path,
        &worktree.path,
        &Default::default(),
        "",
    )?;
    let question = Harness::Codex.prompt_question(command, &kept, model, prompt);
    let (question, settings) = prepare(worktree, model, &question, true, "casting")?;
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
                settings.model = Some(model.into());
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

pub(crate) fn planned(model: Option<&str>, explicit: bool) -> Value {
    let file = user_config();
    let selected = automatic_model(explicit.then_some(model).flatten());
    match selected.and_then(|(model, source)| {
        let config = config(&file)?;
        settings(model.as_deref(), source, &config, &file)
    }) {
        Ok(settings) => json!(settings),
        Err(error) => {
            json!({"model":model,"model_source":if explicit {"casting"} else {"user_selection"},"error":format!("{error:#}")})
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

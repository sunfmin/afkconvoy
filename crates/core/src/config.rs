//! A repo's config: the six keys of `docs/agents/afkconvoy.md` (docs/adr/0001).

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

/// The keys as a repo wrote them: any may be left out, none is yet checked.
/// A key that is not one of these is refused where the file is read.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub base_branch: Option<String>,
    pub gate_command: Option<String>,
    pub ready_label: Option<String>,
    pub epic_labels: Option<Vec<String>>,
    pub concurrency: Option<usize>,
    pub escalate_label: Option<String>,
}

/// A config a fleet can run on: every key present, defaults filled.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[refined_by(concurrency: int)]
#[invariant(concurrency > 0)]
pub struct Config {
    pub base_branch: String,
    pub gate_command: String,
    pub ready_label: String,
    pub epic_labels: Vec<String>,
    #[field(usize[concurrency])]
    concurrency: usize,
    pub escalate_label: String,
}

/// Why a repo's settings are no config. Each names the key at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A key with no default was left out, or set to nothing.
    Required(&'static str),
    /// A label key set to the empty string.
    Empty(&'static str),
    ZeroConcurrency,
    /// `escalate_label` is `ready_label`: escalating would leave the issue dispatchable.
    EscalatesToReady,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Required(key) => write!(f, "{key}: required, and it has no default"),
            ConfigError::Empty(key) => write!(f, "{key}: must not be empty"),
            ConfigError::ZeroConcurrency => write!(f, "concurrency: must be at least 1"),
            ConfigError::EscalatesToReady => {
                write!(f, "escalate_label: must differ from ready_label")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// The config `settings` asks for, defaults filled — or the first key that
    /// is missing or cannot be run on.
    #[trusted]
    pub fn new(settings: Settings) -> Result<Config, ConfigError> {
        let base_branch = required("base_branch", settings.base_branch)?;
        let gate_command = required("gate_command", settings.gate_command)?;
        let ready_label = label("ready_label", settings.ready_label, "ready-for-agent")?;
        let escalate_label = label("escalate_label", settings.escalate_label, "ready-for-human")?;
        if escalate_label == ready_label {
            return Err(ConfigError::EscalatesToReady);
        }
        let epic_labels = settings.epic_labels.unwrap_or_else(|| vec!["epic".into(), "prd".into()]);
        match checked(
            base_branch,
            gate_command,
            ready_label,
            epic_labels,
            settings.concurrency.unwrap_or(3),
            escalate_label,
        ) {
            Some(config) => Ok(config),
            None => Err(ConfigError::ZeroConcurrency),
        }
    }

    /// How many claims the fleet holds at once. Never zero.
    #[spec(fn(&Config[@c]) -> usize{n: n == c && n > 0})]
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }
}

/// The one place a `Config` is built: a zero concurrency never gets in.
#[spec(fn(String, String, String, Vec<String>, concurrency: usize, String)
    -> Option<Config[concurrency]>)]
fn checked(
    base_branch: String,
    gate_command: String,
    ready_label: String,
    epic_labels: Vec<String>,
    concurrency: usize,
    escalate_label: String,
) -> Option<Config> {
    if concurrency == 0 {
        return None;
    }
    Some(Config {
        base_branch,
        gate_command,
        ready_label,
        epic_labels,
        concurrency,
        escalate_label,
    })
}

#[trusted]
fn required(key: &'static str, value: Option<String>) -> Result<String, ConfigError> {
    match value {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(ConfigError::Required(key)),
    }
}

#[trusted]
fn label(key: &'static str, value: Option<String>, default: &str) -> Result<String, ConfigError> {
    match value {
        None => Ok(default.to_string()),
        Some(text) if text.trim().is_empty() => Err(ConfigError::Empty(key)),
        Some(text) => Ok(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Settings {
        Settings {
            base_branch: Some("main".into()),
            gate_command: Some("make test".into()),
            ..Settings::default()
        }
    }

    #[test]
    fn defaults_fill_what_a_repo_left_out() {
        let config = Config::new(minimal()).unwrap();
        assert_eq!(config.ready_label, "ready-for-agent");
        assert_eq!(config.escalate_label, "ready-for-human");
        assert_eq!(config.epic_labels, ["epic", "prd"]);
        assert_eq!(config.concurrency(), 3);
    }

    #[test]
    fn the_two_keys_without_a_default_are_required() {
        let no_base = Settings { base_branch: None, ..minimal() };
        assert_eq!(Config::new(no_base), Err(ConfigError::Required("base_branch")));
        let blank_gate = Settings { gate_command: Some("  ".into()), ..minimal() };
        assert_eq!(Config::new(blank_gate), Err(ConfigError::Required("gate_command")));
    }

    #[test]
    fn a_config_nothing_could_run_on_is_refused() {
        let zero = Settings { concurrency: Some(0), ..minimal() };
        assert_eq!(Config::new(zero), Err(ConfigError::ZeroConcurrency));
        let same = Settings { escalate_label: Some("ready-for-agent".into()), ..minimal() };
        assert_eq!(Config::new(same), Err(ConfigError::EscalatesToReady));
        let empty = Settings { ready_label: Some(String::new()), ..minimal() };
        assert_eq!(Config::new(empty), Err(ConfigError::Empty("ready_label")));
    }
}

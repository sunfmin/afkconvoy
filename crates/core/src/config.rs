//! A repo's config: the keys of `docs/agents/afkconvoy.md` (docs/adr/0001), and
//! whether they bind the repo to a team (docs/adr/0008).

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

use crate::harness::Harness;

/// The escalate label where nothing names one: a repo's, and a team's.
pub const ESCALATE_LABEL: &str = "ready-for-human";

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
    pub team: Option<String>,
    pub patrols: Option<Vec<String>>,
    pub lock_label: Option<String>,
    pub intake_limit: Option<u64>,
    pub harness: Option<String>,
}

/// What a bound repo's config says beside the keys every repo has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bound {
    /// The team's repo.
    pub team: RepoName,
    /// Which of the team's patrols run on this repo.
    pub patrols: Vec<String>,
    /// How many issues taken in by workers may be open at once.
    pub intake_limit: u64,
    /// The harness of every worker nothing of the team names one for: a
    /// stage's or a patrol's that names none, a consult, a landing and the
    /// proposal of a precedent.
    pub harness: Harness,
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
    /// An open issue carrying this is a fleet lock, in a bound repo and an
    /// unbound one alike.
    pub lock_label: String,
    /// `None` in an unbound repo, whose config is the keys above and nothing else.
    #[serde(flatten)]
    pub bound: Option<Bound>,
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
    /// `team` is not `owner/repo`.
    NotARepo(&'static str),
    /// A key that means something only in a bound repo, in a repo naming no team.
    NeedsTeam(&'static str),
    /// `harness` names none: what it was set to.
    NotAHarness(String),
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
            ConfigError::NotARepo(key) => write!(f, "{key}: must be a repo, as owner/repo"),
            ConfigError::NeedsTeam(key) => {
                write!(f, "{key}: is read only in a bound repo, and this one names no team")
            }
            ConfigError::NotAHarness(named) => write!(f, "harness: {}", Harness::unknown(named)),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// The harness of every worker nothing names one for: the one a bound
    /// repo's config names. An unbound repo knows of no harness, and its one
    /// worker launch command is kept as the default's.
    pub fn harness(&self) -> Harness {
        self.bound.as_ref().map_or(Harness::DEFAULT, |bound| bound.harness)
    }

    /// The config `settings` asks for, defaults filled — or the first key that
    /// is missing or cannot be run on.
    pub fn new(settings: Settings) -> Result<Config, ConfigError> {
        let base_branch = required("base_branch", settings.base_branch)?;
        let gate_command = required("gate_command", settings.gate_command)?;
        let ready_label = label("ready_label", settings.ready_label, "ready-for-agent")?;
        let escalate_label = label("escalate_label", settings.escalate_label, ESCALATE_LABEL)?;
        if escalate_label == ready_label {
            return Err(ConfigError::EscalatesToReady);
        }
        let lock_label = label("lock_label", settings.lock_label, "fleet-lock")?;
        let epic_labels = settings.epic_labels.unwrap_or_else(|| vec!["epic".into(), "prd".into()]);
        let bound = match settings.team {
            Some(team) => Some(Bound {
                team: repo("team", team)?,
                patrols: settings.patrols.unwrap_or_default(),
                intake_limit: settings.intake_limit.unwrap_or(50),
                harness: match settings.harness {
                    Some(named) => Harness::read(&named).ok_or(ConfigError::NotAHarness(named))?,
                    None => Harness::DEFAULT,
                },
            }),
            None if settings.patrols.is_some() => return Err(ConfigError::NeedsTeam("patrols")),
            None if settings.intake_limit.is_some() => {
                return Err(ConfigError::NeedsTeam("intake_limit"));
            }
            None if settings.harness.is_some() => return Err(ConfigError::NeedsTeam("harness")),
            None => None,
        };
        match checked(
            base_branch,
            gate_command,
            ready_label,
            epic_labels,
            settings.concurrency.unwrap_or(3),
            escalate_label,
            lock_label,
            bound,
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
#[spec(fn(String, String, String, Vec<String>, concurrency: usize, String, String, Option<Bound>)
    -> Option<Config[concurrency]>)]
#[allow(clippy::too_many_arguments)]
fn checked(
    base_branch: String,
    gate_command: String,
    ready_label: String,
    epic_labels: Vec<String>,
    concurrency: usize,
    escalate_label: String,
    lock_label: String,
    bound: Option<Bound>,
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
        lock_label,
        bound,
    })
}

/// A repo's name as GitHub names it: `owner/repo`, each part of letters,
/// digits, `.`, `_` and `-`. The one home of the form: a team and a role are
/// both named so, and `read` is the only way to one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RepoName(String);

impl RepoName {
    /// `text` as a repo's name; `None` when it has not the form.
    pub fn read(text: &str) -> Option<RepoName> {
        let part = |part: &str| {
            !part.is_empty()
                && !part.starts_with('-')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        };
        match text.split_once('/') {
            Some((owner, name)) if part(owner) && part(name) => Some(RepoName(text.to_string())),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RepoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn repo(key: &'static str, value: String) -> Result<RepoName, ConfigError> {
    RepoName::read(&value).ok_or(ConfigError::NotARepo(key))
}

fn required(key: &'static str, value: Option<String>) -> Result<String, ConfigError> {
    match value {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(ConfigError::Required(key)),
    }
}

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
        assert_eq!(config.lock_label, "fleet-lock", "a fleet lock needs no team");
        assert_eq!(config.bound, None, "a repo that names no team is unbound");
    }

    #[test]
    fn a_repo_that_names_a_team_is_bound_with_defaults_filled() {
        let named = Settings { team: Some("acme/team".into()), ..minimal() };
        let bound = Config::new(named).unwrap().bound.unwrap();
        assert_eq!(bound.team.as_str(), "acme/team");
        assert_eq!(bound.patrols, Vec::<String>::new());
        assert_eq!(bound.intake_limit, 50);
        assert_eq!(
            bound.harness,
            Harness::ClaudeCode,
            "a config that names none means Claude Code"
        );
        let set = Settings {
            team: Some("acme/team".into()),
            patrols: Some(vec!["triage".into()]),
            intake_limit: Some(7),
            ..minimal()
        };
        let bound = Config::new(set).unwrap().bound.unwrap();
        assert_eq!((bound.patrols, bound.intake_limit), (vec!["triage".to_string()], 7));
    }

    #[test]
    fn a_team_is_named_as_github_names_a_repo() {
        for team in ["team", "acme/", "/team", "acme/team/x", "acme/te am", "-x/team", ""] {
            let named = Settings { team: Some(team.into()), ..minimal() };
            assert_eq!(Config::new(named), Err(ConfigError::NotARepo("team")), "{team:?}");
        }
        assert!(RepoName::read("acme-inc/team.roles_2").is_some());
    }

    #[test]
    fn a_key_of_a_bound_repo_is_refused_where_no_team_is_named() {
        let patrols = Settings { patrols: Some(vec![]), ..minimal() };
        assert_eq!(Config::new(patrols), Err(ConfigError::NeedsTeam("patrols")));
        let limit = Settings { intake_limit: Some(1), ..minimal() };
        assert_eq!(Config::new(limit), Err(ConfigError::NeedsTeam("intake_limit")));
        let harness = Settings { harness: Some("claude-code".into()), ..minimal() };
        assert_eq!(Config::new(harness), Err(ConfigError::NeedsTeam("harness")));
    }

    #[test]
    fn a_bound_repo_names_the_default_harness_by_its_key() {
        let named = |harness: &str| Settings {
            team: Some("acme/team".into()),
            harness: Some(harness.into()),
            ..minimal()
        };
        let bound = Config::new(named("claude-code")).unwrap().bound.unwrap();
        assert_eq!(bound.harness, Harness::ClaudeCode);
        for none in ["pi", "Claude Code", "claude", ""] {
            let refused = Config::new(named(none)).unwrap_err();
            assert_eq!(refused, ConfigError::NotAHarness(none.to_string()));
            let said = refused.to_string();
            assert!(said.starts_with("harness: ") && said.contains("claude-code"), "{said}");
        }
    }

    #[test]
    fn the_lock_label_is_read_with_a_team_and_without_one() {
        let unbound = Settings { lock_label: Some("stop".into()), ..minimal() };
        assert_eq!(Config::new(unbound).unwrap().lock_label, "stop");
        let bound =
            Settings { team: Some("a/b".into()), lock_label: Some("stop".into()), ..minimal() };
        assert_eq!(Config::new(bound).unwrap().lock_label, "stop");
        let empty = Settings { lock_label: Some(" ".into()), ..minimal() };
        assert_eq!(Config::new(empty), Err(ConfigError::Empty("lock_label")));
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

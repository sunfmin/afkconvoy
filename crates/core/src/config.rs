//! A repo's config: the keys of `docs/agents/afkconvoy.md` (docs/adr/0001), and
//! whether they bind the repo to a team (docs/adr/0008).

use std::num::NonZeroUsize;

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

/// The escalate label where nothing names one: a repo's, and a team's.
pub const ESCALATE_LABEL: &str = "ready-for-human";

/// The lock label where a repo names none.
pub const LOCK_LABEL: &str = "fleet-lock";

/// The epic labels where a repo names none.
pub const EPIC_LABELS: [&str; 2] = ["epic", "prd"];

/// The labels of a repo a team's workflows are read against: what no flow
/// label may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLabels {
    pub lock: String,
    pub epics: Vec<String>,
}

/// A repo whose config names neither.
impl Default for RepoLabels {
    fn default() -> RepoLabels {
        RepoLabels { lock: LOCK_LABEL.to_string(), epics: EPIC_LABELS.map(String::from).to_vec() }
    }
}

/// Legacy `escalate_to` values, accepted so existing configs still load.
/// Both now escalate for the host's watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EscalateTo {
    /// Retired consult routing; resolves to `Human`.
    Consult,
    /// Straight to a human.
    Human,
}

impl EscalateTo {
    pub const ALL: [EscalateTo; 2] = [EscalateTo::Consult, EscalateTo::Human];

    pub fn word(self) -> &'static str {
        match self {
            EscalateTo::Consult => "consult",
            EscalateTo::Human => "human",
        }
    }
}

/// How many claims a fleet instance holds at work at once where its checkout
/// says none on its host.
pub const CONCURRENCY: usize = 3;

/// The gate limit of a repo that declares none, in seconds: an hour.
pub const GATE_SECONDS: u64 = 3600;

/// The most open issues a bound repo's workers take in, where it declares no
/// `intake_limit`.
pub const INTAKE_LIMIT: u64 = 50;

/// The keys as a repo wrote them: any may be left out, none is yet checked.
/// A key that is not one of these is refused where the file is read.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub base_branch: Option<String>,
    pub gate_command: Option<String>,
    pub gate_seconds: Option<u64>,
    pub gate_budget_seconds: Option<u64>,
    pub ready_label: Option<String>,
    pub epic_labels: Option<Vec<String>>,
    /// Read only to be said: a checkout's concurrency is said on its host,
    /// and a config's changes nothing (docs/adr/0025). `doctor` says the line
    /// that keeps it on the host.
    pub concurrency: Option<NonZeroUsize>,
    pub escalate_label: Option<String>,
    pub team: Option<String>,
    pub patrols: Option<Vec<String>>,
    pub lock_label: Option<String>,
    pub intake_limit: Option<u64>,
    /// Read only to be refused: a config names no harness (docs/adr/0020).
    pub harness: Option<String>,
    pub escalate_to: Option<String>,
}

/// What a bound repo's config says beside the keys every repo has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bound {
    /// The team's directory: its repo, or a subdirectory of one.
    pub team: RepoDir,
    /// Which of the team's patrols run on this repo.
    pub patrols: Vec<String>,
    /// How many issues taken in by workers may be open at once.
    pub intake_limit: u64,
}

/// Who says how many claims a fleet instance holds at work at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SaidBy {
    /// Its checkout, on the host it is worked from (`Config::laid`).
    Checkout,
    /// Nobody: `CONCURRENCY`.
    Default,
}

/// A config a fleet can run on: every key present, defaults filled.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[refined_by(concurrency: int, gate_seconds: int)]
#[invariant(concurrency > 0)]
#[invariant(gate_seconds > 0)]
pub struct Config {
    pub base_branch: String,
    pub gate_command: String,
    /// The gate limit, in seconds: the repo's, or `GATE_SECONDS`.
    #[field(u64[gate_seconds])]
    gate_seconds: u64,
    /// Fixed duration budget for slow-gate warnings; never ends a run.
    /// Absent: warn at `slow_gate::PERCENT` percent of the gate limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_budget_seconds: Option<u64>,
    /// Whether the repo declared its gate limit: `gate_seconds` is in its config.
    #[serde(skip)]
    pub gate_limit_declared: bool,
    pub ready_label: String,
    pub epic_labels: Vec<String>,
    /// The checkout's, on the host it is worked from: never the repo's, so
    /// never in what `afkconvoy config` answers.
    #[field(usize[concurrency])]
    #[serde(skip)]
    concurrency: usize,
    /// Who says `concurrency`.
    #[serde(skip)]
    concurrency_said_by: SaidBy,
    /// The `concurrency` the repo's config still names, which changes
    /// nothing: what `doctor` says to keep on the host.
    #[serde(skip)]
    pub retired_concurrency: Option<NonZeroUsize>,
    pub escalate_label: String,
    /// An open issue carrying this is a fleet lock, in a bound repo and an
    /// unbound one alike.
    pub lock_label: String,
    /// Where an unbound repo's hand-off goes first. `None` in a bound repo,
    /// whose stages say it each.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub escalate_to: Option<EscalateTo>,
    /// `None` in an unbound repo, whose config is the keys above and nothing else.
    #[serde(flatten)]
    pub bound: Option<Bound>,
    /// The repo's cut steps, which turn cutting on (docs/adr/0028): read off
    /// a block of their own (`cut::BLOCK`), never the config's, and laid over
    /// it (`Config::cutting`). `None` where the file has no such block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut: Option<crate::cut::Steps>,
    /// The repo's gate split in two (docs/adr/0047): read off a block of
    /// their own (`GATES_BLOCK`), and laid over it (`Config::split`). `None`
    /// where the file has no such block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gates: Option<Gates>,
    /// Commands a host must pass before it can gate this repo.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gate_needs: Vec<GateNeed>,
    /// Commands a host must pass before claiming a cut.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cut_needs: Vec<GateNeed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateNeed {
    pub name: String,
    pub command: String,
    pub seconds: u64,
    /// What installs what the check checks, where it fails: run, then the
    /// check again. None refuses a failed check as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
    /// The install's limit, required with `install`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_seconds: Option<u64>,
}

/// The opening line of the block that splits a repo's gate in two: one of
/// its own, which an older afkconvoy skips, so the version that reads it can
/// still be landed and cut by the one before.
pub const GATES_BLOCK: &str = "```yaml gates";

/// The fast gate and the slow gate: the repo's gate as two, each checking
/// what the other does not, and together what `gate_command` checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gates {
    /// What a worker passes before it opens or updates its PR.
    pub fast: Gate,
    /// Everything else the gate checks.
    pub slow: Gate,
}

/// One of the two gates: what it runs, its limit and its warning budget,
/// as `gate_command`, `gate_seconds` and `gate_budget_seconds` are the
/// whole gate's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub command: String,
    /// The limit: a run still going after it is ended, and red.
    pub seconds: u64,
    /// The warning budget: a run past it is said to be slow, and is not red.
    pub budget_seconds: u64,
}

impl Gates {
    /// These gates, or why they cannot be run on, naming the key at fault.
    /// Neither may name nothing to run, have no time, or a budget not below
    /// its limit; together they are the gate split, so their limits may not
    /// add up past `gate_seconds`, nor their budgets past
    /// `gate_budget_seconds`, where it is declared.
    pub fn checked(
        self,
        gate_seconds: u64,
        gate_budget_seconds: Option<u64>,
    ) -> Result<Gates, String> {
        for (name, gate) in [("fast", &self.fast), ("slow", &self.slow)] {
            if gate.command.trim().is_empty() {
                return Err(format!("{name}.command: names nothing to run"));
            }
            if gate.seconds == 0 {
                return Err(format!(
                    "{name}.seconds: must be a whole number of seconds, at least 1"
                ));
            }
            if gate.budget_seconds == 0 || gate.budget_seconds >= gate.seconds {
                return Err(format!(
                    "{name}.budget_seconds: must be a whole number of seconds, at least 1 and below {name}.seconds"
                ));
            }
        }
        if self.fast.seconds.saturating_add(self.slow.seconds) > gate_seconds {
            return Err(format!(
                "fast.seconds and slow.seconds: together {}s, past gate_seconds, {gate_seconds}s: the two gates are the gate split, and neither limit is raised by it",
                self.fast.seconds.saturating_add(self.slow.seconds)
            ));
        }
        let budgets = self.fast.budget_seconds.saturating_add(self.slow.budget_seconds);
        if let Some(budget) = gate_budget_seconds.filter(|budget| budgets > *budget) {
            return Err(format!(
                "fast.budget_seconds and slow.budget_seconds: together {budgets}s, past gate_budget_seconds, {budget}s: the two gates are the gate split, and neither budget is raised by it"
            ));
        }
        Ok(self)
    }
}

/// Why a repo's settings are no config. Each names the key at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A key with no default was left out, or set to nothing.
    Required(&'static str),
    /// A label key set to the empty string.
    Empty(&'static str),
    /// `gate_seconds` is 0: a gate given no time at all.
    ZeroGateSeconds,
    /// A warning budget must be positive and below the termination limit.
    InvalidGateBudget,
    /// `escalate_label` is `ready_label`: escalating would leave the issue dispatchable.
    EscalatesToReady,
    /// `team` is not `owner/repo` or `owner/repo/path`.
    NotARepo(&'static str),
    /// A key that means something only in a bound repo, in a repo naming no team.
    NeedsTeam(&'static str),
    /// `harness`, which a config names no more: what it was set to.
    Harness(String),
    /// A key that means something only in an unbound repo, in a repo naming a team.
    Unbound(&'static str),
    /// `escalate_to` is neither `consult` nor `human`: what it was set to.
    NotAnEscalateTo(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Required(key) => write!(f, "{key}: required, and it has no default"),
            ConfigError::Empty(key) => write!(f, "{key}: must not be empty"),
            ConfigError::ZeroGateSeconds => {
                write!(f, "gate_seconds: must be a whole number of seconds, at least 1")
            }
            ConfigError::InvalidGateBudget => write!(
                f,
                "gate_budget_seconds: must be a whole number of seconds, at least 1 and below gate_seconds"
            ),
            ConfigError::EscalatesToReady => {
                write!(f, "escalate_label: must differ from ready_label")
            }
            ConfigError::NotARepo(key) => {
                write!(
                    f,
                    "{key}: must be a repo or a directory of one, as owner/repo or owner/repo/path"
                )
            }
            ConfigError::NeedsTeam(key) => {
                write!(f, "{key}: is read only in a bound repo, and this one names no team")
            }
            ConfigError::Harness(named) => write!(
                f,
                "harness: a config names no harness any more (docs/adr/0020): delete the line \
                 `harness: {named}`. A team's `roster` says which harnesses and models each role's \
                 workers may run, and the host kit (`afkconvoy kit`) which this host can start. \
                 A checkout names one with `afkconvoy run <checkout> --harness`"
            ),
            ConfigError::Unbound(key) => write!(
                f,
                "{key}: is read only in an unbound repo: a bound repo's stage says where its \
                 hand-off goes, `escalate: {{ to: human }}` straight to a human"
            ),
            ConfigError::NotAnEscalateTo(named) => {
                write!(f, "escalate_to: must be consult or human, not {named:?}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// The config `settings` asks for, defaults filled — or the first key that
    /// is missing or cannot be run on.
    pub fn new(settings: Settings) -> Result<Config, ConfigError> {
        if let Some(named) = settings.harness {
            return Err(ConfigError::Harness(named));
        }
        let base_branch = required("base_branch", settings.base_branch)?;
        if settings.gate_budget_seconds.is_some_and(|budget| {
            budget == 0 || budget >= settings.gate_seconds.unwrap_or(GATE_SECONDS)
        }) {
            return Err(ConfigError::InvalidGateBudget);
        }
        let gate_command = required("gate_command", settings.gate_command)?;
        let ready_label = label("ready_label", settings.ready_label, "ready-for-agent")?;
        let escalate_label = label("escalate_label", settings.escalate_label, ESCALATE_LABEL)?;
        if escalate_label == ready_label {
            return Err(ConfigError::EscalatesToReady);
        }
        let lock_label = label("lock_label", settings.lock_label, LOCK_LABEL)?;
        let epic_labels =
            settings.epic_labels.unwrap_or_else(|| EPIC_LABELS.map(String::from).to_vec());
        let escalate_to = match (&settings.team, settings.escalate_to.as_deref()) {
            (Some(_), Some(_)) => return Err(ConfigError::Unbound("escalate_to")),
            (Some(_), None) => None,
            (None, None) => Some(EscalateTo::Human),
            (None, Some(named)) => {
                match EscalateTo::ALL.into_iter().find(|to| to.word() == named) {
                    Some(_) => Some(EscalateTo::Human),
                    None => return Err(ConfigError::NotAnEscalateTo(named.to_string())),
                }
            }
        };
        let bound = match settings.team {
            Some(team) => Some(Bound {
                team: repo("team", team)?,
                patrols: settings.patrols.unwrap_or_default(),
                intake_limit: settings.intake_limit.unwrap_or(INTAKE_LIMIT),
            }),
            None if settings.patrols.is_some() => return Err(ConfigError::NeedsTeam("patrols")),
            None if settings.intake_limit.is_some() => {
                return Err(ConfigError::NeedsTeam("intake_limit"));
            }
            None => None,
        };
        match checked(
            base_branch,
            gate_command,
            settings.gate_seconds.unwrap_or(GATE_SECONDS),
            settings.gate_seconds.is_some(),
            ready_label,
            epic_labels,
            CONCURRENCY,
            settings.concurrency,
            escalate_label,
            lock_label,
            escalate_to,
            bound,
        ) {
            Some(config) => {
                Ok(Config { gate_budget_seconds: settings.gate_budget_seconds, ..config })
            }
            None => Err(ConfigError::ZeroGateSeconds),
        }
    }

    /// The labels a team's workflows are read against in this repo.
    pub fn labels(&self) -> RepoLabels {
        RepoLabels { lock: self.lock_label.clone(), epics: self.epic_labels.clone() }
    }

    /// How many claims the fleet holds at once. Never zero.
    #[spec(fn(&Config[@c]) -> usize[c.concurrency])]
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// Who says `concurrency`: the checkout, or nobody.
    pub fn concurrency_said_by(&self) -> SaidBy {
        self.concurrency_said_by
    }

    /// This config with `concurrency`, the one its checkout says on the host
    /// it is worked from, laid over the default. A concurrency of no slot at
    /// all is none, and leaves it as it is.
    #[spec(fn(Config[@c], concurrency: usize) -> Config)]
    pub fn laid(self, concurrency: usize) -> Config {
        if concurrency == 0 {
            return self;
        }
        Config { concurrency, concurrency_said_by: SaidBy::Checkout, ..self }
    }

    /// This config with `steps`, the cut steps its file says in a block of
    /// their own: what turns cutting on.
    pub fn cutting(self, steps: Option<crate::cut::Steps>) -> Config {
        Config { cut: steps, ..self }
    }

    /// This config with `gates`, the fast and slow gate its file says in a
    /// block of their own.
    pub fn split(self, gates: Option<Gates>) -> Config {
        Config { gates, ..self }
    }

    /// The gate `which` names: the fast one or the slow one, where the
    /// repo declares them.
    pub fn gate(&self, which: Which) -> Option<&Gate> {
        self.gates.as_ref().map(|gates| match which {
            Which::Fast => &gates.fast,
            Which::Slow => &gates.slow,
        })
    }

    /// The gate limit, in seconds: how long a run of the gate command is
    /// given before it is ended, and red. Never zero.
    #[spec(fn(&Config[@c]) -> u64[c.gate_seconds])]
    pub fn gate_seconds(&self) -> u64 {
        self.gate_seconds
    }
}

/// One of the two gates a repo's gate is split into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Fast,
    Slow,
}

impl Which {
    pub fn word(self) -> &'static str {
        match self {
            Which::Fast => "fast",
            Which::Slow => "slow",
        }
    }
}

/// The one place a `Config` is built: a zero concurrency never gets in, nor
/// a gate limit of no seconds.
#[spec(fn(String, String, gate_seconds: u64, bool, String, Vec<String>,
        concurrency: usize{concurrency > 0}, Option<NonZeroUsize>, String, String,
        Option<EscalateTo>, Option<Bound>)
    -> Option<Config[concurrency, gate_seconds]>)]
#[allow(clippy::too_many_arguments)]
fn checked(
    base_branch: String,
    gate_command: String,
    gate_seconds: u64,
    gate_limit_declared: bool,
    ready_label: String,
    epic_labels: Vec<String>,
    concurrency: usize,
    retired_concurrency: Option<NonZeroUsize>,
    escalate_label: String,
    lock_label: String,
    escalate_to: Option<EscalateTo>,
    bound: Option<Bound>,
) -> Option<Config> {
    if gate_seconds == 0 {
        return None;
    }
    Some(Config {
        base_branch,
        gate_command,
        gate_seconds,
        gate_budget_seconds: None,
        gate_limit_declared,
        ready_label,
        epic_labels,
        concurrency,
        concurrency_said_by: SaidBy::Default,
        retired_concurrency,
        escalate_label,
        lock_label,
        escalate_to,
        bound,
        cut: None,
        gates: None,
        gate_needs: Vec::new(),
        cut_needs: Vec::new(),
    })
}

/// A repo's name as GitHub names it: `owner/repo`, each part of letters,
/// digits, `.`, `_` and `-`, and neither `.` nor `..`. The one home of the form: the repo of a team,
/// of a role and of a tracker is named so, and `read` is the only way to one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct RepoName(String);

impl RepoName {
    /// `text` as a repo's name; `None` when it has not the form.
    pub fn read(text: &str) -> Option<RepoName> {
        let part = |part: &str| {
            !part.is_empty()
                && !part.starts_with('-')
                && part != "."
                && part != ".."
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

impl TryFrom<String> for RepoName {
    type Error = String;

    fn try_from(text: String) -> Result<RepoName, String> {
        RepoName::read(&text).ok_or_else(|| format!("`{text}` is no repo's name, owner/repo"))
    }
}

impl From<RepoName> for String {
    fn from(name: RepoName) -> String {
        name.0
    }
}

impl std::fmt::Display for RepoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A directory of a repo: the whole of it, or a subdirectory (docs/adr/0014).
/// A team is one and so is a role. It is written `owner/repo` or
/// `owner/repo/path`: the first two segments are the repo, the rest the path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "String")]
pub struct RepoDir {
    pub repo: RepoName,
    /// From the repo's root, its segments joined by `/` with none before or
    /// after: empty for the whole repo. No segment is `.` or `..`.
    path: String,
}

/// Why a text names no directory of a repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotADir {
    /// Its first two segments are no `owner/repo`.
    NoRepo,
    /// Its path starts at the root of a filesystem, not of a repo.
    Absolute,
    /// A segment of its path is `..`: it leaves the repo, or says a
    /// directory the long way round.
    Leaves,
}

impl RepoDir {
    /// The whole of `repo`.
    pub fn whole(repo: RepoName) -> RepoDir {
        RepoDir { repo, path: String::new() }
    }

    /// The directory at `path` of `repo`, read from the repo's root: an
    /// empty segment and a `.` say nothing, so `roles/fellow/` and
    /// `./roles/fellow` are one directory.
    pub fn at(repo: RepoName, path: &str) -> Result<RepoDir, NotADir> {
        if path.starts_with('/') {
            return Err(NotADir::Absolute);
        }
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
        if segments.contains(&"..") {
            return Err(NotADir::Leaves);
        }
        Ok(RepoDir { repo, path: segments.join("/") })
    }

    /// `text` as `owner/repo` or `owner/repo/path`, or why it is neither.
    pub fn read(text: &str) -> Result<RepoDir, NotADir> {
        let mut parts = text.splitn(3, '/');
        let (owner, name) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let repo = RepoName::read(&format!("{owner}/{name}")).ok_or(NotADir::NoRepo)?;
        RepoDir::at(repo, parts.next().unwrap_or(""))
    }

    /// Its path from the root of its repo: empty for the whole repo.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether `other` is this directory or one under it, in the same repo.
    pub fn holds(&self, other: &RepoDir) -> bool {
        self.repo == other.repo
            && (self.path.is_empty()
                || other.path == self.path
                || other.path.strip_prefix(&self.path).is_some_and(|rest| rest.starts_with('/')))
    }

    /// The directory `under` this one: `under` is a path written from here.
    pub fn join(&self, under: &str) -> RepoDir {
        let path =
            if self.path.is_empty() { under.to_string() } else { format!("{}/{under}", self.path) };
        RepoDir { repo: self.repo.clone(), path }
    }
}

impl std::fmt::Display for RepoDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.repo)
        } else {
            write!(f, "{}/{}", self.repo, self.path)
        }
    }
}

impl From<RepoDir> for String {
    fn from(dir: RepoDir) -> String {
        dir.to_string()
    }
}

fn repo(key: &'static str, value: String) -> Result<RepoDir, ConfigError> {
    RepoDir::read(&value).map_err(|_| ConfigError::NotARepo(key))
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
        assert_eq!((config.concurrency(), config.concurrency_said_by()), (3, SaidBy::Default));
        assert_eq!((config.gate_seconds(), config.gate_limit_declared), (3600, false));
        assert_eq!(config.lock_label, "fleet-lock", "a fleet lock needs no team");
        assert_eq!(config.bound, None, "a repo that names no team is unbound");
        assert_eq!(config.escalate_to, Some(EscalateTo::Human));
    }

    #[test]
    fn an_unbound_repo_sends_every_hand_off_to_a_human_with_one_key() {
        let human = Settings { escalate_to: Some("human".into()), ..minimal() };
        assert_eq!(Config::new(human).unwrap().escalate_to, Some(EscalateTo::Human));
        let consult = Settings { escalate_to: Some("consult".into()), ..minimal() };
        assert_eq!(Config::new(consult).unwrap().escalate_to, Some(EscalateTo::Human));
        let other = Settings { escalate_to: Some("boss".into()), ..minimal() };
        assert_eq!(Config::new(other), Err(ConfigError::NotAnEscalateTo("boss".into())));
        // A bound repo's stages say it each.
        let bound = Settings {
            team: Some("acme/team".into()),
            escalate_to: Some("human".into()),
            ..minimal()
        };
        assert_eq!(Config::new(bound), Err(ConfigError::Unbound("escalate_to")));
    }

    #[test]
    fn a_checkouts_concurrency_is_laid_over_the_default() {
        let laid = Config::new(minimal()).unwrap().laid(8);
        assert_eq!((laid.concurrency(), laid.concurrency_said_by()), (8, SaidBy::Checkout));
        // No slot at all is none: the default stands.
        let none = Config::new(minimal()).unwrap().laid(0);
        assert_eq!((none.concurrency(), none.concurrency_said_by()), (3, SaidBy::Default));
        // What `afkconvoy config` answers is the repo's alone, and the
        // concurrency is no fact of the repo's.
        let said = serde_json::to_value(&laid).unwrap();
        assert!(said.get("concurrency").is_none(), "{said}");
        assert!(said.get("concurrency_said_by").is_none(), "{said}");
    }

    #[test]
    fn a_config_that_names_a_concurrency_is_read_and_it_changes_nothing() {
        let named = Config::new(Settings { concurrency: NonZeroUsize::new(5), ..minimal() });
        let named = named.unwrap();
        assert_eq!((named.concurrency(), named.concurrency_said_by()), (3, SaidBy::Default));
        assert_eq!(named.retired_concurrency, NonZeroUsize::new(5));
        let laid = named.laid(6);
        assert_eq!((laid.concurrency(), laid.retired_concurrency), (6, NonZeroUsize::new(5)));
        assert_eq!(Config::new(minimal()).unwrap().retired_concurrency, None);
        let said = serde_json::to_value(&laid).unwrap();
        assert!(said.get("retired_concurrency").is_none(), "{said}");
    }

    #[test]
    fn a_repo_declares_its_gate_limit() {
        let declared = Settings { gate_seconds: Some(600), ..minimal() };
        let config = Config::new(declared).unwrap();
        assert_eq!((config.gate_seconds(), config.gate_limit_declared), (600, true));
        // Declaring the default is declaring it.
        let hour = Config::new(Settings { gate_seconds: Some(3600), ..minimal() }).unwrap();
        assert!(hour.gate_limit_declared);
    }

    #[test]
    fn a_warning_budget_is_fixed_and_below_the_termination_limit() {
        let config = Config::new(Settings {
            gate_seconds: Some(3600),
            gate_budget_seconds: Some(600),
            ..minimal()
        })
        .unwrap();
        assert_eq!(config.gate_budget_seconds, Some(600));
        assert_eq!(config.gate_seconds(), 3600);
        assert_eq!(Config::new(minimal()).unwrap().gate_budget_seconds, None);
        for budget in [0, 3600, u64::MAX] {
            assert_eq!(
                Config::new(Settings { gate_budget_seconds: Some(budget), ..minimal() }),
                Err(ConfigError::InvalidGateBudget)
            );
        }
    }

    #[test]
    fn a_repo_that_names_a_team_is_bound_with_defaults_filled() {
        let named = Settings { team: Some("acme/team".into()), ..minimal() };
        let bound = Config::new(named).unwrap().bound.unwrap();
        assert_eq!(bound.team.to_string(), "acme/team");
        assert_eq!(bound.patrols, Vec::<String>::new());
        assert_eq!(bound.intake_limit, 50);
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
        for team in [
            "team",
            "acme/",
            "/team",
            "acme/te am",
            "-x/team",
            "",
            "acme/team/../x",
            "acme/team//x",
        ] {
            let named = Settings { team: Some(team.into()), ..minimal() };
            assert_eq!(Config::new(named), Err(ConfigError::NotARepo("team")), "{team:?}");
        }
        assert!(RepoName::read("acme-inc/team.roles_2").is_some());
    }

    #[test]
    fn a_team_is_a_repo_or_a_directory_of_one() {
        let team = |named: &str| {
            let named = Settings { team: Some(named.into()), ..minimal() };
            Config::new(named).unwrap().bound.unwrap().team
        };
        let whole = team("acme/team");
        assert_eq!((whole.repo.as_str(), whole.path()), ("acme/team", ""));
        // The first two segments are the repo, the rest the path.
        let under = team("acme/co/teams/hospital/");
        assert_eq!((under.repo.as_str(), under.path()), ("acme/co", "teams/hospital"));
        assert_eq!(under.to_string(), "acme/co/teams/hospital");
    }

    #[test]
    fn a_directory_holds_itself_and_what_is_under_it_in_its_repo() {
        let dir = |text: &str| RepoDir::read(text).unwrap();
        assert!(dir("a/b").holds(&dir("a/b/roles/x")) && dir("a/b/roles").holds(&dir("a/b/roles")));
        assert!(dir("a/b/roles").holds(&dir("a/b/roles/x")));
        assert!(!dir("a/b/roles").holds(&dir("a/b/roles-old")));
        assert!(!dir("a/b/roles/x").holds(&dir("a/b/roles")) && !dir("a/b").holds(&dir("a/c")));
        let repo = || RepoName::read("a/b").unwrap();
        assert_eq!(RepoDir::at(repo(), "./roles//x/"), Ok(dir("a/b/roles/x")));
        assert_eq!(RepoDir::at(repo(), "/roles"), Err(NotADir::Absolute));
        assert_eq!(RepoDir::at(repo(), "roles/../.."), Err(NotADir::Leaves));
        assert_eq!(RepoDir::read("a/b//roles"), Err(NotADir::Absolute));
        assert_eq!(RepoDir::read("roles"), Err(NotADir::NoRepo));
    }

    #[test]
    fn a_key_of_a_bound_repo_is_refused_where_no_team_is_named() {
        let patrols = Settings { patrols: Some(vec![]), ..minimal() };
        assert_eq!(Config::new(patrols), Err(ConfigError::NeedsTeam("patrols")));
        let limit = Settings { intake_limit: Some(1), ..minimal() };
        assert_eq!(Config::new(limit), Err(ConfigError::NeedsTeam("intake_limit")));
    }

    #[test]
    fn a_config_that_names_a_harness_is_refused_with_the_line_to_delete() {
        for team in [None, Some("acme/team".to_string())] {
            let named = Settings { team, harness: Some("pi".into()), ..minimal() };
            let refused = Config::new(named).unwrap_err();
            assert_eq!(refused, ConfigError::Harness("pi".to_string()));
            let said = refused.to_string();
            assert!(said.contains("delete the line `harness: pi`"), "{said}");
            assert!(said.contains("`roster`"), "{said}");
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
        for value in [None, Some("".to_string()), Some("  ".to_string())] {
            let base = Settings { base_branch: value.clone(), ..minimal() };
            assert_eq!(Config::new(base), Err(ConfigError::Required("base_branch")));
            let gate = Settings { gate_command: value, ..minimal() };
            assert_eq!(Config::new(gate), Err(ConfigError::Required("gate_command")));
        }
    }

    #[test]
    fn config_errors_name_the_field_to_change() {
        for (error, key) in [
            (ConfigError::EscalatesToReady, "escalate_label"),
            (ConfigError::NotARepo("team"), "team"),
            (ConfigError::NeedsTeam("patrols"), "patrols"),
            (ConfigError::NeedsTeam("intake_limit"), "intake_limit"),
        ] {
            assert!(error.to_string().contains(key), "{error:?} must name {key}");
        }
    }

    #[test]
    fn a_config_nothing_could_run_on_is_refused() {
        let no_time = Settings { gate_seconds: Some(0), ..minimal() };
        assert_eq!(Config::new(no_time), Err(ConfigError::ZeroGateSeconds));
        let same = Settings { escalate_label: Some("ready-for-agent".into()), ..minimal() };
        assert_eq!(Config::new(same), Err(ConfigError::EscalatesToReady));
        let empty = Settings { ready_label: Some(String::new()), ..minimal() };
        assert_eq!(Config::new(empty), Err(ConfigError::Empty("ready_label")));
    }
}

//! Bounded observation, operator fallback and continuation of stalled
//! workers. This supplies the evidence `afkconvoy_core::worker_health`
//! decides on and carries its decision out: it keeps no table of its own.
//! Health is host-local evidence, never another preference. Only kit order
//! chooses successors.
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::{Status, WorkingSet};
use afkconvoy_core::worker::WorkerState;
pub use afkconvoy_core::worker_health::{Action, Class, Scope};
use afkconvoy_core::worker_health::{Decision, Evidence};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::claims::Me;
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::kit::Kit;
use crate::orca::{Source, Worktree};

pub const OBSERVE_SECONDS: u64 = 300;
pub const CHECK_SECONDS: u64 = 300;
pub const MAX_RATE_RETRIES: u32 = 2;
pub const CAPACITY_WAIT_SECONDS: u64 = 60;
pub const MAX_CAPACITY_STOPS: u32 = 3;
pub const CAPACITY_HEALTHY_SECONDS: u64 = 300;
pub const MAX_CONTINUATIONS: u32 = 3;
pub const RECOVERY_SECONDS: u64 = 3600;
pub const MAX_QUOTA_WAIT_SECONDS: u64 = RECOVERY_SECONDS;

/// The decision on a stopped worker, as its book keeps it. Evidence quotes
/// an actual observation; a confidence number is deliberately not part of
/// it. The class is kept as its word, so a book an older version wrote with
/// a class no decision gives now is still read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnosis {
    pub status: String,
    pub class: String,
    pub scope: Scope,
    pub evidence: Vec<String>,
    pub action: Action,
    pub missing: Vec<String>,
    pub retry_after: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
struct Book {
    workers: Vec<Observed>,
    faults: Vec<Fault>,
    #[serde(default)]
    capacity_stops: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    observations_failed: Vec<FailedObservation>,
}

/// An observation of the worker in `path` that failed with `error`, retried
/// at `retry_at`. It is written as the triple `[path, error, retry_at]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(PathBuf, String, u64)", into = "(PathBuf, String, u64)")]
pub struct FailedObservation {
    pub path: PathBuf,
    pub error: String,
    pub retry_at: u64,
}
impl From<(PathBuf, String, u64)> for FailedObservation {
    fn from((path, error, retry_at): (PathBuf, String, u64)) -> Self {
        FailedObservation { path, error, retry_at }
    }
}
impl From<FailedObservation> for (PathBuf, String, u64) {
    fn from(failed: FailedObservation) -> Self {
        (failed.path, failed.error, failed.retry_at)
    }
}

/// A harness, or one of its models, left out of selection on its evidence
/// until its probe answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fault {
    #[serde(deserialize_with = "crate::kit::harness")]
    pub harness: Harness,
    #[serde(default)]
    command_key: Option<String>,
    pub model: Option<String>,
    pub scope: Scope,
    since: u64,
    pub next_check: u64,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    at: u64,
    source: String,
    output: Vec<String>,
    progress: String,
}

/// One session's capacity episode. A pending wait is counted once; only a
/// nudge whose receipt proves a new turn allows another stop to count.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
struct Capacity {
    stops: u32,
    retry_at: Option<u64>,
    healthy_since: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observed {
    pub path: PathBuf,
    pub terminal: String,
    #[serde(deserialize_with = "crate::kit::harness")]
    harness: Harness,
    #[serde(default)]
    pub recorded_harness: Option<String>,
    #[serde(default)]
    pub harness_mismatch: bool,
    /// Orca's record of the worker's terminal, as Orca answered it.
    #[serde(default)]
    pub orca: Value,
    #[serde(default)]
    command_key: Option<String>,
    model: Option<String>,
    snapshots: Vec<Snapshot>,
    pub next_check: u64,
    rate_retries: u32,
    #[serde(default)]
    capacity: Option<Capacity>,
    #[serde(default)]
    continuations: u32,
    #[serde(default)]
    failed_candidates: Vec<(String, u64)>,
    #[serde(default)]
    window_started: u64,
    pub diagnosis: Option<Diagnosis>,
    /// The latest check's request to Jev and TypeSafe's response.
    #[serde(default)]
    judgment: Option<Value>,
    #[serde(default)]
    pub operator_reason: Option<String>,
    pub source: String,
    pub action: String,
    pub successor: Option<String>,
    pub error: Option<String>,
}

/// A non-secret identity for the launch command/profile. Faults observed on
/// an old command cannot disable a replacement configured while it ran.
pub(crate) fn command_key(command: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    command.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

fn file() -> Result<PathBuf> {
    Ok(crate::copies::cache()?.join("worker-health.json"))
}
fn read() -> Result<Book> {
    match std::fs::read(file()?) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Book::default()),
        Err(e) => Err(e.into()),
    }
}
fn store(book: &Book) -> Result<()> {
    let file = file()?;
    std::fs::create_dir_all(file.parent().context("health cache parent")?)?;
    let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(book)?)?;
    Ok(std::fs::rename(temporary, file)?)
}

/// Worker health as every read-only surface reads it: each worker observed,
/// each fault, each failed observation and the bounds they are kept by. Its
/// JSON is only its serialisation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    pub workers: Vec<Worker>,
    pub faults: Vec<Fault>,
    pub observations_failed: Vec<FailedObservation>,
    observe_seconds: u64,
    check_seconds: u64,
    max_rate_retries: u32,
    capacity_stops: std::collections::BTreeMap<String, u64>,
    capacity_wait_seconds: u64,
    max_capacity_stops: u32,
    capacity_healthy_seconds: u64,
    max_quota_wait_seconds: u64,
}

/// One worker as worker health observed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worker {
    #[serde(flatten)]
    pub observed: Observed,
    /// Since when it has waited for background work, by its worktree's stamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<u64>,
}

/// A worker whose health check waits for the operator: why, the last line
/// its screen showed, and what fixes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorWait {
    pub worker: PathBuf,
    pub reason: String,
    pub screen_line: String,
    pub fix: String,
}

/// An observation past its time and past a tick's chance to make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overdue {
    /// The worker's next observation, due at `due`, is still waiting.
    Waiting { path: PathBuf, due: u64, error: Option<String> },
    /// The worker's observation failed with `error`, and its retry is overdue.
    Retry { path: PathBuf, error: String },
}

/// How long past its time an observation is left to a tick: the sleep and
/// the time of the last tick of the fleet instance whose claim holds its
/// worktree. A worktree no claim holds gets the longest any instance says;
/// with none said, there is no grace. A worktree whose claim a fleet lock
/// keeps its fleet off is left alone by its ticks, so its observation is
/// never overdue while the lock stands.
#[derive(Debug, Clone, Default)]
pub struct Cadence {
    of_worktree: std::collections::BTreeMap<PathBuf, u64>,
    longest: u64,
    left_alone: std::collections::BTreeSet<PathBuf>,
}
impl Cadence {
    /// Worktrees whose claims a standing fleet lock keeps their fleet off
    /// (`WorkingSet::locked_out`): its ticks observe none of them.
    pub fn left_alone(&mut self, worktrees: impl IntoIterator<Item = PathBuf>) {
        self.left_alone.extend(worktrees);
    }

    /// A fleet instance that sleeps `sleep` between ticks, whose last tick
    /// took `took`, holding claims on `worktrees`.
    pub fn instance(
        &mut self,
        sleep: u64,
        took: u64,
        worktrees: impl IntoIterator<Item = PathBuf>,
    ) {
        let cadence = sleep.saturating_add(took);
        self.longest = self.longest.max(cadence);
        for worktree in worktrees {
            self.of_worktree.insert(worktree, cadence);
        }
    }

    fn grace(&self, path: &std::path::Path) -> u64 {
        self.of_worktree.get(path).copied().unwrap_or(self.longest)
    }
}

impl Health {
    /// Every observation past its time by more than its tick's grace at
    /// `now`: the waiting workers first, then the failed observations.
    pub fn overdue(&self, cadence: &Cadence, now: u64) -> Vec<Overdue> {
        let missed = |path: &std::path::Path, due: u64| {
            !cadence.left_alone.contains(path) && due.saturating_add(cadence.grace(path)) <= now
        };
        let waiting = self
            .workers
            .iter()
            .map(|worker| &worker.observed)
            .filter(|worker| missed(&worker.path, worker.next_check));
        let failed =
            self.observations_failed.iter().filter(|failed| missed(&failed.path, failed.retry_at));
        waiting
            .map(|worker| Overdue::Waiting {
                path: worker.path.clone(),
                due: worker.next_check,
                error: worker.error.clone(),
            })
            .chain(failed.map(|failed| Overdue::Retry {
                path: failed.path.clone(),
                error: failed.error.clone(),
            }))
            .collect()
    }
}

impl Observed {
    /// Where its health check waits for the operator: quiet past the
    /// observation bound, and classed by no rule or settled judgment. With
    /// a Jev key command (`keyed`) the screen is the operator's to read;
    /// without one, the fix names one.
    fn operator_wait(&self, keyed: bool) -> Option<OperatorWait> {
        if self.action != "waiting for operator" {
            return None;
        }
        let screen_line = self
            .snapshots
            .last()
            .and_then(|snapshot| snapshot.output.iter().rev().find(|line| !line.trim().is_empty()))
            .cloned()
            .unwrap_or_default();
        let fix = if keyed {
            format!("orca terminal read --terminal {} --screen", self.terminal)
        } else {
            "afkconvoy run --jev-key-command '<command>'".into()
        };
        Some(OperatorWait {
            worker: self.path.clone(),
            reason: self.operator_reason.clone().unwrap_or_default(),
            screen_line,
            fix,
        })
    }
}

/// Worker health as it is, for every read-only surface; why it cannot be
/// read.
pub fn show() -> Result<Health, String> {
    let book = read().map_err(|e| format!("{e:#}"))?;
    let workers = book
        .workers
        .into_iter()
        .filter(|observed| !waiting_on_turn(&observed.path))
        .map(|observed| {
            // The since is the worktree's stamp, read here. The observation
            // keeps none: a copy cleared when the turn opened disagreed with it.
            let background = (observed.action == "waiting for background work")
                .then(|| Worktree::at(observed.path.clone(), String::new()).background_at())
                .flatten();
            Worker { observed, background }
        })
        .collect();
    Ok(Health {
        workers,
        faults: book.faults,
        observations_failed: book
            .observations_failed
            .into_iter()
            .filter(|failed| !waiting_on_turn(&failed.path))
            .collect(),
        observe_seconds: OBSERVE_SECONDS,
        check_seconds: CHECK_SECONDS,
        max_rate_retries: MAX_RATE_RETRIES,
        capacity_stops: book.capacity_stops,
        capacity_wait_seconds: CAPACITY_WAIT_SECONDS,
        max_capacity_stops: MAX_CAPACITY_STOPS,
        capacity_healthy_seconds: CAPACITY_HEALTHY_SECONDS,
        max_quota_wait_seconds: MAX_QUOTA_WAIT_SECONDS,
    })
}

/// What `show` answers, as JSON: {"error"} where worker health cannot be read.
pub fn json(shown: &Result<Health, String>) -> Value {
    match shown {
        Ok(health) => json!(health),
        Err(error) => json!({"error": error}),
    }
}

/// The workers whose health check is waiting for the operator, each with
/// the reason, the last line its screen showed and its fix, for `doctor`.
pub fn waiting_for_operator() -> Vec<OperatorWait> {
    let Ok(health) = show() else { return vec![] };
    let keyed =
        crate::host_settings::of_host().is_ok_and(|settings| settings.jev_key_command.is_some());
    health.workers.iter().filter_map(|worker| worker.observed.operator_wait(keyed)).collect()
}

/// Whether the landing worker at `path` is obeying its turn's wait.
fn waiting_on_turn(path: &Path) -> bool {
    let worktree = Worktree::at(path.to_path_buf(), String::new());
    crate::turn_answer::waiting(&worktree)
}

/// Whether the host kit's paused list or its worker-health faults leave no
/// harness a watch can start on.
pub fn no_harness_for_watch() -> bool {
    let paused = !crate::paused::read().is_empty();
    let faults = read().is_ok_and(|book| !book.faults.is_empty());
    if !paused && !faults {
        return false;
    }
    watch_candidates().map(|candidates| candidates.is_empty()).unwrap_or(true)
}

/// Removes only faults proved at account/model scope. A single stuck session
/// never pauses a harness for unrelated workers. Expiry alone is not recovery.
pub fn available(mut kit: Kit) -> Result<Kit> {
    for fault in read()?.faults {
        if let Some(held) = kit.harnesses.iter_mut().find(|h| {
            h.harness == fault.harness
                && fault.command_key.as_ref() == Some(&command_key(&h.command))
        }) && fault.scope == Scope::Model
        {
            held.models.retain(|m| Some(&m.model) != fault.model.as_ref());
        }
        // A harness with no placed model left runs its default: a model fault
        // there, or on the default itself, leaves the harness out.
        kit.harnesses.retain(|h| {
            !(h.harness == fault.harness
                && fault.command_key.as_ref() == Some(&command_key(&h.command))
                && (fault.scope == Scope::Account
                    || (fault.scope == Scope::Model && h.placed().next().is_none())))
        });
    }
    Ok(kit)
}

/// Why selection skips a harness with a recorded account or model fault.
pub fn fault_reason(harness: Harness) -> Option<String> {
    let kit = crate::kit::load().ok()?;
    let command_key = command_key(kit.command(harness)?);
    let fault = read().ok()?.faults.into_iter().find(|fault| {
        fault.harness == harness
            && fault.command_key.as_ref() == Some(&command_key)
            && fault.scope != Scope::Worker
    })?;
    let scope = match fault.scope {
        Scope::Account => "account",
        Scope::Model => "model",
        Scope::Worker => return None,
    };
    Some(format!("{} has an {scope} fault: {}", harness.key(), fault.evidence.join("; ")))
}

/// The harnesses whose workers took their briefs, most recent first.
fn delivered_file() -> Result<PathBuf> {
    Ok(crate::copies::cache()?.join("watch/delivered.json"))
}

fn delivered_read() -> Vec<String> {
    let bytes = delivered_file().and_then(|file| Ok(std::fs::read(file)?));
    bytes.ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Records that the worker in `worktree` took its brief: its harness is the
/// one a watch starts on first.
pub fn delivered(worktree: &Worktree) -> Result<()> {
    let Some(harness) = worktree.harness()? else { return Ok(()) };
    let mut delivered = delivered_read();
    delivered.retain(|held| held != harness.key());
    delivered.insert(0, harness.key().to_string());
    let file = delivered_file()?;
    std::fs::create_dir_all(file.parent().context("watch cache parent")?)?;
    let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec(&delivered)?)?;
    Ok(std::fs::rename(temporary, file)?)
}

/// The harnesses a watch may start on, in order: the one this host last
/// started a worker on that took its brief first, then the host kit's order;
/// none paused, and none with a fault on record, of any scope, until its
/// probe answers.
pub fn watch_candidates() -> Result<Vec<crate::kit::Named>> {
    let (kit, _) = crate::kit::running()?;
    let faults = read()?.faults;
    let faulted = |held: &crate::kit::Held| {
        faults.iter().any(|fault| {
            fault.harness == held.harness
                && fault.command_key.as_ref() == Some(&command_key(&held.command))
        })
    };
    let delivered = delivered_read();
    let delivered = delivered.iter().filter_map(|key| Harness::read(key));
    let mut candidates: Vec<crate::kit::Named> = vec![];
    for harness in delivered.chain(kit.harnesses.iter().map(|held| held.harness)) {
        if candidates.iter().any(|named| named.harness == harness) {
            continue;
        }
        if let Some(held) = kit.harnesses.iter().find(|held| held.harness == harness)
            && !faulted(held)
        {
            // A watch has no model label. It runs the harness's user default.
            candidates.push(crate::kit::Named { harness, model: None });
        }
    }
    Ok(candidates)
}

/// Records the fault a watch's session showed of `harness`, at the scope its
/// diagnosis proved: the watch skips it, and an account's fault every worker
/// too, until the hourly probe answers.
pub fn watch_fault(harness: Harness, diagnosis: &Diagnosis, now: u64) -> Result<()> {
    let file = file()?;
    std::fs::create_dir_all(file.parent().context("health cache parent")?)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(file.with_extension("lock"))?;
    lock.lock()?;
    let mut book = read()?;
    let command = crate::kit::load()?.command(harness).unwrap_or_default().to_string();
    let fault = Fault {
        harness,
        command_key: Some(command_key(&command)),
        model: None,
        scope: diagnosis.scope,
        since: now,
        next_check: now.saturating_add(RECOVERY_SECONDS),
        evidence: diagnosis.evidence.clone(),
    };
    book.faults.retain(|held| {
        !(held.harness == fault.harness
            && held.command_key == fault.command_key
            && held.model.is_none()
            && held.scope == fault.scope)
    });
    book.faults.push(fault);
    store(&book)
}

/// A fault the harness's own words in `output` prove, without another model.
pub fn terminal_fault(harness: Harness, output: &[String]) -> Option<Diagnosis> {
    terminal_rule(harness, output)
}

/// A watch uses the same structured errors and fixed harness messages as
/// an issue worker, with runtime evidence scoped to its terminal's pane.
pub(crate) fn watch_diagnosis(
    harness: Harness,
    output: &[String],
    runtime: &Value,
) -> Option<Diagnosis> {
    terminal_rule(harness, output).or_else(|| classified_for(Some(harness), runtime))
}

/// A session that exited with a failure: its harness failed it, and is held
/// to have failed that session alone.
pub fn failed(evidence: Vec<String>) -> Diagnosis {
    decided(Evidence::Exited(&evidence)).expect("an exit is always decided")
}

/// The latest delivery failure, kept beside the worker's brief.
pub(crate) const SIGNAL_FILE: &str = "convoy-worker-error.json";

/// A delivery failure is evidence for this worker first. A rule or Jev's
/// table determines its scope, never a count of arbitrary refused sends.
pub fn signal(worktree: &Worktree, reason: &str) -> Result<()> {
    let file = worktree.note(SIGNAL_FILE)?;
    std::fs::write(
        file,
        serde_json::to_vec(
            &json!({"at": crate::clock(), "source": "orca terminal send", "error": reason}),
        )?,
    )?;
    Ok(())
}

fn error_of(worktree: &Worktree) -> Result<String> {
    let file = worktree.note(SIGNAL_FILE)?;
    Ok(std::fs::read_to_string(file).unwrap_or_default())
}

/// A refused line is evidence of a stopped worker even if its last runtime
/// report still says working. A replacement clears it only once started.
pub fn has_signal(worktree: &Worktree) -> Result<bool> {
    Ok(!error_of(worktree)?.is_empty())
}

impl Diagnosis {
    /// Whether the decision was `class`.
    pub fn is(&self, class: Class) -> bool {
        self.class == class.key()
    }
}

impl From<Decision> for Diagnosis {
    fn from(decision: Decision) -> Diagnosis {
        Diagnosis {
            status: decision.status().into(),
            class: decision.class.key().into(),
            scope: decision.scope,
            evidence: decision.evidence,
            action: decision.action,
            missing: vec![],
            retry_after: decision.retry_after,
        }
    }
}

/// What the one decision makes of words, an error or an exit: none of them
/// is ever unsettled, only unrecognized.
fn decided(evidence: Evidence) -> Option<Diagnosis> {
    afkconvoy_core::worker_health::decide(evidence).ok().flatten().map(Diagnosis::from)
}

/// The harness's own words on its screen.
fn terminal_rule(harness: Harness, output: &[String]) -> Option<Diagnosis> {
    decided(Evidence::Words { harness, lines: output })
}

/// Conservative structured/error classification. Plain terminal text goes to
/// Jev, or the operator, unless the harness's own explicit words decide the
/// fault.
fn classified(value: &Value) -> Option<Diagnosis> {
    classified_for(None, value)
}

fn classified_for(harness: Option<Harness>, value: &Value) -> Option<Diagnosis> {
    decided(Evidence::Error { harness, error: value })
}

/// A tick's issue workers being observed, judged or held for the operator.
/// Legacy silent worker escalation cannot bypass this decision branch.
pub fn holding(worktree: &Worktree) -> bool {
    holds(worktree, &["healthy", "continued"])
}

/// `holding`, read by a report that runs no observation first: a recorded
/// wait on background work holds nothing, since the read of the screen the
/// judgment makes itself says whether that work still runs, and the record
/// is only as fresh as the last tick.
pub fn holding_past_background(worktree: &Worktree) -> bool {
    holds(worktree, &["healthy", "continued", "waiting for background work"])
}

fn holds(worktree: &Worktree, free: &[&str]) -> bool {
    if has_signal(worktree).unwrap_or(true) {
        return true;
    }
    read().map_or(true, |book| {
        book.observations_failed.iter().any(|failed| failed.path == worktree.path)
            || book
                .workers
                .iter()
                .any(|o| o.path == worktree.path && !free.contains(&o.action.as_str()))
    })
}

/// Recovery probes are scoped exactly like the evidence. A successful probe
/// re-enables future starts, never moves a healthy successor back.
fn probe_due(book: &mut Book, kit: &Kit, now: u64) {
    book.faults.retain_mut(|fault| {
        if now < fault.next_check {
            return true;
        }
        let Some(held) = kit.harnesses.iter().find(|h| {
            h.harness == fault.harness
                && fault.command_key.as_ref() == Some(&command_key(&h.command))
        }) else {
            return false;
        };
        fault.next_check = now.saturating_add(RECOVERY_SECONDS);
        let model =
            fault.model.as_deref().or_else(|| held.placed().next().map(|m| m.model.as_str()));
        !matches!(
            crate::launch::probe(held.harness, &held.command, model),
            crate::launch::Answer::Answered(..)
        )
    });
}

/// Observe this fleet's workers and retire evidence of ended workers or
/// landing workers obeying a turn's wait, across the host. An unreadable
/// runtime keeps its evidence. Account/model faults survive their workers.
pub fn reconcile(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    open: &crate::gh::Open,
    flows: &crate::stage::Flows,
    set: &WorkingSet,
) -> Result<Vec<Value>> {
    let file = file()?;
    std::fs::create_dir_all(file.parent().context("health cache parent")?)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(file.with_extension("lock"))?;
    if lock.try_lock().is_err() {
        return Ok(vec![]);
    }
    let mut book = read()?;
    // A caster has no claim to retire its record. Any fleet's next tick can
    // retire an ended worker, even after its worktree was removed. Check
    // failure-only entries too, and keep evidence when Orca cannot answer.
    let paths: std::collections::BTreeSet<_> = book
        .workers
        .iter()
        .map(|worker| worker.path.clone())
        .chain(book.observations_failed.iter().map(|failed| failed.path.clone()))
        .collect();
    let retired: std::collections::BTreeSet<_> = paths
        .into_iter()
        .filter(|path| {
            waiting_on_turn(path)
                || Worktree::at(path.clone(), String::new())
                    .state(fleet.now)
                    .is_ok_and(|state| state == WorkerState::Gone)
        })
        .collect();
    book.workers.retain(|worker| !retired.contains(&worker.path));
    book.observations_failed.retain(|failed| !retired.contains(&failed.path));
    let kit = crate::kit::load()?;
    probe_due(&mut book, &kit, fleet.now);
    store(&book)?;
    let source = Source::of(&fleet.repo)?;
    let mut jobs = vec![];
    for claim in &fleet.scan.claims {
        if !claim.is_held_by(&me.instance) {
            continue;
        }
        if set.mine.iter().any(|mine| {
            mine.number == claim.number
                && matches!(mine.status, Status::Landed | Status::Closed | Status::Gone)
        }) {
            if let Some(worktree) = Worktree::of_issue(&source, claim.number)? {
                book.workers.retain(|o| o.path != worktree.path);
                book.observations_failed.retain(|failed| failed.path != worktree.path);
            }
            continue;
        }
        if set.locked_out(claim.number) {
            continue;
        }
        // Startup failures are continued by the next tick, with their own
        // bounded failure path. There is no worker here to diagnose yet.
        if crate::dispatch::incomplete(fleet, claim.number)? {
            continue;
        }
        if !open.issues.rows.iter().any(|i| i.number == claim.number) {
            continue;
        }
        let Some(worktree) = Worktree::of_issue(&source, claim.number)? else {
            continue;
        };
        if waiting_on_turn(&worktree.path) {
            continue;
        }
        let state = match worktree.state(fleet.now) {
            Ok(state) => state,
            Err(_) => continue,
        };
        // An open turn has no since. A later wait starts when it is seen again.
        if state == WorkerState::Busy {
            worktree.background_since(false, None, fleet.now)?;
        }
        if let afkconvoy_core::stopped::Nudged::At(at) = worktree.nudged()?
            && crate::lock::explaining_lock(&fleet.gh, &fleet.config, Some(at))?.is_some()
        {
            book.workers.retain(|o| o.path != worktree.path);
            book.observations_failed.retain(|failed| failed.path != worktree.path);
            continue;
        }
        let busy_without_fault = worktree.agent_report()?.is_null()
            && state == WorkerState::Busy
            && classified(&worktree.health_report()?).is_none()
            && error_of(&worktree)?.is_empty();
        let capacity_episode =
            book.workers.iter().any(|o| o.path == worktree.path && o.capacity.is_some());
        if state == WorkerState::Gone || (busy_without_fault && !capacity_episode) {
            book.workers.retain(|o| o.path != worktree.path);
            book.observations_failed.retain(|failed| failed.path != worktree.path);
            continue;
        }
        let comments = open.issues.comments(claim.number);
        let prohibited_work = worktree.writes_nothing()?.is_some_and(|start| {
            start.broken(&claim.id, &worktree.tip().unwrap_or_default()).is_some()
        });
        if prohibited_work {
            book.workers.retain(|o| o.path != worktree.path);
            book.observations_failed.retain(|failed| failed.path != worktree.path);
            continue;
        }
        let verdict = crate::gh::verdict_since(comments, claim.since);
        let said = verdict.as_ref().and_then(|v| comments.get(v.comment)).map(|c| c.url.as_str());
        let answered = worktree.answered()?;
        let unbound_declared =
            afkconvoy_core::stopped::standing_verdict(verdict, said, answered.as_deref()).is_some();
        let declared = match &fleet.config.bound {
            Some(_) => flows.declared(claim.number, &claim.id),
            None => unbound_declared,
        };
        if declared {
            book.workers.retain(|o| o.path != worktree.path);
            book.observations_failed.retain(|failed| failed.path != worktree.path);
            continue;
        }
        jobs.push((Some(claim.number), None, worktree));
    }
    for run in &fleet.scan.runs {
        if !set.locks.is_empty() {
            continue;
        }
        if run.over {
            continue;
        }
        if run.owner.as_ref().is_none_or(|owner| owner.instance != me.instance.as_str()) {
            continue;
        }
        if let Some(name) = afkconvoy_core::patrol::claimed(&run.key)
            && let Some(worktree) =
                Worktree::named(&source, &afkconvoy_core::patrol::checkout_name(name))?
        {
            jobs.push((None, Some(name.to_string()), worktree));
        }
    }
    // Automatic casting is gone. An old caster worktree is not a worker.
    if let Some(worktree) = Worktree::named(&source, afkconvoy_core::casting::CASTER_WORKTREE)? {
        book.workers.retain(|worker| worker.path != worktree.path);
        book.observations_failed.retain(|failed| failed.path != worktree.path);
    }
    let mut events = vec![];
    for (issue, patrol, worktree) in jobs {
        let handled =
            observe(&mut book, fleet, me, agent, &kit, issue, patrol.as_deref(), &worktree);
        if let Err(e) = handled {
            events.push(json!({"worktree": worktree.path, "error": format!("{e:#}")}));
            book.observations_failed.retain(|failed| failed.path != worktree.path);
            book.observations_failed.push(FailedObservation {
                path: worktree.path.clone(),
                error: format!("{e:#}"),
                retry_at: fleet.now.saturating_add(CHECK_SECONDS),
            });
        } else {
            book.observations_failed.retain(|failed| failed.path != worktree.path);
        }
        store(&book)?;
    }
    store(&book)?;
    Ok(events)
}

#[allow(clippy::too_many_arguments)]
fn observe(
    book: &mut Book,
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    kit: &Kit,
    issue: Option<u64>,
    patrol: Option<&str>,
    worktree: &Worktree,
) -> Result<()> {
    let Some(terminal) = worktree.terminal()? else {
        book.workers.retain(|o| o.path != worktree.path);
        book.observations_failed.retain(|failed| failed.path != worktree.path);
        return Ok(());
    };
    let state = worktree.state(fleet.now)?;
    let orca = worktree.agent_report()?;
    let signal = error_of(worktree)?;
    // A delivery that failed after the runtime reported working still
    // belongs to classification; it cannot be cleared by that old report.
    // An open session turn is healthy. Orca's `working` is not: a harness it
    // cannot observe never reports one, and a turn the session has ended is
    // idle however the runtime last labeled it.
    let working = state == WorkerState::Busy && signal.is_empty();
    // An open turn has no since, including a patrol's or a caster's, which
    // the claim loop above does not see. A later wait starts when seen again.
    if state == WorkerState::Busy {
        worktree.background_since(false, None, fleet.now)?;
    }
    let harness = worktree.harness()?.unwrap_or(Harness::DEFAULT);
    let recorded = worktree.recorded_harness()?;
    let mismatch = recorded.is_some_and(|h| h != harness);
    let repo = Repo::new(worktree.path.clone());
    let model = std::fs::read_to_string(worktree.note("convoy-worker-model")?)
        .ok()
        .filter(|m| !m.is_empty());
    let binding = std::fs::read_to_string(worktree.note("convoy-worker-command-key")?)
        .ok()
        .filter(|s| !s.is_empty())
        .filter(|key| {
            !mismatch
                || kit
                    .harnesses
                    .iter()
                    .any(|h| h.harness == harness && *key == command_key(&h.command))
        });
    let output = if working { vec![] } else { worktree.screen(&terminal, 100)? };
    let processes = if working { vec![] } else { crate::hot::listed_checked()? };
    let activity: Vec<_> = crate::hot::tree(&processes, &worktree.path)
        .into_iter()
        .map(|p| json!({"pid":p.pid,"parent":p.ppid,"cpu_ms":p.cpu_ms,"resident_bytes":p.resident}))
        .collect();
    let scoped: Vec<_> = book
        .faults
        .iter()
        .filter(|f| {
            f.harness == harness
                && binding.is_some()
                && f.command_key == binding
                && (f.scope == Scope::Account || f.model == model)
        })
        .collect();
    let structured = json!({"runtime": worktree.health_report()?, "process_activity":activity,
        "known_scoped_faults": scoped,
        "send": serde_json::from_str::<Value>(&signal).unwrap_or(Value::Null)});
    let progress =
        format!("{}\n{}", repo.git(&["rev-parse", "HEAD"])?, repo.git(&["status", "--porcelain"])?);
    let mut observed = match book.workers.iter().position(|o| o.path == worktree.path) {
        Some(at) => book.workers.remove(at),
        None => Observed {
            path: worktree.path.clone(),
            terminal: terminal.clone(),
            harness,
            recorded_harness: None,
            harness_mismatch: false,
            orca: Value::Null,
            model: model.clone(),
            command_key: binding.clone(),
            snapshots: vec![],
            next_check: fleet.now,
            rate_retries: 0,
            capacity: None,
            continuations: 0,
            failed_candidates: vec![],
            window_started: fleet.now,
            diagnosis: None,
            judgment: None,
            operator_reason: None,
            source: "observation".into(),
            action: "observing".into(),
            successor: None,
            error: None,
        },
    };
    if observed.terminal != terminal || observed.harness != harness {
        if observed.action == "continued" {
            observed.continuations = 0;
            observed.failed_candidates.clear();
            observed.window_started = fleet.now;
        }
        observed.terminal = terminal;
        observed.harness = harness;
        observed.model = model;
        observed.command_key = binding;
        observed.snapshots.clear();
        observed.rate_retries = 0;
        observed.capacity = None;
        observed.next_check = fleet.now;
        observed.diagnosis = None;
        observed.judgment = None;
        observed.operator_reason = None;
        observed.error = None;
        observed.action = "observing".into();
    }
    // A failed turn is actionable at its first observation, including a
    // worker previously held until its next check. Repeated identical
    // failures still obey the existing retry and recovery bounds.
    let newly_failed = orca["state"] == "done"
        && orca["outcome"] == "failure"
        && ["state", "outcome", "stateStartedAt", "agentType"]
            .iter()
            .any(|key| observed.orca[*key] != orca[*key]);
    let needs_observation = observed.action == "healthy" && !working;
    if newly_failed || needs_observation {
        observed.next_check = fleet.now;
        observed.window_started = fleet.now;
        observed.diagnosis = None;
    }
    observed.harness = harness;
    observed.recorded_harness = recorded.map(|h| h.key().to_string());
    observed.harness_mismatch = mismatch;
    observed.orca = orca;
    if working {
        if let Some(capacity) = &mut observed.capacity {
            let since = *capacity.healthy_since.get_or_insert(fleet.now);
            if fleet.now.saturating_sub(since) >= CAPACITY_HEALTHY_SECONDS {
                observed.capacity = None;
            }
        }
        observed.action = "healthy".into();
        observed.source = "session turn".into();
        observed.error = None;
        observed.diagnosis = None;
        observed.judgment = None;
        observed.operator_reason = None;
        observed.snapshots.clear();
        // The runtime was just checked. A read-only watch must not mistake
        // this healthy observation for a check that is already overdue.
        observed.next_check = fleet.now.saturating_add(CHECK_SECONDS);
        book.workers.push(observed);
        return Ok(());
    }
    if let Some(capacity) = &mut observed.capacity {
        capacity.healthy_since = None;
    }
    let changed =
        observed.snapshots.last().is_some_and(|s| s.output != output || s.progress != progress);
    if changed {
        observed.snapshots.clear();
        observed.diagnosis = None;
        observed.judgment = None;
        observed.operator_reason = None;
        observed.error = None;
        observed.next_check = fleet.now;
        observed.window_started = fleet.now;
    }
    observed.snapshots.push(Snapshot {
        at: fleet.now,
        source: "orca terminal read --screen; git HEAD/status".into(),
        output,
        progress,
    });
    if observed.snapshots.len() > 3 {
        observed.snapshots.remove(1);
    }
    let previous = (observed.action.clone(), observed.continuations);
    let result = decide(
        book,
        &mut observed,
        fleet,
        me,
        agent,
        kit,
        issue,
        patrol,
        worktree,
        state,
        &structured,
    );
    if let Err(e) = &result {
        observed.error = Some(format!("{e:#}"));
        observed.action = "waiting".into();
        observed.next_check = fleet.now.saturating_add(CHECK_SECONDS);
    }
    if previous != (observed.action.clone(), observed.continuations)
        || observed.judgment.is_some()
        || observed.error.is_some()
    {
        crate::log::Log::of(&fleet.repo)?.append(&crate::log::Entry {
            at: fleet.now,
            instance: me.instance.as_str().into(),
            host: me.host.clone(),
            what: crate::log::What::Command {
                command: "worker_health".into(),
                on: issue.map(|n| n.to_string()).or_else(|| patrol.map(str::to_string)),
                by: observed.source.clone(),
                outcome: json!(observed).to_string(),
                failed: result.is_err()
                    || (observed.action == "waiting for operator" && observed.error.is_some()),
            },
        })?;
    }
    book.workers.push(observed);
    result
}

fn escalate_observed(
    observed: &mut Observed,
    fleet: &mut Fleet,
    me: &Me,
    issue: Option<u64>,
) -> Result<()> {
    let Some(issue) = issue else {
        observed.action = "waiting for new evidence".into();
        observed.next_check = fleet.now.saturating_add(CHECK_SECONDS);
        return Ok(());
    };
    let evidence = serde_json::to_string_pretty(&json!({
        "worker": observed.path,
        "harness": observed.harness,
        "diagnosis": observed.diagnosis,
        "waiting_for_operator": observed.operator_reason,
        "jev_answers": observed.judgment.as_ref().map(|judgment| &judgment["response"]["answers"]),
        "error": observed.error,
        "snapshots": observed.snapshots,
    }))?;
    let why = if observed.diagnosis.as_ref().is_some_and(|d| d.is(Class::Refusal)) {
        "No eligible harness remains for this issue after task refusal."
    } else {
        "The worker's fault remained unresolved for one hour on unchanged evidence."
    };
    let reason = format!(
        "{why} The fleet preserved its work and handed the issue to a human with this evidence:\n\n```json\n{evidence}\n```"
    );
    if observed.diagnosis.as_ref().is_some_and(|d| d.is(Class::Refusal)) {
        if let Some(claim) = crate::judge::mine(fleet, me, issue)? {
            let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
            let pr =
                afkconvoy_core::plan::closing_pr(&fleet.gh.open_prs()?, issue).map(|p| p.number);
            crate::judge::escalate_claim(
                fleet,
                &claim,
                &crate::judge::Handed::refusal_exhausted(reason),
                pr,
                worktree.as_ref(),
            )?;
        }
    } else {
        crate::judge::escalate(fleet, me, issue, &reason)?;
    }
    observed.action = "escalated".into();
    Ok(())
}

/// Hands a worker no rule or settled judgment classes to the operator:
/// `doctor` names it, with `reason`, until its evidence changes or the hour
/// that bounds every wait on unchanged evidence escalates it. A `failed`
/// reason is an error of the check, as the tick log says it.
fn wait_for_operator(
    observed: &mut Observed,
    now: u64,
    reason: String,
    failed: bool,
) -> Result<()> {
    observed.diagnosis = None;
    observed.operator_reason = Some(reason.clone());
    observed.error = failed.then_some(reason);
    observed.action = "waiting for operator".into();
    observed.next_check = now
        .saturating_add(CHECK_SECONDS)
        .min(observed.window_started.saturating_add(RECOVERY_SECONDS));
    Ok(())
}

/// Returns true while the capacity rule is waiting or has nudged this
/// session. False hands the bounded episode to ordinary continuation.
fn capacity_wait(
    book: &mut Book,
    observed: &mut Observed,
    worktree: &Worktree,
    now: u64,
) -> Result<bool> {
    let capacity = observed.capacity.get_or_insert_with(Capacity::default);
    if capacity.stops >= MAX_CAPACITY_STOPS {
        return Ok(false);
    }
    if let Some(at) = capacity.retry_at {
        observed.next_check = at;
        if now < at {
            observed.action = format!(
                "waiting {} seconds for capacity",
                CAPACITY_WAIT_SECONDS * u64::from(capacity.stops),
            );
            return Ok(true);
        }
        // Leave the wait pending on a failed submission. An accepted input
        // without a turn-start receipt must not spend another capacity stop.
        worktree.tell_at(
            &observed.terminal,
            "The selected model was at capacity. Continue the existing task from your current work.",
            now,
        )?;
        capacity.retry_at = None;
        observed.next_check = now;
        observed.action = "capacity nudged".into();
        return Ok(true);
    }
    capacity.stops = capacity.stops.saturating_add(1);
    let total = book.capacity_stops.entry(observed.harness.key().into()).or_default();
    *total = total.saturating_add(1);
    if capacity.stops >= MAX_CAPACITY_STOPS {
        return Ok(false);
    }
    let at = now.saturating_add(CAPACITY_WAIT_SECONDS * u64::from(capacity.stops));
    capacity.retry_at = Some(at);
    observed.next_check = at;
    observed.action = format!(
        "waiting {} seconds for capacity",
        CAPACITY_WAIT_SECONDS * u64::from(capacity.stops),
    );
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn decide(
    book: &mut Book,
    observed: &mut Observed,
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    kit: &Kit,
    issue: Option<u64>,
    patrol: Option<&str>,
    worktree: &Worktree,
    state: WorkerState,
    structured: &Value,
) -> Result<()> {
    let output = observed.snapshots.last().map(|s| s.output.as_slice()).unwrap_or_default();
    let terminal = terminal_rule(observed.harness, output);
    if fleet.now < observed.next_check && terminal.is_none() {
        return Ok(());
    }
    observed.judgment = None;
    let expired = fleet.now.saturating_sub(observed.window_started) >= RECOVERY_SECONDS;
    if terminal.is_none()
        && expired
        && (observed.error.is_some()
            || observed.action == "waiting for operator"
            || observed.continuations >= MAX_CONTINUATIONS)
    {
        return escalate_observed(observed, fleet, me, issue);
    }
    let raw_error = structured.get("error").unwrap_or(structured);
    let rule = terminal.map(|rule| ("terminal rule", rule)).or_else(|| {
        classified_for(Some(observed.harness), raw_error).map(|rule| ("structured rule", rule))
    });
    let failed = observed.orca["state"] == "done" && observed.orca["outcome"] == "failure";
    if !failed && rule.is_none() {
        // One read stamps the since the tick line and a later tick name.
        if crate::judge::own_background(fleet, worktree, true, crate::judge::Look::Acting)?
            .is_some()
        {
            observed.action = "waiting for background work".into();
            observed.next_check = fleet.now.saturating_add(CHECK_SECONDS);
            return Ok(());
        }
    }
    let quiet = fleet.now.saturating_sub(observed.snapshots.first().map_or(fleet.now, |s| s.at));
    if !failed && rule.is_none() && (state == WorkerState::Busy || quiet < OBSERVE_SECONDS) {
        observed.action = if state == WorkerState::Busy { "healthy" } else { "observing" }.into();
        return Ok(());
    }
    let mut diagnosis = if let Some((source, rule)) = rule {
        observed.source = source.into();
        rule
    } else {
        let (request, response, decided) = match crate::jev::judge(output) {
            crate::jev::Outcome::NoKey => {
                let reason = "no Jev key command is set in the host settings".into();
                return wait_for_operator(observed, fleet.now, reason, false);
            }
            crate::jev::Outcome::Judged { request, response, decision } => {
                (request, response, decision.map_err(|reason| (reason, false)))
            }
            crate::jev::Outcome::Failed { request, response, error } => {
                (request, response, Err((error, true)))
            }
        };
        observed.source = "jev".into();
        observed.judgment =
            (!request.is_null()).then(|| json!({"request": request, "response": response}));
        match decided {
            Ok(decision) => decision.into(),
            Err((reason, failed)) => return wait_for_operator(observed, fleet.now, reason, failed),
        }
    };
    observed.diagnosis = Some(diagnosis.clone());
    observed.operator_reason = None;
    if diagnosis.is(Class::Capacity) {
        observed.source = "capacity rule".into();
        observed.error = None;
        if capacity_wait(book, observed, worktree, fleet.now)? {
            return Ok(());
        }
        diagnosis.action = Action::Continue;
        observed.diagnosis = Some(diagnosis.clone());
    }
    let refusal = diagnosis.is(Class::Refusal);
    if refusal && let Some(issue) = issue {
        fleet.remember_refusal(
            issue,
            afkconvoy_core::worker::Refusal {
                harness: observed.harness,
                evidence: diagnosis.evidence.join("\n"),
            },
        )?;
    }
    observed.error = None;
    observed.next_check =
        fleet.now.saturating_add(diagnosis.retry_after.unwrap_or(CHECK_SECONDS).max(1));
    match diagnosis.action {
        Action::Nudge => {
            if let Some(issue) = issue {
                use afkconvoy_core::stopped::Silence;
                match crate::judge::silence_of(
                    fleet,
                    issue,
                    worktree,
                    state,
                    None,
                    crate::judge::Look::Acting,
                )? {
                    Silence::Nudge(unnudged) => {
                        let line = if fleet.config.bound.is_some() {
                            afkconvoy_core::prompt::stage_nudge_line
                        } else {
                            afkconvoy_core::prompt::nudge_line
                        };
                        crate::judge::nudge_worker(worktree, unnudged, line, fleet.now)?;
                        observed.action = "nudged".into();
                        return Ok(());
                    }
                    Silence::Stalled(_) => {
                        return wait_for_operator(
                            observed,
                            fleet.now,
                            "worker is still short of its task after its nudge".into(),
                            false,
                        );
                    }
                    _ => {
                        observed.action = "observing".into();
                        return Ok(());
                    }
                }
            }
            return wait_for_operator(
                observed,
                fleet.now,
                "worker is short of its task".into(),
                false,
            );
        }
        Action::Complete => {
            observed.action = "complete".into();
            return Ok(());
        }
        Action::Wait | Action::Unknown => {
            let quota = diagnosis.is(Class::Quota) && diagnosis.scope == Scope::Account;
            let within_reset = diagnosis.retry_after.is_some_and(|delay| {
                delay <= MAX_QUOTA_WAIT_SECONDS
                    && fleet.now.saturating_add(delay)
                        <= observed.window_started.saturating_add(MAX_QUOTA_WAIT_SECONDS)
            });
            if quota && within_reset {
                observed.action = "waiting for quota reset".into();
                return Ok(());
            }
            if !quota {
                if expired {
                    return escalate_observed(observed, fleet, me, issue);
                }
                observed.next_check = observed
                    .next_check
                    .min(observed.window_started.saturating_add(RECOVERY_SECONDS));
                observed.action = "waiting".into();
                return Ok(());
            }
        }
        Action::Retry if observed.rate_retries < MAX_RATE_RETRIES => {
            observed.rate_retries += 1;
            observed.action = "retrying".into();
            if !diagnosis.is(Class::RateLimit) {
                worktree.tell(
                    &observed.terminal,
                    "Continue the existing task if ready. Do not restart or discard work.",
                )?;
            }
            return Ok(());
        }
        Action::Retry if !diagnosis.is(Class::RateLimit) => {
            observed.action = "waiting".into();
            return Ok(());
        }
        _ => {}
    }
    if observed.command_key.is_some()
        && diagnosis.scope != Scope::Worker
        && let Some(held) = kit.harnesses.iter().find(|h| h.harness == observed.harness)
    {
        let fault = Fault {
            harness: held.harness,
            command_key: observed.command_key.clone(),
            model: if diagnosis.scope == Scope::Model { observed.model.clone() } else { None },
            scope: diagnosis.scope,
            since: fleet.now,
            next_check: fleet.now.saturating_add(RECOVERY_SECONDS),
            evidence: diagnosis.evidence.clone(),
        };
        if !book.faults.iter().any(|f| {
            f.harness == fault.harness
                && f.command_key == fault.command_key
                && f.model == fault.model
                && f.scope == fault.scope
        }) {
            book.faults.push(fault);
        }
    }
    // An account quota continues at once, but not through a gate or landing
    // the worker still has running: that work keeps the session until it
    // ends. The fault is already recorded, so a new start skips the harness.
    if diagnosis.is(Class::Quota)
        && diagnosis.scope == Scope::Account
        && crate::judge::own_background(fleet, worktree, true, crate::judge::Look::Acting)?
            .is_some()
    {
        observed.action = "waiting for background work".into();
        observed.next_check = fleet.now.saturating_add(CHECK_SECONDS);
        store(book)?;
        fleet.invalidate_selection();
        return Ok(());
    }
    // A fallback never promotes another kit entry. Exclusions last only for
    // this continuation; shared scoped evidence is persisted independently.
    std::fs::write(
        worktree.note("convoy-recovery-evidence.json")?,
        serde_json::to_vec_pretty(&json!({"source": observed.source, "diagnosis": diagnosis,
            "snapshots": observed.snapshots, "pending": "continue the original task and current stage"}))?,
    )?;
    let same_command =
        kit.harnesses.iter().find(|h| h.harness == observed.harness).is_none_or(|h| {
            observed.command_key.as_ref().is_none_or(|key| key == &command_key(&h.command))
        });
    // A proved model fault removes that model through available(), so the
    // same harness's next model remains the first continuation candidate.
    // Session faults still need a different harness; legacy observations
    // without a command/model identity cannot safely exclude just one model.
    let model_fault = diagnosis.scope == Scope::Model
        && observed.model.is_some()
        && observed.command_key.is_some();
    let mut excluded = if same_command && !model_fault { vec![observed.harness] } else { vec![] };
    store(book)?;
    fleet.invalidate_selection();
    if refusal && let Some(issue) = issue {
        // Selection happens before the old writer is stopped. Exhaustion is
        // settled by the rule now, rather than waiting out a recovery window.
        if let Err(error) = crate::dispatch::continuation_harness(fleet, agent, issue) {
            let why = format!("{error:#}");
            if error.is::<crate::fleet::NothingSafe>()
                || error.is::<crate::paused::Held>()
                || why.contains("no eligible harness")
            {
                observed.error = Some(why);
                return escalate_observed(observed, fleet, me, Some(issue));
            }
            return Err(error);
        }
    }
    excluded.extend(
        observed
            .failed_candidates
            .iter()
            .filter(|(_, at)| fleet.now < *at)
            .filter_map(|(h, _)| Harness::read(h)),
    );
    let mut failures = vec![];
    for candidate in &kit.harnesses {
        // A checkout that names a harness does not continue onto the next
        // one in the host kit. A model fault still retries this harness.
        if fleet.pinned.is_some_and(|pinned| candidate.harness != pinned) {
            continue;
        }
        if excluded.contains(&candidate.harness) {
            continue;
        }
        if !refusal && observed.continuations >= MAX_CONTINUATIONS {
            break;
        }
        observed.continuations += 1;
        if crate::role::startable(&agent.launches, candidate.harness, "fault continuation").is_err()
        {
            excluded.push(candidate.harness);
            continue;
        }
        fleet.avoiding = excluded.clone();
        let next = match issue {
            Some(n) => crate::switch::switch_issue(fleet, me, agent, n, observed.harness),
            None if patrol.is_some_and(|p| p.starts_with('@')) => {
                crate::switch::auxiliary(fleet, agent, worktree)
            }
            None => crate::patrol::switch(
                fleet,
                me,
                agent,
                patrol.context("patrol continuation")?,
                |w| crate::switch::continuing(Some(w), observed.harness),
            ),
        };
        match next {
            Ok(next) => {
                observed.successor = next["to"].as_str().map(str::to_string);
                observed.action = "continued".into();
                observed.next_check = fleet.now.saturating_add(CHECK_SECONDS);
                break;
            }
            Err(e) => {
                failures.push(format!("{}: {e:#}", candidate.harness.key()));
                excluded.push(candidate.harness);
                observed.failed_candidates.push((
                    candidate.harness.key().into(),
                    fleet.now.saturating_add(RECOVERY_SECONDS),
                ));
            }
        }
    }
    fleet.avoiding.clear();
    if observed.action != "continued" {
        if expired {
            observed.error = Some(format!("no successor started: {}", failures.join("; ")));
            return escalate_observed(observed, fleet, me, issue);
        }
        // The named harness is the only candidate. The fault stays on it, the
        // claim stays, and the tick goes on so plan and doctor can say why.
        // No other harness of the host kit is started in its place.
        if fleet.pinned.is_some() {
            observed.action = "staying".into();
            observed.error = Some(
                "this checkout names this harness, so no worker starts on another harness of the \
                 host kit"
                    .to_string(),
            );
            return Ok(());
        }
        bail!("no successor started; preserving claim and work: {}", failures.join("; "));
    }
    let entry = crate::log::Entry {
        at: fleet.now,
        instance: me.instance.as_str().into(),
        host: me.host.clone(),
        what: crate::log::What::Command {
            command: "worker_health".into(),
            on: issue.map(|n| n.to_string()).or_else(|| patrol.map(str::to_string)),
            by: observed.source.clone(),
            outcome: json!(observed).to_string(),
            failed: false,
        },
    };
    crate::log::Log::of(&fleet.repo)?.append(&entry)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_turn_and_every_wait_exempt_health_but_repairs_do_not() {
        use afkconvoy_core::ending::Ending;

        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::new(dir.path().to_path_buf());
        repo.git(&["init", "-q"]).unwrap();
        assert!(!waiting_on_turn(dir.path()));
        for ending in Ending::ALL {
            crate::turn_answer::noted(&repo, None, 1700, || {
                assert!(waiting_on_turn(dir.path()), "a live Asking process needs no operator");
                Ok(json!({"outcome": ending.word()}))
            })
            .unwrap();
            assert_eq!(waiting_on_turn(dir.path()), ending.waits(), "{}", ending.word());
        }
        let note = repo.git_dir().unwrap().join(crate::orca::TURN_FILE);
        std::fs::write(&note, "not a turn note").unwrap();
        assert!(!waiting_on_turn(dir.path()));
        std::fs::write(&note, json!({"outcome": "error", "at": 1700}).to_string()).unwrap();
        assert!(waiting_on_turn(dir.path()));
    }

    fn screen(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    /// What the harness reports is an account quota that continues. The
    /// words themselves are the harness's; here only the class is decided.
    #[test]
    fn an_account_quota_continues_and_only_on_the_harness_that_names_it() {
        let cases = [
            (
                Harness::Codex,
                screen(&["Usage limit", "reached. Increase your limits to continue using codex."]),
            ),
            (
                Harness::Kimi,
                screen(&[
                    "Error: [provider.api_error] 429 You've reached your",
                    "5-hour usage limit.",
                ]),
            ),
            (
                Harness::Pi,
                screen(&[
                    "Error: 429: {\"code\":\"1308\",\"message\":\"Usage limit reached for 5 hour. Your limit will reset at",
                    "2026-10-27 17:52:40\"}",
                ]),
            ),
        ];
        for (harness, output) in cases {
            let diagnosis = terminal_rule(harness, &output).unwrap();
            assert_eq!(diagnosis.class, Class::Quota.key(), "{}", harness.key());
            assert_eq!(diagnosis.scope, Scope::Account);
            assert_eq!(diagnosis.action, Action::Continue);
            assert_eq!(diagnosis.evidence, vec![harness.account_quota(&output).unwrap()]);
            for other in Harness::ALL {
                if other != harness {
                    assert!(terminal_rule(other, &output).is_none(), "{}", other.key());
                }
            }
        }
    }

    #[test]
    fn a_runtime_account_quota_continues_for_the_harness_that_names_it() {
        let error = r#"429: {"code":"1308","message":"Usage limit reached for 5 hour"}"#;
        let diagnosis = classified_for(Some(Harness::Pi), &json!(error)).unwrap();
        assert_eq!(diagnosis.class, Class::Quota.key());
        assert_eq!(diagnosis.scope, Scope::Account);
        assert_eq!(diagnosis.action, Action::Continue);
        assert_eq!(diagnosis.retry_after, None);
        assert!(classified_for(Some(Harness::Codex), &json!(error)).is_none());
        assert!(classified_for(Some(Harness::Kimi), &json!(error)).is_none());
    }

    /// A worker as the book records it, observed next at `next_check`.
    fn observed(path: &str, next_check: u64, action: &str, output: &[&str]) -> Observed {
        serde_json::from_value(json!({"path": path, "terminal": "term-1", "harness": "codex",
            "snapshots": [{"at": 1, "source": "orca", "output": output, "progress": ""}],
            "next_check": next_check, "rate_retries": 0, "source": "rule", "action": action,
            "operator_reason": "Jev left the table unsettled", "error": null}))
        .unwrap()
    }

    fn health(workers: Vec<Observed>, failed: Vec<FailedObservation>) -> Health {
        Health {
            workers: workers
                .into_iter()
                .map(|observed| Worker { observed, background: None })
                .collect(),
            observations_failed: failed,
            ..Health::default()
        }
    }

    #[test]
    fn an_observation_is_overdue_only_past_its_tick_s_chance() {
        // The instance holding /w sleeps 90s and its last tick took 10s; one
        // holding nothing here ticks every 300s.
        let mut cadence = Cadence::default();
        cadence.instance(90, 10, [PathBuf::from("/w")]);
        cadence.instance(280, 20, []);
        let at = |next: u64| {
            health(
                vec![observed("/w", next, "waiting", &[])],
                vec![FailedObservation {
                    path: "/f".into(),
                    error: "no terminal".into(),
                    retry_at: next,
                }],
            )
        };
        assert!(
            at(998).overdue(&cadence, 1000).is_empty(),
            "the next tick observes it on schedule"
        );
        assert!(at(901).overdue(&cadence, 1000).is_empty(), "within its sleep and a tick's time");
        assert_eq!(
            at(899).overdue(&cadence, 1000),
            [Overdue::Waiting { path: "/w".into(), due: 899, error: None }],
            "a worktree no claim holds gets the longest cadence"
        );
        assert_eq!(
            at(700).overdue(&cadence, 1000),
            [
                Overdue::Waiting { path: "/w".into(), due: 700, error: None },
                Overdue::Retry { path: "/f".into(), error: "no terminal".into() },
            ]
        );
        assert_eq!(
            at(1000).overdue(&Cadence::default(), 1000).len(),
            2,
            "with none said, no grace"
        );
    }

    #[test]
    fn a_worker_waiting_for_the_operator_says_why_its_screen_and_its_fix() {
        let waiting = observed("/w", 0, "waiting for operator", &["working", "■ stopped", "  "]);
        let keyless = waiting.operator_wait(false).unwrap();
        assert_eq!(
            keyless,
            OperatorWait {
                worker: "/w".into(),
                reason: "Jev left the table unsettled".into(),
                screen_line: "■ stopped".into(),
                fix: "afkconvoy run --jev-key-command '<command>'".into(),
            }
        );
        let keyed = waiting.operator_wait(true).unwrap();
        assert_eq!(keyed.fix, "orca terminal read --terminal term-1 --screen");
        assert!(observed("/w", 0, "healthy", &[]).operator_wait(false).is_none());
    }

    #[test]
    fn the_view_is_written_as_the_book_was() {
        let failed =
            FailedObservation { path: "/f".into(), error: "no terminal".into(), retry_at: 7 };
        let shown = json(&Ok(health(vec![observed("/w", 5, "healthy", &[])], vec![failed])));
        assert_eq!(shown["observations_failed"], json!([["/f", "no terminal", 7]]));
        assert!(shown["workers"][0].get("background").is_none(), "{shown}");
        assert_eq!(shown["workers"][0]["next_check"], 5);
        let read: Health = serde_json::from_value(shown.clone()).unwrap();
        assert_eq!(json(&Ok(read)), shown);
        assert_eq!(json(&Err("unreadable".into())), json!({"error": "unreadable"}));
    }
}

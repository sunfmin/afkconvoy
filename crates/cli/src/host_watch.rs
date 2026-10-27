//! The host runner's watch of its host (docs/adr/0030). A look is the fleet's
//! own `doctor --all --since`, and starts no model; a look holding something
//! an operator would act on starts one bounded watch in the operator's
//! directory, or afkconvoy's own, continued where its harness gives out. The
//! process holds nothing of it: the watch, its terminal and its transcript are
//! read back from the host's cache after every restart.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::host_watch::{self, Item};
use afkconvoy_core::prompt::brief_pointer;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::checkout_list::List;
use crate::copies::cache;
use crate::git::Repo;
use crate::health::Diagnosis;
use crate::log::{Entry, What};
use crate::orca::Worktree;
use crate::watch_report::Report;
use crate::watch_terminal::{self, Phase, TITLE};

const STATE: &str = "watch/state.json";
/// How often a running watch's terminal and transcript are read.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// How long a look may take before it is given up, and taken again later.
const LOOK_SECONDS: u64 = 300;
/// How many ended watches are kept, for what earlier watches on the same
/// items did.
const HISTORY: usize = 20;
/// The linked checkout a watch makes for a change of a listed repo.
pub const CHECKOUT: &str = "afkconvoy-watch";

/// The four kinds every watch leaves with a human, in either directory.
const HUMAN_KINDS: &str = "\
- spending money, or needing an account, a credential or a person's hands;\n\
- what cannot be undone;\n\
- taking a secret or private data off the host;\n\
- changing what a product is for, or who it is for.";

/// The stock watch's whole instruction, kept as its directory's rules.
fn stock_instruction() -> String {
    format!(
        "\
# The stock watch

Find what holds this host's fleets and their work back, fix it with the watch's authority,
check that the work goes on, and say what you did.
Leave these with a human, never act on them, and list each in your report:
{HUMAN_KINDS}
"
    )
}

/// Where the host's watch runs. Resolving the stock directory writes nothing:
/// only starting a stock watch makes it and puts its instruction there.
pub fn directory(settings: &crate::host_settings::Settings) -> Result<PathBuf> {
    match &settings.watch_dir {
        Some(directory) => Ok(directory.clone()),
        None => Ok(cache()?.join("stock-watch")),
    }
}

/// A watch as the host keeps it: what a continuation of it needs too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watch {
    pub id: String,
    pub directory: PathBuf,
    /// Whether it started under afkconvoy's own instruction. Kept with the
    /// watch so a later host-setting change cannot change a continuation.
    #[serde(default)]
    pub stock: bool,
    /// What the look that started it answered.
    pub look: Value,
    /// Why it was started.
    pub items: Vec<Item>,
    pub started: u64,
    /// The session running now, or the last one.
    #[serde(deserialize_with = "crate::kit::harness")]
    pub harness: Harness,
    pub model: Option<String>,
    #[serde(default)]
    pub model_source: Option<String>,
    pub terminal: String,
    pub transcript: PathBuf,
    /// What earlier watches on the same items did.
    #[serde(default)]
    pub earlier: Vec<String>,
    /// What its sessions did, each line one thing.
    #[serde(default)]
    pub did: Vec<String>,
    #[serde(default)]
    pub report: Option<Report>,
    #[serde(default)]
    pub nudged: Option<u64>,
    #[serde(default)]
    pub session_started: u64,
    /// Each session whose harness gave out, in order.
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// A session of a watch whose harness gave out, and what it did first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    #[serde(deserialize_with = "crate::kit::harness")]
    pub harness: Harness,
    pub model: Option<String>,
    #[serde(default)]
    pub model_source: Option<String>,
    pub terminal: String,
    pub why: String,
    #[serde(default)]
    pub did: Vec<String>,
    /// Terminal evidence for continuation, never a report of actions taken.
    #[serde(default)]
    pub screen: Vec<String>,
}

/// A watch as `doctor` says it: all but the look, which is the watch's own.
pub fn watch_json(watch: &Watch) -> Value {
    json!({"id": watch.id, "directory": watch.directory, "stock": watch.stock, "items": watch.items,
        "started": watch.started, "harness": watch.harness, "model": watch.model,
        "model_source": watch.model_source,
        "terminal": watch.terminal, "transcript": watch.transcript, "did": watch.did,
        "reported": watch.report.is_some(), "report": watch.report,
        "attempts": watch.attempts, "ended": watch.ended, "outcome": watch.outcome})
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// The `until` of the last look: where the next starts.
    until: Option<u64>,
    /// The items a watch was started on, while their episodes last.
    watched: Vec<String>,
    /// Last successful start that took kit research, retained across restarts.
    research_started: Option<u64>,
    running: Option<Watch>,
    last: Option<Watch>,
    history: Vec<Watch>,
    /// Why the last watch a look called for could not start or go on.
    unavailable: Option<String>,
}

impl State {
    /// A start reserves its episodes while the watch runs; only a report
    /// keeps them covered afterwards. Reconcile from retained outcomes so a
    /// restart also repairs coverage left by an older runner. A later report
    /// on the same item wins over an earlier unsuccessful watch.
    fn retry_unreported(&mut self) {
        let mut seen = BTreeSet::new();
        for watch in self.last.iter().chain(self.history.iter().rev()) {
            for item in &watch.items {
                if seen.insert(&item.id) && watch.report.is_none() {
                    self.watched.retain(|id| id != &item.id);
                }
            }
            if watch.report.is_none()
                && self.research_started == Some(watch.started)
                && watch.items.iter().any(|item| item.id.starts_with(host_watch::RESEARCH_ITEM))
            {
                self.research_started = None;
            }
        }
    }
}

fn read() -> Result<State> {
    let file = cache()?.join(STATE);
    match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("cannot read the watch's state {}", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn running() -> Result<Option<Watch>> {
    match read()?.running {
        Some(watch) => Ok(Some(watch)),
        None => watch_terminal::active(),
    }
}

/// The running watch, shared with what the host runner answers.
#[derive(Clone, Default)]
pub struct Shared {
    running: Arc<Mutex<Option<Watch>>>,
}

impl Shared {
    fn set(&self, running: Option<Watch>) {
        *self.lock() = running;
    }

    fn lock(&self) -> MutexGuard<'_, Option<Watch>> {
        self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn running(&self) -> Option<Watch> {
        self.lock().clone()
    }
}

/// The host runner's watch, on a thread of its own, for as long as it is
/// held. Dropping it ends that thread, a look it is taking with it, and waits
/// for it: never for the watch's session, which it does not end — the next
/// host runner keeps it.
pub struct HostWatch {
    ended: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HostWatch {
    /// Watches the host, saying each start, end and thing done to `say`.
    pub fn start(shared: Shared, say: impl Fn(What) + Send + Sync + 'static) -> HostWatch {
        let ended = Arc::new(AtomicBool::new(false));
        let watcher = Watcher {
            ended: Arc::clone(&ended),
            shared,
            say: Box::new(say),
            looking: Failing::default(),
            checking: Failing::default(),
        };
        HostWatch { ended, thread: Some(std::thread::spawn(move || watcher.run())) }
    }
}

impl Drop for HostWatch {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Watcher {
    ended: Arc<AtomicBool>,
    shared: Shared,
    say: Box<dyn Fn(What) + Send + Sync>,
    looking: Failing,
    checking: Failing,
}

/// What failed last of one kind of step, said once until it fails otherwise.
#[derive(Default)]
struct Failing(Option<String>);

impl Failing {
    fn said(&mut self, done: Result<()>, say: &dyn Fn(What)) {
        let failed = done.err().map(|error| format!("{error:#}"));
        if let Some(said) = &failed
            && self.0.as_ref() != Some(said)
        {
            say(What::WatchFailed { said: said.clone() });
        }
        self.0 = failed;
    }
}

impl Watcher {
    fn run(mut self) {
        self.shared.set(read().ok().and_then(|state| state.running));
        let (mut next_look, mut next_check) = (Instant::now(), Instant::now());
        while !self.ended.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= next_check {
                next_check = now + CHECK_EVERY;
                let checked = self.check();
                self.checking.said(checked, &self.say);
            }
            // A look is taken only while no watch runs: what it would find
            // waits for the first look after the watch ends.
            if now >= next_look && self.shared.running().is_none() {
                next_look = now + CHECK_EVERY;
                let looked = crate::host_settings::of_host().and_then(|settings| {
                    next_look = now + Duration::from_secs(settings.watch_every());
                    self.look(&settings)
                });
                self.looking.said(looked, &self.say);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn save(&self, state: &State) -> Result<()> {
        let file = cache()?.join(STATE);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
        std::fs::rename(temporary, &file)?;
        self.shared.set(state.running.clone());
        Ok(())
    }

    /// One look at the host, and the watch it calls for.
    fn look(&self, settings: &crate::host_settings::Settings) -> Result<()> {
        let mut state = read()?;
        if state.running.is_some() {
            return Ok(());
        }
        let directory = directory(settings)?;
        if settings.watch_dir.is_some() && !directory.is_dir() {
            bail!(
                "the watch directory {} is not a directory: `afkconvoy run --watch-dir <dir>` \
                 names the operator's",
                directory.display()
            );
        }
        let mut look = doctor_look(state.until, &self.ended)?;
        state.until = look["until"].as_u64().or(state.until);
        let current = actionable(&look, look["until"].as_u64().unwrap_or_else(crate::clock));
        let mut episodes = current.clone();
        for resource in look["host"]["resources"].as_array().into_iter().flatten() {
            if let Some(id) = resource["id"].as_str() {
                episodes.push(Item::new(id, "resource episode recovering"));
            }
        }
        state.retry_unreported();
        state.watched = host_watch::covered(&state.watched, &episodes);
        let mut pending = host_watch::pending(
            &current,
            &state.watched,
            false,
            state.research_started,
            crate::clock(),
        );
        if pending.is_empty() {
            state.unavailable = None;
        } else {
            crate::resources::evidence(&mut look, &pending)?;
            // Disk evidence can take minutes. A condition that recovered
            // while it was taken must not start a watch from an old look.
            let resources = crate::resources::json(crate::clock())?;
            pending.retain(|item| {
                !item.id.starts_with("resource:")
                    || resources
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|resource| resource["id"] == item.id && resource["active"] == true)
            });
            if !pending.is_empty() {
                self.start(&mut state, &directory, settings.watch_dir.is_none(), look, pending)?;
            }
        }
        self.save(&state)
    }

    /// Starts a watch on `items` on the first harness that can start it.
    fn start(
        &self,
        state: &mut State,
        directory: &Path,
        stock: bool,
        look: Value,
        items: Vec<Item>,
    ) -> Result<()> {
        let ids: BTreeSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
        let earlier = state
            .history
            .iter()
            .filter(|watch| watch.items.iter().any(|item| ids.contains(item.id.as_str())))
            .flat_map(|watch| {
                let ended = format!(
                    "The watch {} started at {} UTC ended: {}",
                    watch.id,
                    crate::log::utc(watch.started),
                    watch.outcome.as_deref().unwrap_or("unknown")
                );
                let report = match &watch.report {
                    Some(report) => serde_json::to_string_pretty(report).unwrap_or_default(),
                    None => "It did not report.".into(),
                };
                [ended, report]
            })
            .collect();
        let now = crate::clock();
        let mut watch = Watch {
            id: format!("{now}-{}", crate::unique()),
            directory: directory.to_path_buf(),
            stock,
            look,
            items,
            started: now,
            harness: Harness::DEFAULT,
            model: None,
            model_source: None,
            terminal: String::new(),
            transcript: PathBuf::new(),
            earlier,
            did: vec![],
            report: None,
            nudged: None,
            session_started: now,
            attempts: vec![],
            ended: None,
            outcome: None,
        };
        // Keep a finished watch's screen until its replacement actually starts
        // in that directory. Never close the operator's other terminals.
        let mut closed = BTreeSet::new();
        for last in
            state.history.iter().chain(state.last.iter()).filter(|last| last.directory == directory)
        {
            if !closed.insert(last.terminal.clone()) {
                continue;
            }
            let worktree = Worktree::at(last.directory.clone(), String::new());
            close(&worktree, &last.terminal, worktree.live(&last.terminal)?)?;
        }
        let mut failures = vec![];
        for candidate in crate::health::watch_candidates()? {
            watch.harness = candidate.harness;
            (watch.model, watch.model_source) = if candidate.harness == Harness::Codex {
                let (model, source) = crate::codex_launch::automatic_model(None)?;
                (model, Some(source.to_string()))
            } else {
                (candidate.model, None)
            };
            match start_session(&mut watch) {
                Ok((terminal, transcript)) => {
                    (watch.terminal, watch.transcript) = (terminal, transcript);
                    state.watched.extend(watch.items.iter().map(|item| item.id.clone()));
                    state.watched.sort();
                    state.watched.dedup();
                    if watch.items.iter().any(|item| item.id.starts_with(host_watch::RESEARCH_ITEM))
                    {
                        state.research_started = Some(now);
                    }
                    state.unavailable = None;
                    (self.say)(What::WatchStarted {
                        id: watch.id.clone(),
                        directory: watch.directory.display().to_string(),
                        harness: watch.harness.key().to_string(),
                        reasons: watch.items.iter().map(|item| item.reason.clone()).collect(),
                    });
                    state.running = Some(watch);
                    return Ok(());
                }
                Err(error) => {
                    failures.push(format!("{}: {error:#}", candidate.harness.key()));
                    if let Some(pending) = watch_terminal::active()? {
                        state.running = Some(pending);
                        state.unavailable = Some(watch_terminal::recovery(&failures.join("; ")));
                        return Ok(());
                    }
                }
            }
        }
        state.unavailable = Some(unstartable(&failures));
        Ok(())
    }

    /// Reads the running watch, and ends or continues it where its session
    /// is over, its harness gave out or its hour is up.
    fn check(&self) -> Result<()> {
        let mut state = read()?;
        let kept = state.running.take();
        let active = watch_terminal::active()?;
        let running = match (kept, active) {
            (Some(kept), Some(active))
                if kept.id == active.id && kept.attempts.len() == active.attempts.len() =>
            {
                Some(kept)
            }
            (_, Some(active)) => Some(active),
            (kept, None) => kept,
        };
        let Some(mut watch) = running else {
            self.shared.set(None);
            return Ok(());
        };
        for item in &watch.items {
            if !state.watched.contains(&item.id) {
                state.watched.push(item.id.clone());
            }
            if item.id.starts_with(host_watch::RESEARCH_ITEM) {
                state.research_started = Some(watch.started);
            }
        }
        let worktree = Worktree::at(watch.directory.clone(), String::new());
        let transcript = std::fs::read_to_string(&watch.transcript).unwrap_or_default();
        let exited = host_watch::exited(&transcript);
        let now = crate::clock();
        if let Some(report) = crate::watch_report::read(&watch.id)? {
            stop_session(&watch)?;
            state.unavailable = None;
            for did in &report.did {
                (self.say)(What::WatchDid { id: watch.id.clone(), did: did.clone() });
            }
            watch.did = report.did.clone();
            watch.report = Some(report);
            self.finish_said(&mut state, watch, "ended", "it reported; its harness was ended");
        } else {
            // Recovery precedes every terminal read, input and termination.
            // An uncertain creation or delivery blocks the next launch too.
            let recovered = (|| -> Result<bool> {
                let terminal = watch_terminal::locate(&watch)?;
                if let Some(terminal) = terminal {
                    watch.terminal = terminal;
                    resume_delivery(&mut watch)?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })();
            let live = match recovered {
                Ok(live) => live,
                Err(error) => {
                    let why = watch_terminal::recovery(&format!("{error:#}"));
                    watch_terminal::problem(&why)?;
                    state.unavailable = Some(why);
                    state.running = Some(watch);
                    return self.save(&state);
                }
            };
            state.unavailable = None;
            if host_watch::expired(watch.started, now) {
                stop_session(&watch)?;
                self.finish_said(
                    &mut state,
                    watch,
                    "time_limit",
                    "it reached its hour and did not report",
                );
            } else if !live && let Some(fault) = gave_out(watch.harness, &transcript, exited) {
                stop_session(&watch)?;
                self.go_on(&mut state, watch, fault, now, vec![])?;
            } else if !live {
                stop_session(&watch)?;
                self.finish_said(
                    &mut state,
                    watch,
                    "closed",
                    "its terminal closed; it did not report",
                );
            } else {
                let screen = worktree.screen(&watch.terminal, host_watch::EVIDENCE_LINES)?;
                let (worker, runtime) = worktree.terminal_health(&watch.terminal, now)?;
                let fault = crate::health::watch_diagnosis(watch.harness, &screen, &runtime)
                    .or_else(|| gave_out(watch.harness, &transcript, exited));
                if let Some(fault) = fault {
                    stop_session(&watch)?;
                    close(&worktree, &watch.terminal, live)?;
                    self.go_on(&mut state, watch, fault, now, screen)?;
                } else if exited.is_some() {
                    stop_session(&watch)?;
                    self.finish_said(
                        &mut state,
                        watch,
                        "not_reported",
                        "its harness exited; it did not report",
                    );
                } else {
                    let grace = afkconvoy_core::worker::GRACE;
                    let stopped = match worker {
                        afkconvoy_core::worker::WorkerState::Idle { quiet: Some(quiet) } => {
                            quiet >= grace
                        }
                        afkconvoy_core::worker::WorkerState::Idle { quiet: None } => true,
                        _ => false,
                    };
                    if stopped
                        && watch.nudged.is_none()
                        && now.saturating_sub(watch.session_started.max(watch.started)) >= grace
                    {
                        worktree.tell_watch(&watch.terminal,
                    "Continue this watch. You have not reported; finish the work and run the exact afkconvoy watched line at the end of your brief.")?;
                        watch.nudged = Some(now);
                    }
                    state.running = Some(watch);
                }
            }
        }
        self.save(&state)
    }

    /// Continues `watch`, whose harness gave out, on the next harness that
    /// can start it, with the same look and what it did so far.
    fn go_on(
        &self,
        state: &mut State,
        mut watch: Watch,
        fault: Diagnosis,
        now: u64,
        screen: Vec<String>,
    ) -> Result<()> {
        let why = fault.evidence.join("; ");
        crate::health::watch_fault(watch.harness, &fault, now)?;
        let from = watch.harness;
        watch.attempts.push(Attempt {
            harness: from,
            model: watch.model.clone(),
            model_source: watch.model_source.clone(),
            terminal: watch.terminal.clone(),
            why: why.clone(),
            did: vec![],
            screen,
        });
        watch.nudged = None;
        watch.session_started = now;
        let mut failures = vec![];
        for candidate in crate::health::watch_candidates()? {
            if watch.attempts.iter().any(|attempt| attempt.harness == candidate.harness) {
                continue;
            }
            watch.harness = candidate.harness;
            (watch.model, watch.model_source) = if candidate.harness == Harness::Codex {
                let (model, source) = crate::codex_launch::automatic_model(None)?;
                (model, Some(source.to_string()))
            } else {
                (candidate.model, None)
            };
            match start_session(&mut watch) {
                Ok((terminal, transcript)) => {
                    (watch.terminal, watch.transcript) = (terminal, transcript);
                    (self.say)(What::WatchContinued {
                        id: watch.id.clone(),
                        from: from.key().into(),
                        to: watch.harness.key().into(),
                        why,
                    });
                    state.running = Some(watch);
                    return Ok(());
                }
                Err(error) => {
                    failures.push(format!("{}: {error:#}", candidate.harness.key()));
                    if let Some(pending) = watch_terminal::active()? {
                        state.running = Some(pending);
                        state.unavailable = Some(watch_terminal::recovery(&failures.join("; ")));
                        return Ok(());
                    }
                }
            }
        }
        state.unavailable = Some(unstartable(&failures));
        let said = format!("{} gave out ({why}), and no harness can go on with it", from.key());
        self.finish_said(state, watch, "no_harness", &said);
        Ok(())
    }

    fn finish_said(&self, state: &mut State, mut watch: Watch, outcome: &str, said: &str) {
        watch.ended = Some(crate::clock());
        watch.outcome = Some(outcome.into());
        (self.say)(What::WatchEnded {
            id: watch.id.clone(),
            outcome: outcome.into(),
            said: said.into(),
        });
        state.last = Some(watch.clone());
        state.history.push(watch);
        let over = state.history.len().saturating_sub(HISTORY);
        state.history.drain(..over);
        state.running = None;
    }
}

/// Why no harness could start a watch, of each one tried.
fn unstartable(failures: &[String]) -> String {
    match failures {
        [] => "the host kit holds no harness free of a fault on record".into(),
        _ => failures.join("; "),
    }
}

/// Ends the watch's own terminal, and no other: the operator's directory holds
/// the operator's too. Its session is over once orca lists it no more.
fn close(worktree: &Worktree, terminal: &str, live: bool) -> Result<()> {
    if live {
        worktree.close_terminal(terminal)?;
        if worktree.live(terminal)? {
            bail!("the watch's terminal {terminal} is still live after it was closed");
        }
    }
    Ok(())
}

fn session_identity(watch: &Watch) -> String {
    format!("{}-{}", watch.id, watch.attempts.len() + 1)
}

/// Ends the session's uniquely named wrapper and its descendants. Its
/// terminal shell is above that wrapper and keeps its screen and input.
fn stop_session(watch: &Watch) -> Result<()> {
    // A pre-upgrade, headless session carried no marker and its terminal
    // was disposable. Keep its former termination behavior until it ends.
    if watch.session_started == 0 {
        let worktree = Worktree::at(watch.directory.clone(), String::new());
        return close(&worktree, &watch.terminal, worktree.live(&watch.terminal)?);
    }
    stop_marked(&session_identity(watch))?;
    watch_terminal::phase(watch, Phase::Ended)
}

fn stop_marked(identity: &str) -> Result<()> {
    let marker = format!("{}{identity}", host_watch::SESSION_PROCESS);
    let began = Instant::now();
    let mut pids = BTreeSet::new();
    loop {
        let processes = crate::hot::listed_checked()?;
        pids.retain(|pid| processes.iter().any(|proc| proc.pid == *pid));
        pids.extend(
            processes
                .iter()
                .filter(|proc| proc.line.split_whitespace().any(|word| word == marker))
                .map(|proc| proc.pid),
        );
        loop {
            let before = pids.len();
            for proc in &processes {
                if pids.contains(&proc.ppid) {
                    pids.insert(proc.pid);
                }
            }
            if before == pids.len() {
                break;
            }
        }
        if pids.is_empty() {
            return Ok(());
        }
        if began.elapsed() >= Duration::from_secs(5) {
            bail!("watch session {identity} still has processes; no replacement started");
        }
        let signal = if began.elapsed() >= Duration::from_secs(1) { "-KILL" } else { "-TERM" };
        for pid in &pids {
            let _ = crate::process::output(Command::new("kill").args([signal, &pid.to_string()]));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether the harness of a session gave out: its own words for a fault it
/// cannot go on past, or an exit with a failure.
fn gave_out(harness: Harness, transcript: &str, exited: Option<i32>) -> Option<Diagnosis> {
    let lines: Vec<String> = transcript.lines().map(str::to_string).collect();
    crate::health::terminal_fault(harness, &lines).or_else(|| {
        let status = exited.filter(|status| *status != 0)?;
        let mut evidence = host_watch::evidence(transcript);
        evidence.drain(..evidence.len().saturating_sub(3));
        evidence.push(format!("{} exited with status {status}", harness.key()));
        Some(crate::health::failed(evidence))
    })
}

/// Starts a session of `watch` on its harness: its brief written beside its
/// transcript, in the host's cache. Answers its terminal and transcript.
fn start_session(watch: &mut Watch) -> Result<(String, PathBuf)> {
    if watch.stock {
        std::fs::create_dir_all(&watch.directory).with_context(|| {
            format!("cannot make the stock watch's directory {}", watch.directory.display())
        })?;
        // Each harness finds the same instruction by its own rules filename.
        // Refresh it at each start, including after moving onto a new release.
        for file in [afkconvoy_core::team::RULES, "AGENTS.md"] {
            std::fs::write(watch.directory.join(file), stock_instruction())?;
        }
        crate::orca::watch_directory(&watch.directory)?;
    }
    let session = watch.attempts.len() + 1;
    let dir = cache()?.join("watch").join(&watch.id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot make the watch's directory {}", dir.display()))?;
    let brief_file = dir.join(format!("brief-{session}.md"));
    std::fs::write(&brief_file, brief(watch))
        .with_context(|| format!("cannot write the watch's brief {}", brief_file.display()))?;
    let transcript = dir.join(format!("session-{session}.log"));
    watch.transcript = transcript.clone();
    // A watch is started as a worker of an unbound repo is: the operator's
    // directory holds no config, and no role.
    let repo = Repo::new(watch.directory.clone());
    let launches = crate::launch::Launches::new(&repo, true)?;
    let command = launches.command(watch.harness)?;
    let command = if watch.harness == Harness::Codex {
        format!("{} --search", command.as_str())
    } else {
        command.as_str().to_string()
    };
    let kept = crate::kept::keep(watch.harness, true)?;
    let line = if watch.harness == Harness::Codex {
        crate::codex_launch::watch(
            &watch.directory,
            watch.model.as_deref(),
            watch.model_source.as_deref().unwrap_or("codex_default"),
            &command,
        )?
    } else {
        watch.harness.watch(
            command.as_str(),
            &watch.directory.display().to_string(),
            &kept.display().to_string(),
            watch.model.as_deref(),
        )
    };
    let line = launches.worker_line(watch.harness, &line);
    let line = format!("export {}=host; {line}", host_watch::WATCH_ENV);
    let line =
        host_watch::session(&line, &transcript.display().to_string(), &session_identity(watch));
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    // Persist intent before the non-idempotent external creation. An error
    // cannot authorize another create, even on another harness.
    watch_terminal::prepare(watch, &line)?;
    let line = watch_terminal::launch_line(watch, &line);
    match worktree.start_watch(&line, TITLE) {
        Ok(terminal) => {
            watch_terminal::created(watch, &terminal)?;
            watch.terminal = terminal;
        }
        Err(error) => {
            watch.terminal =
                watch_terminal::locate(watch)?.with_context(|| format!("{error:#}"))?;
        }
    }
    resume_delivery(watch)?;
    Ok((watch.terminal.clone(), transcript))
}

/// A Ready launch has not sent input. Sending is written first: a crash or
/// lost acknowledgement at that boundary must never duplicate a prompt.
fn resume_delivery(watch: &mut Watch) -> Result<()> {
    let Some(record) = watch_terminal::read()?.filter(|r| watch_terminal::matches(r, watch)) else {
        return Ok(()); // a watch retained from before launch journaling
    };
    match record.phase {
        Phase::Running | Phase::Ended => return Ok(()),
        Phase::Sending => bail!("the watch's first prompt has an uncertain receipt"),
        Phase::Creating => bail!("the watch's creation is not yet confirmed"),
        Phase::Ready => (),
    }
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    worktree.await_ready(&watch.terminal, 60, watch.harness, &record.command)?;
    watch_terminal::phase(watch, Phase::Sending)?;
    let brief = watch.transcript.with_file_name(format!("brief-{}.md", watch.attempts.len() + 1));
    worktree.tell_watch(&watch.terminal, &brief_pointer(&brief.display().to_string()))?;
    watch_terminal::phase(watch, Phase::Running)
}

/// What a session of `watch` is told.
fn brief(watch: &Watch) -> String {
    let why: Vec<String> =
        watch.items.iter().map(|item| format!("- `{}`: {}", item.id, item.reason)).collect();
    let said = |lines: &[String]| match lines {
        [] => "None.".to_string(),
        _ => lines.join("\n"),
    };
    let sessions: Vec<String> = watch
        .attempts
        .iter()
        .flat_map(|attempt| {
            let gave_out = format!(
                "A session on {} began this watch, and its harness gave out: {}. Its terminal evidence (not a report):",
                attempt.harness.key(),
                attempt.why
            );
            std::iter::once(gave_out).chain(attempt.screen.iter().map(|line| format!("    {line}")))
        })
        .collect();
    let report_file =
        crate::watch_report::directory(&watch.id).unwrap_or_default().join("report.json");
    let report_line = afkconvoy_core::line::Line::new()
        .fixed("afkconvoy watched --watch ")
        .word(&watch.id)
        .fixed(" --text-file ")
        .word(&report_file.display().to_string())
        .own();
    format!(
        "# A watch of this host\n\n\
         You are an afkconvoy **watch**: the operator on this host, started in the watch's \
         directory by the host runner, because its look found something an operator would act \
         on. This is an interactive session: a human may see and type to you. Carry on \
         without waiting for a human, and finish with your reporting command.\n\n\
         Find what holds this host's fleets and their work back, fix it — whatever that takes — \
         and check that the work goes on. You have what the operator at this host's keyboard \
         has, within these bounds (`afkconvoy run --help`): only what the fleets \
         of this host started or own; never the host runner, the user's programs, other apps \
         or orca; never delete what cannot be rebuilt, nor a change no commit holds; no force \
         push, no push to a base branch, no merge outside a landing turn. Follow this \
         directory's own rules for looking after the host's projects.\n\n\
         Your authority covers every repo on this host's checkout list, not just the repo of \
         the fault or this directory. `afkconvoy checkouts` gives the current list; the look \
         below names each checkout and its repo. Read the help of each command you need.\n\n\
         ## Check the resource condition after acting\n\n\
         For a hot worker, read its evidence, check its processes and worktree, and check \
         that it is under its limits or that its heavy work goes through `afkconvoy exclusive`. \
         You may end a fleet-owned runaway, type a line at its terminal, restart or switch it. \
         For admission, check `afkconvoy plan`'s `admission` admits again. For memory pressure, \
         check `sysctl -n kern.memorystatus_vm_pressure_level` is below 2: admission may already \
         admit, and proves nothing about pressure. The look carries the condition, how long \
         it held, the reading and its evidence.\n\n\
         ## Fix the host, then hand the issue back\n\n\
         You may trust a worktree for its harness; restart, nudge, continue or switch a worker; \
         or change a host setting. Read an escalation's reason and evidence before acting. \
         A fleet fault — a worker start that failed, a worktree trust gap or a harness fault — \
         is yours to fix on the host. Once its cause is gone, hand the escalated issue back \
         with its current `waiting_on_human.answer` line from `afkconvoy doctor`, in that \
         repo's checkout. Use the tracker and issue number that line names. Run \
         `afkconvoy wake <checkout>` after changing GitHub with `gh`, then check the next tick \
         actually dispatches or continues the issue and that its worker takes its brief. \
         Relabelling it alone is no evidence of recovery.\n\n\
         ## Answer escalations\n\n\
         Answer an escalation yourself, as the operator would: find out what is true, answer \
         the cause, keep to earlier decisions, and say what is to be done. For each issue in \
         `waiting_on_human`, read its reason and evidence, then run its current answer line \
         in that repo's checkout to hand it back, or use these answering commands:\n\n{}\n\n\
         Replace `<n>` with that issue's number. Use the tracker that checkout names and its \
         repo label when opening an issue; `afkconvoy doctor` gives the tracker and \
         `afkconvoy flows` gives the create line. A ruling or send carries no role or team \
         commits and no `--consult`: it moves the issue exactly as a human's answer does, \
         starting its rework counts over. Closing it leaves nothing to dispatch; changing \
         blocked-by changes what holds it back. Wake its checkout and verify the resulting \
         position and that work which can proceed does proceed. You may fix the cause on \
         the host or in a repo under the bounds above.\n\n\
         Leave only these four kinds with a human; never act on them, and list each with \
         its repo, issue, question and answer line in your report, which `doctor` \
         lists as what you did:\n\n\
         {HUMAN_KINDS}\n\n\
         ## Research the host kit's unplaced models\n\n\
         Take the host kit's unplaced models after anything holding work up. Read \
         `afkconvoy kit --help` for the research file's format and `afkconvoy doctor --all` \
         for the latest listed ids, names and research. Use your web tools to search primary \
         and high-trust sources: vendor model cards and release notes, SWE-bench, \
         Terminal-Bench and Aider. Research each pending identity once across harnesses; \
         use dated sources for its coding strengths, weaknesses and other names. Write a \
         JSON research file in preference order and run `afkconvoy kit --research <file>`. \
         A refusal names the fault: correct the file and run it again. Place what the \
         evidence supports, or settle an unsuitable listed identity with a reason and \
         dated sources in the format the help gives. Omitted models still await research, \
         and a harness with none placed stays on its default model. Check the command's \
         `placed`, `settled` and updated doctor answer; record what you placed or settled \
         and what remains unplaced.\n\n\
         ## Change any repo on the checkout list\n\n\
         When a lasting fix needs code, make it yourself, even if that repo's fleet cannot \
         start a worker. In the affected repo create a linked worktree on a branch of your \
         own from its configured base branch. Name its directory `{CHECKOUT}` or \
         `{CHECKOUT}-<digits>` so `afkconvoy land --watch` accepts it; never reuse \
         another worker's checkout. Work only in your own checkout of that \
         repo. Commit and push each completed piece before a long operation. Open its PR \
         against that repo's base branch, on no issue: no intake or flow label is needed, in \
         a bound repo too. Run `afkconvoy land --watch` there: it takes a turn to merge, \
         gate, record green and land its merge commit on the current base tip. \
         Resolve conflicts by merging, committing and asking again; never rebase. This is the watch's \
         landing path: its merge names a watch on this host and closes no issue. A \
         fleet lock standing lands nothing: record what holds it back. Keep the branch and \
         PR, and name the repo and PR in your record.\n\n\
         ## Cut an installed fix, then check it\n\n\
         Landing code does not update an installed tool. When the \
         fix must reach an installed tool — afkconvoy itself, for example — wait until the \
         change lands, then have that repo's fleet cut a version through its declared cut \
         steps (`afkconvoy cut --help`). Wake its checkout and read `afkconvoy plan`'s `cut`: \
         the next idle tick starts a due cut. Fix what holds that work back; do not bypass \
         the landing turn, tag by hand, or manufacture a cut record. If a cut failed for a \
         mechanical cause, fix it and continue with the `cut --from <step>` line its report \
         gives. If cutting is off or one of the four human kinds holds it back, say so.\n\n\
         Check that the cut published and installed the version containing your change. \
         For afkconvoy, let the host runner move onto it by itself; read `doctor --all`'s \
         `host.installed` for what holds the move back and `checkouts` for its version. Never \
         kill or restart the host runner. On the new version repeat the check that showed \
         the fault and check the fleets' work goes on. Record the version, the evidence and \
         whether the fault is gone or still present; an upgrade alone proves nothing.\n\n\
         This watch has one hour from {}, its sessions together; at the hour it is ended. End \
         with what held the work back, each thing you did and its evidence, each PR and cut, \
         and what you left — including every human decision — in your report. If landing, \
         cutting or the move has not finished, say what is still \
         pending; never report the fix verified before checking it.\n\n\
         ## Why it was started\n\n{}\n\n\
         ## What earlier watches on the same items did\n\n{}\n\n\
         ## What this watch did before this session\n\n{}\n\n\
         ## What the look answered\n\n`afkconvoy doctor --all --since <until of the look \
         before>` answered:\n\n```json\n{}\n```\n\n\
         ## Report, then stop\n\n\
         Write `{}` as a JSON object with all five keys: `held_back`, `did`, `verified`, \
         `human`, `pending`. The first three and `pending` are lists of nonempty strings; \
         `human` is a list of objects with nonempty `question` and `answer` strings, the \
         exact question left alone and its answer command. Use [] for an empty part. \
         Printed output is not a report. Your last step is this exact command; once it \
         records, the host runner ends your harness and keeps this terminal open:\n\n{}\n",
        answering(&watch.look),
        crate::log::utc(watch.started),
        why.join("\n"),
        said(&watch.earlier),
        said(&sessions),
        serde_json::to_string_pretty(&watch.look).unwrap_or_default(),
        report_file.display(),
        report_line,
    )
}

/// The shared answering commands for every waiting issue, with its tracker
/// and number fixed by the look. Its current answer also gives label swaps.
fn answering(look: &Value) -> String {
    use afkconvoy_core::config::RepoName;
    use afkconvoy_core::tracker::Tracker;
    let mut lines = vec![];
    for checkout in look["checkouts"].as_array().into_iter().flatten() {
        let doctor = &checkout["doctor"];
        let tracker = match &doctor["fleet"]["tracker"] {
            Value::Null => Tracker::Own,
            Value::String(named) => {
                let Some(tracker) = RepoName::read(named) else { continue };
                let Some(repo) = checkout["repo"].as_str().and_then(RepoName::read) else {
                    continue;
                };
                Tracker::Other { tracker, repo }
            }
            _ => continue,
        };
        for issue in doctor["waiting_on_human"].as_array().into_iter().flatten() {
            let Some(number) = issue["issue"].as_u64() else { continue };
            lines.push(format!(
                "In `{}`, issue {}:",
                checkout["checkout"].as_str().unwrap_or("unknown"),
                tracker.issue(number)
            ));
            if let Some(answer) = issue["answer"].as_str() {
                lines.push(format!("- Hand it back: `{answer}`"));
            }
            lines.extend(
                afkconvoy_core::waiting::answering(&tracker)
                    .iter()
                    .map(|line| format!("- `{}`", line.replace("<n>", &number.to_string()))),
            );
        }
    }
    if lines.is_empty() {
        "No waiting issue was listed; read `afkconvoy doctor` for current issues and their answer lines.".into()
    } else {
        lines.join("\n")
    }
}

/// What `afkconvoy doctor --all --since <until>` answers: the fleet's own
/// look, which runs no model. One with something wrong exits non-zero, and is
/// read all the same. One under way when the watch is `ended` is given up.
fn doctor_look(until: Option<u64>, ended: &AtomicBool) -> Result<Value> {
    let program = std::env::current_exe().context("cannot find this afkconvoy")?;
    let mut args = vec!["doctor".to_string(), "--all".to_string()];
    if let Some(until) = until {
        args.extend(["--since".to_string(), until.to_string()]);
    }
    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("the host's look could not be started")?;
    let mut stdout = child.stdout.take().context("the host's look has no stdout")?;
    let reading = std::thread::spawn(move || {
        let mut bytes = vec![];
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(LOOK_SECONDS);
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline || ended.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the host's look did not answer within {LOOK_SECONDS} seconds");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let bytes = reading.join().unwrap_or_default();
    let look: Value = serde_json::from_slice(&bytes).context("the host's look answered no JSON")?;
    if look["until"].is_null() && look["checkouts"].is_null() {
        bail!("the host's look failed: {}", look["error"]);
    }
    Ok(look)
}

fn push(items: &mut Vec<Item>, id: String, reason: String) {
    if !items.iter().any(|item| item.id == id) {
        items.push(Item::new(id, reason));
    }
}

fn findings(items: &mut Vec<Item>, at: &str, findings: &Value) {
    let unavailable = crate::doctor::Kind::WatchUnavailable.symptom();
    for finding in findings.as_array().into_iter().flatten() {
        let what = finding["what"].as_str().unwrap_or("a finding");
        // An `info` finding holds nothing up; that no watch can start is the
        // operator's to act on, not a watch's.
        if finding["severity"] == "info" || what.contains(unavailable) {
            continue;
        }
        let reason = format!("{what}; evidence: {}; fix: {}", finding["evidence"], finding["fix"]);
        let id = finding["watch_id"].as_str().map(str::to_string).unwrap_or_else(|| {
            if what.contains(crate::doctor::Kind::KitUnplaced.symptom()) {
                format!("{}{what}", host_watch::RESEARCH_ITEM)
            } else {
                format!("finding:{at}:{what}")
            }
        });
        push(items, id, reason);
    }
}

/// Everything in a look an operator would act on, each by an identity that
/// stays the same while its episode lasts.
fn actionable(look: &Value, now: u64) -> Vec<Item> {
    let mut items = vec![];
    findings(&mut items, "host", &look["findings"]);
    for checkout in look["checkouts"].as_array().into_iter().flatten() {
        let at = checkout["checkout"].as_str().unwrap_or("unknown");
        let doctor = &checkout["doctor"];
        findings(&mut items, at, &doctor["findings"]);
        for waiting in doctor["waiting_on_human"].as_array().into_iter().flatten() {
            let issue = waiting["issue"].as_u64().unwrap_or(0);
            let reason = format!(
                "#{issue} waits on a human: {}; answer: {}",
                waiting["why"], waiting["answer"]
            );
            push(&mut items, format!("human:{at}:#{issue}"), reason);
        }
        let fleet = &doctor["fleet"];
        if fleet["alive"] == false {
            let reason = format!("the fleet instance of {at} is not alive");
            push(&mut items, format!("fleet:{at}:not-alive"), reason);
        }
        if !fleet["broken"].is_null() {
            let since = fleet["broken"]["since"].as_u64().unwrap_or(0);
            let reason =
                format!("the fleet instance of {at} is broken: {}", fleet["broken"]["why"]);
            push(&mut items, format!("fleet:{at}:broken:{since}"), reason);
        }
        // A tick that fails the same way again is the same episode.
        let ticks = doctor["watching"]["ticks"].as_array().into_iter().flatten();
        for tick in ticks.filter(|tick| tick["failed"] == true) {
            let instance = tick["instance"].as_str().unwrap_or("unknown");
            let errors = &tick["errors"];
            let how = match errors.as_array().is_some_and(|errors| !errors.is_empty()) {
                true => errors.to_string(),
                false => tick["line"].as_str().unwrap_or("").to_string(),
            };
            let reason = format!("a tick of {instance} failed: {}; errors: {errors}", tick["line"]);
            push(&mut items, format!("tick:{at}:{instance}:{how}"), reason);
        }
        for failed in fleet["failed_steps"].as_array().into_iter().flatten() {
            let step = failed["do"].as_str().unwrap_or("tick");
            let reason = format!(
                "`{step}` failed in tick {} and has not recovered: {}",
                failed["tick"], failed["error"]
            );
            push(&mut items, format!("step:{at}:{step}"), reason);
        }
    }
    let health = &look["host"]["host_kit"]["worker_health"];
    for worker in health["workers"].as_array().into_iter().flatten() {
        let next = worker["next_check"].as_u64().unwrap_or(u64::MAX);
        let path = worker["path"].as_str().unwrap_or("unknown");
        if next <= now {
            let reason = format!(
                "the health observation of the worker in {path} was due at {} UTC and is still \
                 waiting: {}",
                crate::log::utc(next),
                worker["error"]
            );
            push(&mut items, format!("health:{path}"), reason);
        }
    }
    for failed in health["observations_failed"].as_array().into_iter().flatten() {
        let next = failed[2].as_u64().unwrap_or(u64::MAX);
        let path = failed[0].as_str().unwrap_or("unknown");
        if next <= now {
            let reason = format!(
                "the health observation of the worker in {path} failed, and its retry is overdue: {}",
                failed[1]
            );
            push(&mut items, format!("health:{path}"), reason);
        }
    }
    items
}

/// What `doctor` answers of the host's watch: the one running, the last that
/// ended, and why none could start or go on, where none could.
pub fn json(running: Option<&Watch>) -> Value {
    let state = read();
    let (last, unavailable) = match &state {
        Ok(state) => (json!(state.last.as_ref().map(watch_json)), json!(state.unavailable)),
        Err(error) => (Value::Null, json!(format!("{error:#}"))),
    };
    let mut answer =
        json!({"running": running.map(watch_json), "last": last, "unavailable": unavailable});
    answer["association"] = watch_terminal::json();
    match crate::host_settings::of_host()
        .and_then(|settings| Ok((directory(&settings)?, settings.watch_dir.is_none())))
    {
        Ok((directory, stock)) => {
            answer["directory"] = json!(directory);
            answer["stock"] = json!(stock);
        }
        Err(error) => answer["error"] = json!(format!("{error:#}")),
    }
    answer
}

/// The host's finding where a look called for a watch no harness could start.
pub fn finding() -> Option<crate::doctor::Finding> {
    let why = read().ok().and_then(|state| state.unavailable).or_else(|| {
        watch_terminal::read().err().map(|error| watch_terminal::recovery(&format!("{error:#}")))
    })?;
    Some(crate::doctor::finding(
        crate::doctor::Kind::WatchUnavailable,
        "no harness can start this host's watch".into(),
        why,
        "afkconvoy run --help (WATCH RECOVERY); afkconvoy kit".into(),
    ))
}

/// Says `what` in every listed checkout's tick log: a log that cannot take it
/// loses that line, never the watch.
pub fn append_to_tick_logs(list: &List, what: &What) {
    let entry = Entry {
        at: crate::clock(),
        instance: INSTANCE.into(),
        host: crate::this_host(),
        what: what.clone(),
    };
    for checkout in list.read().map(|listed| listed.checkouts).unwrap_or_default() {
        if let Ok(repo) = Repo::at(&checkout.checkout)
            && let Ok(log) = crate::log::Log::of(&repo)
        {
            let _ = log.append(&entry);
        }
    }
}

/// The instance the watch's lines are said by, in every log.
pub const INSTANCE: &str = "host-watch";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ending_a_session_keeps_its_terminal_shell_and_the_operators_process() {
        use afkconvoy_core::line::Line;
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let kept = dir.path().join("kept");
        let exited = dir.path().join("exited");
        let identity = format!("test-{}", crate::unique());
        let harness = Line::new()
            .fixed("printf ready > ")
            .word(ready.to_str().unwrap())
            .fixed("; exec sleep 30")
            .own();
        let line = host_watch::session(&harness, exited.to_str().unwrap(), &identity);
        let line = format!(
            "{line}; {}",
            Line::new()
                .fixed("printf kept > ")
                .word(kept.to_str().unwrap())
                .fixed("; exec sleep 30")
                .own()
        );
        let mut terminal = Command::new("sh").args(["-c", &line]).spawn().unwrap();
        let mut operator = Command::new("sleep").arg("30").current_dir(dir.path()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "harness never started");
            std::thread::sleep(Duration::from_millis(10));
        }
        let stopped = stop_marked(&identity);
        while !kept.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let terminal_alive = terminal.try_wait().unwrap().is_none();
        let operator_alive = operator.try_wait().unwrap().is_none();
        let _ = terminal.kill();
        let _ = operator.kill();
        let _ = terminal.wait();
        let _ = operator.wait();
        stopped.unwrap();
        assert!(kept.exists(), "terminal shell did not continue");
        assert!(terminal_alive && operator_alive, "ended an unrelated process");
        assert!(host_watch::exited(&std::fs::read_to_string(exited).unwrap()).is_some());
    }

    #[test]
    fn each_waiting_issue_gets_answering_commands_for_its_own_tracker() {
        let look = json!({"checkouts": [
            {"checkout": "/one", "repo": "acme/one", "doctor": {
                "fleet": {"tracker": "acme/tracker"}, "waiting_on_human": [
                    {"issue": 7, "answer": "afkconvoy rule --issue 7 --text-file <file>"}]}},
            {"checkout": "/two", "repo": "acme/two", "doctor": {
                "fleet": {"tracker": null}, "waiting_on_human": [
                    {"issue": 8, "answer": "gh issue edit 8 --add-label ready --remove-label human"}]}}
        ]});
        let commands = answering(&look);
        for expected in [
            "In `/one`, issue acme/tracker#7:",
            "gh issue close 7 --repo acme/tracker --comment <why>",
            "afkconvoy send --issue 7 --stage <stage> --text-file <file>",
            "gh api repos/acme/tracker/issues/7/dependencies/blocked_by -F issue_id=<id>",
            "--label repo:acme/one",
            "In `/two`, issue #8:",
            "gh issue close 8 --comment <why>",
            "gh issue edit 8 --add-label ready --remove-label human",
        ] {
            assert!(commands.contains(expected), "{expected}: {commands}");
        }
        assert!(!commands.contains("<n>"));
        assert!(!commands.contains("close 8 --repo acme/tracker"));
    }

    #[test]
    fn a_quiet_look_has_nothing_to_act_on() {
        let look = json!({"findings": [], "checkouts": [{"checkout": "/r", "doctor": {
            "findings": [], "waiting_on_human": [], "fleet": {"alive": true, "broken": null,
            "failed_steps": []}, "watching": {"ticks": [{"instance": "a", "failed": false}]}}}],
            "host": {"host_kit": {"worker_health": {"workers": [{"path": "/w", "next_check": 2000}]}}}});
        assert!(actionable(&look, 1000).is_empty());
    }

    #[test]
    fn every_actionable_kind_has_an_identity_that_holds_through_its_episode() {
        let look = |first: u64, next: u64| {
            json!({"findings": [{"what": "orca is not running", "evidence": "down", "fix": "orca open"},
                {"what": "no harness can start this host's watch", "evidence": "", "fix": ""},
                {"severity": "info", "what": "a key is read and changes nothing"}],
            "host": {"host_kit": {"worker_health": {"workers": [{"path": "/w", "next_check": next, "error": "stuck"}]}}},
            "checkouts": [{"checkout": "/r", "doctor": {
            "findings": [], "waiting_on_human": [{"issue": 7, "why": "escalated", "answer": "gh issue view 7"}],
            "fleet": {"alive": false, "broken": {"since": 800, "why": "panic"},
                "failed_steps": [{"tick": first, "do": "dispatch", "error": "no"}]},
            "watching": {"ticks": [{"instance": "alpha", "first": first, "failed": true,
                "line": "FAILED", "errors": [{"error": "gh is down"}]}]}}}]})
        };
        let ids = |look: &Value| -> Vec<String> {
            actionable(look, 1000).into_iter().map(|item| item.id).collect()
        };
        let first = ids(&look(850, 900));
        assert_eq!(
            first,
            [
                "finding:host:orca is not running",
                "human:/r:#7",
                "fleet:/r:not-alive",
                "fleet:/r:broken:800",
                r#"tick:/r:alpha:[{"error":"gh is down"}]"#,
                "step:/r:dispatch",
                "health:/w",
            ]
        );
        assert_eq!(ids(&look(950, 990)), first, "the same failure later is the same episode");
        assert!(!ids(&look(850, 1001)).contains(&"health:/w".to_string()), "not due yet");
    }

    #[test]
    fn a_harness_gives_out_by_its_own_words_or_by_a_failed_exit() {
        let codex = "checked #7\nUsage limit reached; increase your limits to continue using Codex";
        let fault = gave_out(Harness::Codex, codex, None).unwrap();
        assert_eq!(fault.class, "quota");
        let fault = gave_out(Harness::ClaudeCode, "API Error: 500\n", Some(1)).unwrap();
        assert!(fault.evidence.iter().any(|said| said.contains("API Error: 500")), "{fault:?}");
        assert!(fault.evidence.iter().any(|said| said.contains("exited with status 1")));
        assert!(gave_out(Harness::ClaudeCode, "answered #7\n", Some(0)).is_none());
        assert!(gave_out(Harness::ClaudeCode, "working\n", None).is_none());
    }
}

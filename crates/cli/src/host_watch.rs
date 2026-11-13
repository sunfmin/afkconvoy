//! The host runner's resident watch (docs/adr/0040). Deterministic looks
//! queue episodes while independent bounded rounds execute in one interactive
//! session. Reports complete rounds and leave a healthy executor at input.
//! The cache owns the queue, coverage, round, delivery and session identities;
//! neither a restart nor an Agent's conversation becomes their authority.

use std::collections::{BTreeMap, BTreeSet};
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
use crate::watch_terminal::{self, LaunchState, TITLE};

const STATE: &str = "watch/state.json";
/// How often a running watch's terminal and transcript are read.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// How long a look may take before it is given up, and taken again later.
const LOOK_SECONDS: u64 = 300;
/// How many ended watches are kept, for what earlier watches on the same
/// items did.
const HISTORY: usize = 20;
/// What a distillation's item identity begins with, as its brief names it.
const DISTILL: &str = afkconvoy_core::distill::ITEM;
const FAILED_CUT: &str = afkconvoy_core::cut::FAILED_LABEL;
/// The linked checkout a watch makes for a change of a listed repo.
pub const CHECKOUT: &str = "afkconvoy-watch";

/// The four kinds every watch leaves with a human, in either directory.
pub(crate) const HUMAN_KINDS: &str = "\
- spending money, or needing an account, a credential or a person's hands;\n\
- what cannot be undone;\n\
- taking a secret or private data off the host;\n\
- changing what a product is for, or who it is for.";

/// What a watch, and the asking session acting on the operator's words, may
/// touch: one home for both briefs.
pub(crate) const BOUNDS: &str = "only what the fleets of this host started or own; never the \
host runner, the user's programs, other apps or orca; never delete what cannot be rebuilt, nor \
a change no commit holds; no force push, no push to a base branch, no merge outside a landing turn";

/// The stock watch's whole instruction, kept as its directory's rules.
fn stock_instruction() -> String {
    format!(
        "\
# The stock watch

Find what holds this host's fleets and their work back, fix it with the watch's authority,
check that the work goes on, and say what you did.
A resident session whose brief names it “afkconvoy asks” is not the stock watch:
it runs no watch round, takes no look and reports nothing through `afkconvoy watched`;
it asks the human questions its brief carries and carries out what the operator tells it there.
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

/// One watch round, including what a continuation needs. The resident keeps
/// its latest round's executor metadata to start another round in that session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watch {
    pub id: String,
    /// One round belongs to a resident executor; this identity stays fixed
    /// when that executor receives another round. Empty for older runners.
    #[serde(default)]
    pub session: String,
    /// Episode identities, aligned with items. Unlike a finding's identity,
    /// these change after an observed recovery, even during an active round.
    #[serde(default)]
    pub episodes: Vec<String>,
    pub directory: PathBuf,
    /// Whether it started under afkconvoy's own instruction. Kept with the
    /// watch so a later host-setting change cannot change a continuation.
    #[serde(default)]
    pub stock: bool,
    /// What the look that started it answered.
    pub look: Value,
    /// Every condition's episode at this round's look, including a human
    /// wait already covered by an earlier report.
    #[serde(default)]
    pub conditions: BTreeMap<String, String>,
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
    #[serde(default)]
    pub tokens: Option<afkconvoy_core::account::Tokens>,
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
    json!({"id": watch.id, "session": session_identity(watch), "episodes": watch.episodes,
        "directory": watch.directory, "stock": watch.stock, "items": watch.items,
        "conditions": watch.conditions, "started": watch.started, "harness": watch.harness, "model": watch.model,
        "model_source": watch.model_source,
        "terminal": watch.terminal, "transcript": watch.transcript, "did": watch.did,
        "reported": watch.report.is_some(), "report": watch.report,
        "delivery": watch_terminal::delivery_json(watch),
        "deadline": (watch.started != 0).then(|| watch.started.saturating_add(host_watch::TIME_LIMIT)),
        "attempts": watch.attempts, "ended": watch.ended, "outcome": watch.outcome,
        "tokens": watch.tokens})
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// The `until` of the last look: where the next starts.
    until: Option<u64>,
    /// Raw coverage retained for older-state migration. Once episodes are
    /// initialized, `covered` holds the authoritative episode coverage.
    watched: Vec<String>,
    /// Last successful start that took kit research, retained across restarts.
    research_started: Option<u64>,
    /// When it last distilled the daybook, and the last day it covered.
    distilling: crate::distill::Distilling,
    /// A resident session is independent of the round currently executing.
    resident: Option<Watch>,
    /// Current observation's episode identities and durable delivery queue.
    observed: BTreeMap<String, String>,
    episodes_initialized: bool,
    covered: Vec<String>,
    pending: Vec<Queued>,
    look: Value,
    looked: Option<u64>,
    running: Option<Watch>,
    last: Option<Watch>,
    history: Vec<Watch>,
    /// Why the last watch a look called for could not start or go on.
    unavailable: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Queued {
    item: Item,
    episode: String,
}

/// Serializes round transitions with the report command. No read/validate/
/// write race may accept a report after a timeout or against another round.
pub(crate) fn state_lock() -> Result<std::fs::File> {
    let path = cache()?.join("watch/state.lock");
    std::fs::create_dir_all(path.parent().expect("watch state parent"))?;
    let lock = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(path)?;
    lock.lock()?;
    Ok(lock)
}

impl State {
    /// Refresh the queue from deterministic observations. The active round
    /// reserves only its own episodes; a recovery and return can queue a new
    /// episode of that same finding while the old round is still executing.
    fn observe(&mut self, look: Value, now: u64) {
        let current = actionable(&look);
        let mut present: BTreeSet<String> = current.iter().map(|i| i.id.clone()).collect();
        for resource in look["host"]["resources"].as_array().into_iter().flatten() {
            if let Some(id) = resource["id"].as_str() {
                present.insert(id.into());
            }
        }
        // Migrate old coverage before adding new episode identities.
        let legacy = !self.episodes_initialized;
        if legacy && let Some(round) = self.running.as_ref().or(self.resident.as_ref()) {
            for (item, episode) in round.items.iter().zip(&round.episodes) {
                self.observed.insert(item.id.clone(), episode.clone());
                if round.report.is_some() {
                    self.covered.push(episode.clone());
                }
            }
        }
        self.observed.retain(|id, _| present.contains(id));
        for id in &present {
            let episode =
                self.observed.entry(id.clone()).or_insert_with(|| uuid::Uuid::new_v4().to_string());
            if legacy && self.watched.contains(id) {
                self.covered.push(episode.clone());
            }
        }
        if let Some(round) = &mut self.running
            && round.episodes.is_empty()
        {
            round.episodes =
                round.items.iter().filter_map(|i| self.observed.get(&i.id).cloned()).collect();
        }
        self.covered.retain(|episode| self.observed.values().any(|id| id == episode));
        self.watched.retain(|id| present.contains(id));
        // Research keeps its daily bound, and joins other pending work.
        let watched: Vec<String> = current
            .iter()
            .filter(|item| {
                let episode = self.observed.get(&item.id);
                episode.is_some_and(|episode| {
                    self.covered.contains(episode)
                        || self.running.as_ref().is_some_and(|r| r.episodes.contains(episode))
                })
            })
            .map(|i| i.id.clone())
            .collect();
        let due = host_watch::pending(&current, &watched, false, self.research_started, now);
        self.pending = due
            .into_iter()
            .filter_map(|item| {
                let episode = self.observed.get(&item.id)?.clone();
                if self.running.as_ref().is_some_and(|r| r.episodes.contains(&episode)) {
                    return None;
                }
                Some(Queued { item, episode })
            })
            .collect();
        self.until = look["until"].as_u64().or(self.until);
        // A quiet look needs only its interval and episode state. Do not
        // retain unrelated operator launch configuration in an idle cache;
        // the complete look belongs only to work awaiting a brief.
        self.look = if self.pending.is_empty() { Value::Null } else { look };
        self.looked = Some(now);
        self.episodes_initialized = true;
    }

    /// A start reserves its episodes while the watch runs; only a report
    /// keeps them covered afterwards. Reconcile from retained outcomes so a
    /// restart also repairs coverage left by an older runner. A later report
    /// on the same item wins over an earlier unsuccessful watch.
    fn retry_unreported(&mut self) {
        let mut seen = BTreeSet::new();
        let mut research_seen = self
            .running
            .as_ref()
            .is_some_and(|r| r.items.iter().any(|i| i.id.starts_with(host_watch::RESEARCH_ITEM)));
        let mut distill_seen =
            self.running.as_ref().is_some_and(|round| crate::distill::takes(&round.items));
        for watch in self.last.iter().chain(self.history.iter().rev()) {
            for item in &watch.items {
                if seen.insert(&item.id) && watch.report.is_none() {
                    self.watched.retain(|id| id != &item.id);
                }
            }
            if !research_seen
                && watch.items.iter().any(|item| item.id.starts_with(host_watch::RESEARCH_ITEM))
            {
                research_seen = true;
                if watch.report.is_none() && self.research_started == Some(watch.started) {
                    self.research_started = None;
                }
            }
            if !distill_seen && crate::distill::takes(&watch.items) {
                distill_seen = true;
                if watch.report.is_none() {
                    self.distilling.unreported(watch.started);
                }
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

fn write(state: &State) -> Result<()> {
    let file = cache()?.join(STATE);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(temporary, &file)?;
    Ok(())
}

/// `afkconvoy daybook --distill`: asks the host's watch to distill the daybook
/// at its next look, without waiting out its week → {asked, at}.
pub fn ask_distillation(now: u64) -> Result<Value> {
    let _lock = state_lock()?;
    let mut state = read()?;
    state.distilling.asked = Some(now);
    write(&state)?;
    Ok(json!({"asked": true, "at": now}))
}

pub(crate) fn running() -> Result<Option<Watch>> {
    let state = read()?;
    if let Some(record) = watch_terminal::read()?
        && !matches!(record.status, LaunchState::Idle | LaunchState::Ended)
        && state.running.as_ref().is_none_or(|round| round.id != record.watch.id)
    {
        return Ok(Some(record.watch));
    }
    Ok(state.running)
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
    sender: Option<std::thread::JoinHandle<()>>,
    asking: Option<std::thread::JoinHandle<()>>,
    daybook: Option<std::thread::JoinHandle<()>>,
}

impl HostWatch {
    /// Watches the host, saying each start, end and thing done to `say`.
    pub fn start(
        shared: Shared,
        panel: Option<String>,
        say: impl Fn(What) + Send + Sync + 'static,
    ) -> HostWatch {
        let ended = Arc::new(AtomicBool::new(false));
        let say = Arc::new(say);
        let sender_ended = Arc::clone(&ended);
        let sender_say = Arc::clone(&say);
        let sender = std::thread::spawn(move || {
            crate::watch_notify::run(&sender_ended, panel.as_deref(), |said| {
                sender_say(What::WatchFailed { said })
            });
        });
        let daybook_ended = Arc::clone(&ended);
        let daybook_say = Arc::clone(&say);
        let daybook = std::thread::spawn(move || {
            crate::watch_daybook::run(&daybook_ended, |said| {
                daybook_say(What::WatchFailed { said });
            });
        });
        let asking_ended = Arc::clone(&ended);
        let asking_say = Arc::clone(&say);
        let asking = std::thread::spawn(move || {
            crate::host_asking::run(&asking_ended, |said| {
                asking_say(What::WatchFailed { said });
            });
        });
        let watcher = Watcher {
            ended: Arc::clone(&ended),
            shared,
            say: Box::new(move |what| say(what)),
            looking: Failing::default(),
            checking: Failing::default(),
        };
        HostWatch {
            ended,
            thread: Some(std::thread::spawn(move || watcher.run())),
            sender: Some(sender),
            asking: Some(asking),
            daybook: Some(daybook),
        }
    }
}

impl Drop for HostWatch {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(sender) = self.sender.take() {
            let _ = sender.join();
        }
        if let Some(asking) = self.asking.take() {
            let _ = asking.join();
        }
        if let Some(daybook) = self.daybook.take() {
            let _ = daybook.join();
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
        let mut next_beat = Instant::now();
        while !self.ended.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= next_beat {
                next_beat = now + Duration::from_secs(60);
                if let Err(error) = crate::human_wait::heartbeat() {
                    (self.say)(What::WatchFailed {
                        said: format!("human wait heartbeat failed: {error:#}"),
                    });
                }
            }
            let checking = now >= next_check;
            if checking {
                next_check = now + CHECK_EVERY;
                let checked = self.check();
                self.checking.said(checked, &self.say);
            }
            let queued = checking
                && read().is_ok_and(|s| {
                    s.running.is_none() && !s.pending.is_empty() && s.unavailable.is_none()
                });
            if now >= next_look || queued {
                next_look = now + CHECK_EVERY;
                let looked = crate::host_settings::of_host().and_then(|settings| {
                    let result = self.look(&settings, queued);
                    let due = read()?
                        .looked
                        .unwrap_or_else(crate::clock)
                        .saturating_add(settings.watch_every());
                    next_look = Instant::now()
                        + Duration::from_secs(due.saturating_sub(crate::clock()).max(2));
                    result
                });
                self.looking.said(looked, &self.say);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn save(&self, state: &State) -> Result<()> {
        write(state)?;
        self.shared.set(state.running.clone());
        Ok(())
    }

    /// One look at the host, and the watch it calls for.
    fn look(&self, settings: &crate::host_settings::Settings, revalidate: bool) -> Result<()> {
        let _lock = state_lock()?;
        let mut state = read()?;
        if !revalidate
            && state
                .looked
                .is_some_and(|last| crate::clock().saturating_sub(last) < settings.watch_every())
        {
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
        state.retry_unreported();
        let mut look = doctor_look(state.until, &self.ended)?;
        crate::resources::looked(&look)?;
        crate::unworked::reconcile(&mut look, &crate::this_host(), crate::clock())?;
        crate::human_wait::take_look(&mut look)?;
        crate::distill::look(&mut look, &mut state.distilling, crate::clock());
        state.observe(look, crate::clock());
        self.say_state(&state);
        // Commit the observation interval and queue before any launch or
        // delivery. A restart cannot consume a look without retaining work.
        self.save(&state)?;
        crate::watch_notify::looked(&state.observed)?;
        if state.running.is_none() && !state.pending.is_empty() {
            let pending: Vec<Item> = state.pending.iter().map(|q| q.item.clone()).collect();
            crate::resources::evidence(&mut state.look, &pending)?;
            // Disk evidence can take minutes. A condition that recovered
            // while it was taken must not start a watch from an old look.
            // Only whether each is active counts here, which no holder changes.
            let resources =
                crate::resources::json(crate::clock(), &crate::admission::Readings::new(), None)?;
            state.pending.retain(|q| {
                !q.item.id.starts_with("resource:")
                    || resources
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|resource| resource["id"] == q.item.id && resource["active"] == true)
            });
            // A host-wide condition names no checkout. Its evidence is read
            // at the first checkout on the list, and a look that ran before
            // any `run` handed one over has nowhere to read it. That
            // condition waits for the next look.
            let evidenced = |id: &str| {
                state.look["host"]["resources"].as_array().is_some_and(|rows| {
                    rows.iter().any(|row| row["id"] == id && row["evidence"].as_str().is_some())
                })
            };
            state.pending.retain(|q| !q.item.id.starts_with("resource:") || evidenced(&q.item.id));
            if !state.pending.is_empty() {
                let look = state.look.clone();
                let items = state.pending.iter().map(|q| q.item.clone()).collect();
                self.start(&mut state, &directory, settings.watch_dir.is_none(), look, items)?;
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
        if let Some(resident) = &mut state.resident {
            if resident.directory != directory {
                // A changed watch directory explicitly resets its session.
                stop_session(resident)?;
                let worktree = Worktree::at(resident.directory.clone(), String::new());
                close(&worktree, &resident.terminal, worktree.live(&resident.terminal)?)?;
                state.resident = None;
                self.save(state)?;
            } else {
                let worktree = Worktree::at(resident.directory.clone(), String::new());
                resident.terminal = watch_terminal::locate(resident)?.context(
                    "resident Terminal is missing; wait for its executor termination check",
                )?;
                if !worktree.watch_input_ready(&resident.terminal)? {
                    return Ok(());
                }
            }
        }
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
            session: String::new(),
            episodes: state.pending.iter().map(|q| q.episode.clone()).collect(),
            directory: directory.to_path_buf(),
            stock,
            look,
            conditions: state.observed.clone(),
            items,
            started: 0,
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
            tokens: None,
        };
        if let Some(resident) = &state.resident {
            watch.session = session_identity(resident);
            watch.harness = resident.harness;
            watch.model = resident.model.clone();
            watch.model_source = resident.model_source.clone();
            watch.terminal = resident.terminal.clone();
            watch.transcript = resident.transcript.clone();
            watch.session_started = resident.session_started;
            write_brief(&watch)?;
            // The journal is authoritative across a crash between this
            // intent and the progress file. It retains the new round ID.
            watch_terminal::begin_round(&watch)?;
            state.running = Some(watch.clone());
            state.pending.clear();
            self.save(state)?;
            resume_delivery(&mut watch)?;
            self.round_started(state, watch)?;
            return Ok(());
        }
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
            (watch.model, watch.model_source) =
                crate::codex_launch::watch_selection(candidate.harness, candidate.model)?;
            match start_session(&mut watch) {
                Ok((terminal, transcript)) => {
                    (watch.terminal, watch.transcript) = (terminal, transcript);
                    self.round_started(state, watch)?;
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

    fn round_started(&self, state: &mut State, watch: Watch) -> Result<()> {
        state.watched.extend(watch.items.iter().map(|item| item.id.clone()));
        state.watched.sort();
        state.watched.dedup();
        if watch.items.iter().any(|item| item.id.starts_with(host_watch::RESEARCH_ITEM)) {
            state.research_started = Some(watch.started);
        }
        state.distilling.started(&watch.items, watch.started);
        state.pending.clear();
        state.unavailable = None;
        (self.say)(What::WatchStarted {
            id: watch.id.clone(),
            directory: watch.directory.display().to_string(),
            harness: watch.harness.key().to_string(),
            reasons: watch.items.iter().map(|item| item.reason.clone()).collect(),
        });
        state.resident = Some(watch.clone());
        state.running = Some(watch);
        self.say_state(state);
        self.save(state)
    }

    fn say_state(&self, state: &State) {
        let session = state.running.as_ref().or(state.resident.as_ref());
        (self.say)(What::WatchState {
            session: session.map(session_identity),
            round: state.running.as_ref().map(|r| r.id.clone()),
            terminal: session.map(|r| r.terminal.clone()),
            status: if state.unavailable.is_some() {
                "unavailable"
            } else if state.running.is_some() {
                "active"
            } else if state.resident.is_some() {
                "standby"
            } else {
                "absent"
            }
            .into(),
            pending: state.pending.iter().map(|q| format!("{}:{}", q.item.id, q.episode)).collect(),
        });
    }

    /// Reads the running watch, and ends or continues it where its session
    /// is over, its harness gave out or its hour is up.
    fn check(&self) -> Result<()> {
        let _lock = state_lock()?;
        let mut state = read()?;
        let kept = state.running.take();
        let record = watch_terminal::read()?;
        let active = record
            .as_ref()
            .filter(|r| !matches!(r.status, LaunchState::Idle | LaunchState::Ended))
            .map(|r| r.watch.clone());
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
            // Reconstruct an idle resident from its launch journal too.
            if let Some(record) = record.filter(|r| r.status == LaunchState::Idle) {
                state.resident = Some(record.watch);
            }
            if let Some(mut resident) = state.resident.take() {
                let worktree = Worktree::at(resident.directory.clone(), String::new());
                match watch_terminal::locate(&resident) {
                    Ok(Some(terminal)) => {
                        resident.terminal = terminal;
                        let text =
                            std::fs::read_to_string(&resident.transcript).unwrap_or_default();
                        let screen =
                            worktree.screen(&resident.terminal, host_watch::EVIDENCE_LINES)?;
                        let (_, runtime) = worktree.terminal_health(
                            &resident.terminal,
                            resident.harness,
                            crate::clock(),
                        )?;
                        let fault =
                            crate::health::watch_diagnosis(resident.harness, &screen, &runtime)
                                .or_else(|| {
                                    gave_out(resident.harness, &text, host_watch::exited(&text))
                                });
                        if let Some(fault) = fault {
                            stop_session(&resident)?;
                            crate::health::watch_fault(resident.harness, &fault, crate::clock())?;
                            close(&worktree, &resident.terminal, true)?;
                        } else if host_watch::exited(&text).is_some() {
                            stop_session(&resident)?;
                        } else {
                            state.resident = Some(resident);
                        }
                        state.unavailable = None;
                    }
                    Ok(None) => {
                        // Its terminal is gone or was re-incarnated and
                        // retired: the next look may start a fresh watch.
                        stop_session(&resident)?;
                        state.unavailable = None;
                    }
                    Err(error) => {
                        let why = watch_terminal::recovery(&format!("{error:#}"));
                        watch_terminal::problem(&why)?;
                        state.unavailable = Some(why);
                        state.resident = Some(resident);
                    }
                }
            }
            self.shared.set(None);
            return self.save(&state);
        };
        for item in &watch.items {
            if !state.watched.contains(&item.id) {
                state.watched.push(item.id.clone());
            }
            if item.id.starts_with(host_watch::RESEARCH_ITEM) {
                state.research_started = Some(watch.started);
            }
        }
        state.distilling.started(&watch.items, watch.started);
        let worktree = Worktree::at(watch.directory.clone(), String::new());
        let transcript = std::fs::read_to_string(&watch.transcript).unwrap_or_default();
        let exited = host_watch::exited(&transcript);
        let now = crate::clock();
        if let Some(report) = crate::watch_report::read(&watch.id)? {
            if let Err(error) = crate::watch_notify::reported(&watch, &report, &state.observed) {
                // Reports accepted before the condition field existed may
                // be ambiguous. Never guess from their words or hold a
                // completed round open for the sender's cache error.
                (self.say)(What::WatchFailed {
                    said: format!("human decision report not queued: {error:#}"),
                });
            }
            // The daybook is written later, by its own thread; a failure to
            // queue it never holds the round open.
            if let Err(error) = crate::watch_daybook::reported(&watch, &report) {
                (self.say)(What::WatchFailed {
                    said: format!("daybook entry not queued: {error:#}"),
                });
            }
            // Pressure from another account is left with the operator now.
            if let Err(error) = crate::resources::reported(&watch) {
                (self.say)(What::WatchFailed {
                    said: format!("pressure left with the operator not kept: {error:#}"),
                });
            }
            // A fast or explicitly recovered report can precede Orca's
            // execution receipt. It still settles this exact round; never
            // retain a zero research reservation that is already daily-due.
            if watch.started == 0 {
                watch.started = watch_terminal::read()?
                    .filter(|r| r.watch.id == watch.id)
                    .and_then(|r| r.offered)
                    .filter(|started| *started != 0)
                    .unwrap_or(now);
            }
            if watch.items.iter().any(|item| item.id.starts_with(host_watch::RESEARCH_ITEM)) {
                state.research_started = Some(watch.started);
            }
            state.distilling.reported(&watch.items, watch.started);
            state.unavailable = None;
            for did in &report.did {
                (self.say)(What::WatchDid { id: watch.id.clone(), did: did.clone() });
            }
            watch.did = report.did.clone();
            watch.report = Some(report);
            state.covered.extend(watch.episodes.iter().cloned());
            state.covered.sort();
            state.covered.dedup();
            state.resident = Some(watch.clone());
            watch_terminal::set_status(&watch, LaunchState::Idle)?;
            self.finish_said(
                &mut state,
                watch,
                "ended",
                "the round reported; its resident session awaits input",
            )?;
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
                    if watch.started != 0 && host_watch::expired(watch.started, now) {
                        stop_session(&watch)?;
                        self.finish_said(
                            &mut state,
                            watch,
                            "time_limit",
                            "the active round expired while delivery recovery was paused",
                        )?;
                        return self.save(&state);
                    }
                    let why = watch_terminal::recovery(&format!("{error:#}"));
                    watch_terminal::problem(&why)?;
                    state.unavailable = Some(why);
                    state.running = Some(watch);
                    return self.save(&state);
                }
            };
            state.unavailable = None;
            if watch.started != 0 && host_watch::expired(watch.started, now) {
                stop_session(&watch)?;
                self.finish_said(
                    &mut state,
                    watch,
                    "time_limit",
                    "the round reached its hour and did not report",
                )?;
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
                )?;
            } else {
                let screen = worktree.screen(&watch.terminal, host_watch::EVIDENCE_LINES)?;
                let (worker, runtime) =
                    worktree.terminal_health(&watch.terminal, watch.harness, now)?;
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
                    )?;
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
                    "Continue this watch. You have not reported; finish the work and run the exact afkconvoy watched line at the end of your brief.",
                    watch.harness)?;
                        watch.nudged = Some(now);
                    }
                    state.resident = Some(watch.clone());
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
            (watch.model, watch.model_source) =
                crate::codex_launch::watch_selection(candidate.harness, candidate.model)?;
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
        self.finish_said(state, watch, "no_harness", &said)?;
        Ok(())
    }

    fn finish_said(
        &self,
        state: &mut State,
        mut watch: Watch,
        outcome: &str,
        said: &str,
    ) -> Result<()> {
        if watch.report.is_none() {
            state.resident = None;
            state.covered.retain(|episode| !watch.episodes.contains(episode));
        }
        let ended = crate::clock();
        watch.ended = Some(ended);
        watch.outcome = Some(outcome.into());
        let usage = keep_usage(&watch, ended)?;
        watch.tokens = usage.tokens.clone();
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
        self.say_state(state);
        Ok(())
    }
}

fn keep_usage(watch: &Watch, ended: u64) -> Result<afkconvoy_core::accounts::Usage> {
    use afkconvoy_core::account::Tokens;
    let start = (watch.started != 0).then_some(watch.started);
    let mut tokens = Tokens::new();
    let mut known = start.is_some();
    let attempts = watch.attempts.iter().map(|attempt| attempt.harness).chain([watch.harness]);
    for harness in attempts {
        let Some(start) = start else { continue };
        let Some(usage) = crate::session::tokens_between(
            &watch.directory,
            harness,
            start,
            ended,
            &cache()?.join("watch"),
        )
        .ok()
        .flatten() else {
            known = false;
            break;
        };
        for (model, counts) in usage {
            tokens.entry(model).or_default().add(&counts);
        }
    }
    let usage = afkconvoy_core::accounts::Usage {
        id: watch.id.clone(),
        harness: Some(watch.harness.into()),
        model: watch.model.clone(),
        start,
        end: Some(ended),
        seconds: start.and_then(|start| ended.checked_sub(start)),
        tokens: known.then_some(tokens),
        conditions: watch.items.iter().map(|item| item.id.clone()).collect(),
    };
    let directory = crate::watch_report::directory(&watch.id)?;
    std::fs::create_dir_all(&directory)?;
    let temporary = directory.join(format!("usage-{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&usage)?)?;
    std::fs::rename(temporary, directory.join("usage.json"))?;
    Ok(usage)
}

/// Every round whose usage record the host retained, including rounds beyond
/// the scheduler's short in-state history.
pub fn rounds() -> Result<Vec<afkconvoy_core::accounts::Usage>> {
    let directory = cache()?.join("watch");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut rounds = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("usage.json");
        match std::fs::read(&path) {
            Ok(bytes) => rounds.push(
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("cannot read watch usage {}", path.display()))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(rounds)
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

pub(crate) fn session_identity(watch: &Watch) -> String {
    if watch.session.is_empty() {
        format!("{}-{}", watch.id, watch.attempts.len() + 1)
    } else {
        watch.session.clone()
    }
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
    watch_terminal::set_status(watch, LaunchState::Ended)
}

pub(crate) fn stop_marked(identity: &str) -> Result<()> {
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
pub(crate) fn gave_out(
    harness: Harness,
    transcript: &str,
    exited: Option<i32>,
) -> Option<Diagnosis> {
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
    let result = launch_session(watch)?;
    resume_delivery(watch)?;
    Ok(result)
}

/// Creates only the owned executor. Round delivery has a separate journal
/// boundary, so creation recovery cannot silently become a second prompt.
fn launch_session(watch: &mut Watch) -> Result<(String, PathBuf)> {
    watch.session = uuid::Uuid::new_v4().to_string();
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
    write_brief(watch)?;
    let transcript = dir.join(format!("session-{session}.log"));
    watch.transcript = transcript.clone();
    // A watch is started as a worker of an unbound repo is: the operator's
    // directory holds no config, and no role.
    let repo = Repo::new(watch.directory.clone());
    let launches = crate::launch::Launches::new(&repo, true)?;
    let command = launches.command(watch.harness)?;
    let line = crate::codex_launch::watch_line(
        watch.harness,
        &watch.directory,
        watch.model.as_deref(),
        command.as_str(),
        Some(&crate::codex_launch::terminal_guard()),
    )?;
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
    Ok((watch.terminal.clone(), transcript))
}

/// A Ready launch has not sent input. Sending is written first: a crash or
/// lost acknowledgement at that boundary must never duplicate a prompt.
fn resume_delivery(watch: &mut Watch) -> Result<()> {
    let Some(mut record) = watch_terminal::read()?.filter(|r| watch_terminal::matches(r, watch))
    else {
        return Ok(()); // a watch retained from before launch journaling
    };
    match record.status {
        LaunchState::Running => {
            // Crash after the receipt, before the progress file.
            if record.watch.id == watch.id && watch.started == 0 {
                watch.started = record.watch.started;
            }
            return Ok(());
        }
        LaunchState::Idle | LaunchState::Ended => return Ok(()),
        LaunchState::Sending if record.request.is_none() => {
            bail!("the watch's first prompt has an uncertain receipt")
        }
        LaunchState::Sending => (),
        LaunchState::Creating => bail!("the watch's creation is not yet confirmed"),
        LaunchState::Ready => (),
    }
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    if record.status == LaunchState::Ready {
        worktree.await_ready(&watch.terminal, 60, watch.harness, &record.command)?;
        if !worktree.watch_input_ready(&watch.terminal)? {
            bail!("{}", crate::doctor::Kind::WatchBusy.symptom());
        }
        record.request = Some(uuid::Uuid::new_v4().to_string());
        record.status = LaunchState::Sending;
        record.offered = Some(crate::clock());
        record.watch = watch.clone();
        write_brief(watch)?;
        watch_terminal::save(&record)?;
    }
    let brief = brief_file(watch)?;
    // This round's offer, not a turn already on the session. A previous
    // round a second ago must not prove this submission.
    let earliest = record.offered.unwrap_or_else(crate::clock);
    let sent = worktree.send_watch_round(
        &watch.terminal,
        &brief_pointer(&brief.display().to_string()),
        record.request.as_deref().expect("saved request"),
    );
    let receipt = match sent {
        Ok(receipt) => receipt,
        Err(error) => {
            if worktree.saw_turn(watch.harness, earliest)? {
                json!({"send": {"accepted": true, "prompt": {"stages": ["session_turn"]}}})
            } else {
                return Err(error);
            }
        }
    };
    record.receipt = Some(receipt.clone());
    watch_terminal::save(&record)?;
    // A line the terminal did not accept did not start a turn. The session
    // is not consulted for it. An accepted line is proved by the session.
    let evidence =
        if receipt["send"]["accepted"] == true && worktree.prove(watch.harness, earliest)? {
            Some(json!({"kind": "session_turn"}))
        } else {
            None
        };
    if evidence.is_none() {
        bail!(
            "watch round {} submission is unproven; retained request {} and receipt {}; no fresh input will be sent",
            watch.id,
            record.request.as_deref().unwrap_or("unknown"),
            receipt
        );
    }
    if watch.started == 0 {
        watch.started = record.offered.unwrap_or_else(crate::clock);
    }
    record.watch = watch.clone();
    record.execution_evidence = evidence;
    watch_terminal::save(&record)?;
    watch_terminal::set_status(watch, LaunchState::Running)
}

fn brief_file(watch: &Watch) -> Result<PathBuf> {
    Ok(crate::watch_report::directory(&watch.id)?
        .join(format!("brief-{}.md", watch.attempts.len() + 1)))
}

fn write_brief(watch: &Watch) -> Result<()> {
    let path = brief_file(watch)?;
    std::fs::create_dir_all(path.parent().expect("round directory"))?;
    std::fs::write(&path, brief(watch))
        .with_context(|| format!("cannot write watch brief {}", path.display()))
}

/// What a session of `watch` is told.
fn brief(watch: &Watch) -> String {
    let human_conditions = serde_json::to_string_pretty(
        &actionable(&watch.look).iter().map(|item| &item.id).collect::<Vec<_>>(),
    )
    .unwrap_or_default();
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
         has, within these bounds (`afkconvoy run --help`): {BOUNDS}. Follow this \
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
         A repo nobody works is a human decision about which host works it. For a \
         `repo_unworked` finding, never add that repo to this host's checkout list. \
         Name the report, the waiting issues and the hosts that last worked it. Ask \
         the operator which host should take it, through your human report.\n\n\
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
         ## Distill the daybook's lessons into their lasting homes\n\n\
         A `{DISTILL}<tracker>:<since>:<until>` item is the week's distillation of that \
         tracker's daybook; take it after anything holding work up. Read \
         `afkconvoy daybook --help`. List the span's lessons with `afkconvoy daybook \
         --lessons --since <since> --until <until> --tracker <tracker>`, and read the span's \
         day discussions in the tracker's `Daybook` category for patterns that recur across \
         entries without a lesson, the same fault twice. Decide each one's lasting home and \
         propose it there through that home's normal path:\n\n\
         - a repeated mistake: first a mechanism — a hook, a gate check, a tool fix — as a \
         ticket to the repo that owns it;\n\
         - one project's decision or term: a ticket for that project's ADR or GLOSSARY;\n\
         - a cross-project decision or term: the control repo's own ADR or GLOSSARY, changed \
         directly, through the landing path below;\n\
         - how to use a tool: a ticket for that tool's own skill;\n\
         - how the operator thinks, the global principles in sunfmin/dotfiles \
         `claude/CLAUDE.md`: a **draft** PR on dotfiles (`gh pr create --draft`) for the \
         operator to read. Never merge it, mark it ready or land it.\n\n\
         Open each ticket with the create line `afkconvoy flows` gives in that repo's \
         checkout, on the tracker it names, as a human-opened ticket would be. First check \
         the home: a lesson its home already holds is `covered`, with a link to where. Keep \
         nothing of your own — no memory, precedent or proposal list: the proposal is the \
         record. Then write a JSON file carrying every lesson of the span exactly once, and \
         each pattern from its entries, and record the run with `afkconvoy daybook \
         --distilled <file> --tracker <tracker> --repo <tracker>`: it checks the file against \
         the span and writes the run's one daybook entry mapping each lesson to its outcome. \
         A refusal names what is wrong: correct the file and run it again. The daybook's \
         entries are left to age out.\n\n\
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
         the landing turn, tag by hand, or manufacture a cut record. If cutting is off or one \
         of the four human kinds holds it back, say so.\n\n\
         ## Answer a failed cut\n\n\
         A failed cut never holds the next cut back: the next due cut cuts again from the \
         start, on the base tip, whether or not its issue is open, and a cut that publishes \
         closes that issue. Its issue, under the label `{FAILED_CUT}`, is yours to look into, \
         not a human's. The look's `cut_failed` finding names its issue, version, step and \
         why, and its `fix` says what becomes of it: that the next cut starts by itself, or, \
         where the same step failed the same way three times in a row, that the issue is a \
         ticket for the fleet under the ready label and the next cut waits for a landing. Read \
         the issue and find the cause. Where it is code, land a fix as above: the next cut \
         takes it. Run no `cut --from` line and close nothing for the cut to start again; \
         wake its checkout and check that the next cut starts. Only where the cause is one of \
         the four human kinds, swap that label for the repo's escalate label (`afkconvoy \
         config`) and list it in your report: then it is a human's.\n\n\
         Check that the cut published and installed the version containing your change. \
         For afkconvoy, let the host runner move onto it by itself; read `doctor --all`'s \
         `host.installed` for what holds the move back and `checkouts` for its version. Never \
         kill or restart the host runner. On the new version repeat the check that showed \
         the fault and check the fleets' work goes on. Record the version, the evidence and \
         whether the fault is gone or still present; an upgrade alone proves nothing.\n\n\
         This watch round has one hour from {}, its continuations together; at the hour it is ended. End \
         with what held the work back, each thing you did and its evidence, each PR and cut, \
         and what you left — including every human decision — in your report. If landing, \
         cutting or the move has not finished, say what is still \
         pending; never report the fix verified before checking it.\n\n\
         ## Why it was started\n\n{}\n\n\
         ## What earlier watches on the same items did\n\n{}\n\n\
         ## What this watch did before this session\n\n{}\n\n\
         ## What the look answered\n\n`afkconvoy doctor --all --since <until of the look \
         before>` answered:\n\n```json\n{}\n```\n\n\
         ## Report this round, then await the next message\n\n\
         Write `{}` as a JSON object with all five keys: `held_back`, `did`, `verified`, \
         `human`, `pending`. The first three and `pending` are lists of nonempty strings; \
         `human` is a list of objects with `condition`, `question` and `answer` strings: the \
         condition's exact id from this round's look, the question left alone and its answer \
         command. Choose `condition` from the ids below. An older report may omit it only \
         when this look has exactly one condition; ambiguous or invented ids are refused. \
         Questions and answer words never identify a condition. Use [] for an empty part.\n\n\
         Condition ids in this round's look:\n\n```json\n{human_conditions}\n```\n\n\
         Printed output is not a report. Your last step for this round is this exact command. \
         Once it records, return to your interactive input and wait for the next task message \
         in this same session. Do not exit the harness, poll, sleep in a tool loop, or request \
         another model turn while idle. Only this round is complete; the host runner retains \
         other queued work and gives each later round its own ID, brief and report command:\n\n{}\n",
        answering(&watch.look),
        if watch.started == 0 {
            "its accepted Terminal submission".into()
        } else {
            crate::log::utc(watch.started)
        },
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
            if issue["taken_by"].is_string() {
                continue;
            }
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

/// A finding as the look answers it: what the watch acts on, by its kind and
/// its identity, as doctor scoped it.
#[derive(Deserialize)]
struct Said {
    kind: crate::doctor::Kind,
    severity: crate::doctor::Severity,
    what: String,
    evidence: String,
    fix: String,
    watch_id: String,
}

fn findings(items: &mut Vec<Item>, at: &str, findings: &Value) {
    for said in findings.as_array().into_iter().flatten() {
        let Ok(said) = Said::deserialize(said) else { continue };
        // An `info` finding holds nothing up. A watch that cannot start, and
        // a resident watch that is busy, are the operator's to act on.
        if said.severity == crate::doctor::Severity::Info || !said.kind.watched() {
            continue;
        }
        let reason =
            format!("{}; evidence: {}; fix: {}", said.what, json!(said.evidence), json!(said.fix));
        push(items, unscoped_as_scoped(at, &said), reason);
    }
}

/// The identity of `said`, found at `at`, as doctor says it. A look saved by
/// an afkconvoy before doctor scoped `watch_id` itself still holds it
/// unscoped, `checkout:` or `host:`: a round started then and reported after
/// the host runner moved is read by the rule the watch went by then, so its
/// human conditions still match. A scoped identity never begins so. Delete
/// this once no host runs a release older than the one that made doctor
/// scope it, and no round it started is left: one release past it.
fn unscoped_as_scoped(at: &str, said: &Said) -> String {
    let id = &said.watch_id;
    if id.starts_with("checkout:") || id.starts_with("host:") {
        crate::doctor::scoped(at, said.kind, id)
    } else {
        id.clone()
    }
}

/// Everything in a look an operator would act on, each by an identity that
/// stays the same while its episode lasts.
pub(crate) fn actionable(look: &Value) -> Vec<Item> {
    let mut items = vec![];
    findings(&mut items, "host", &look["findings"]);
    for checkout in look["checkouts"].as_array().into_iter().flatten() {
        let at = checkout["checkout"].as_str().unwrap_or("unknown");
        let doctor = &checkout["doctor"];
        findings(&mut items, at, &doctor["findings"]);
        for waiting in doctor["waiting_on_human"].as_array().into_iter().flatten() {
            if waiting["taken_by"].is_string() {
                continue;
            }
            let issue = waiting["issue"].as_u64().unwrap_or(0);
            let reason = format!(
                "#{issue} waits on a human: {}; answer: {}",
                waiting["why"], waiting["answer"]
            );
            if let Ok(id) = crate::doctor::waiting_id(at, waiting) {
                push(&mut items, id, reason);
            }
        }
    }
    for item in crate::distill::items(look) {
        push(&mut items, item.id, item.reason);
    }
    items
}

/// What `doctor` answers of the host's watch: the one running, the last that
/// ended, and why none could start or go on, where none could. Earlier rounds
/// are included only when the caller asks for them.
pub fn json(running: Option<&Watch>, rounds: bool) -> Value {
    let state = read();
    let (last, unavailable) = match &state {
        Ok(state) => (json!(state.last.as_ref().map(watch_json)), json!(state.unavailable)),
        Err(error) => (Value::Null, json!(format!("{error:#}"))),
    };
    let mut answer =
        json!({"running": running.map(watch_json), "last": last, "unavailable": unavailable});
    answer["association"] = watch_terminal::json();
    if let Ok(state) = &state {
        let session = state.running.as_ref().or(state.resident.as_ref());
        answer["session"] = json!(session.map(|session| json!({
            "id": session_identity(session), "terminal": session.terminal,
            "harness": session.harness, "directory": session.directory,
            "status": if state.unavailable.is_some() { "unavailable" } else if state.running.is_some() { "active" } else { "standby" },
            "round": state.running.as_ref().map(|r| &r.id),
        })));
        answer["pending"] = json!(state.pending);
        if rounds {
            answer["rounds"] = json!(state.history.iter().map(watch_json).collect::<Vec<_>>());
        }
        answer["looked"] = json!(state.looked);
        answer["until"] = json!(state.until);
    }
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

/// The host's finding for its watch. A resident watch whose round is still
/// undelivered is that watch. A look no harness could start stays the
/// troubleshooting row for that fact, and only when the host kit's paused
/// list or worker-health faults say so.
pub fn finding() -> Option<crate::doctor::Finding> {
    let association = watch_terminal::json();
    let problem = association["problem"].as_str().map(str::to_string);
    let unavailable = read().ok().and_then(|state| state.unavailable).or_else(|| {
        watch_terminal::read().err().map(|error| watch_terminal::recovery(&format!("{error:#}")))
    });
    let busy = crate::doctor::Kind::WatchBusy.symptom();
    let situation = problem
        .filter(|text| text.contains(busy))
        .or_else(|| unavailable.as_ref().filter(|text| text.contains(busy)).cloned());
    let cannot_start = |why: String| {
        crate::doctor::finding(
            crate::doctor::Kind::WatchUnavailable,
            json!(["watch-unavailable"]),
            crate::doctor::Kind::WatchUnavailable.symptom().to_string(),
            why,
            "afkconvoy kit".to_string(),
        )
    };
    // The kit's paused list or its faults are the no-harness row. The busy
    // sentence alone is not.
    match situation {
        Some(situation) if !crate::health::no_harness_for_watch() => Some(crate::doctor::finding(
            crate::doctor::Kind::WatchBusy,
            json!(["watch-busy"]),
            busy.to_string(),
            situation,
            busy_fix(&association),
        )),
        Some(situation) => Some(cannot_start(unavailable.unwrap_or(situation))),
        None => unavailable.map(cannot_start),
    }
}

/// One command listing the terminals in the watch's own directory. A missing
/// directory stays the `<watch-directory>` placeholder.
fn busy_fix(association: &Value) -> String {
    let directory = association["directory"].as_str().filter(|dir| !dir.is_empty());
    let selector = directory
        .map(|dir| format!("path:{dir}"))
        .unwrap_or_else(|| "path:<watch-directory>".to_string());
    afkconvoy_core::line::Line::new()
        .fixed("orca terminal list --worktree ")
        .word(&selector)
        .fixed(" --json")
        .own()
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

    const HUMAN_SINCE: u64 = 1_793_036_772;

    fn human_look(now: u64) -> Value {
        use afkconvoy_core::tracker::Tracker;
        use afkconvoy_core::waiting::{Back, Said, Waiting, Waits};
        let mut waiting = Waiting::new(
            11,
            &Tracker::Own,
            "quiet measurement environment",
            Waits::Escalated(Back::Ready {
                ready: "ready-for-agent".into(),
                escalate: "ready-for-human".into(),
            }),
            Some(HUMAN_SINCE),
        );
        waiting.explain(&[Said {
            body: "<!--convoy:escalation claim=old-->\nNeeds a quiet measurement environment.",
            url: "https://github.com/sunfmin/control/issues/11#issuecomment-6423248030",
            at: Some(HUMAN_SINCE),
        }]);
        json!({"until": now, "checkouts": [{"checkout": "/bizkernel", "doctor": {
            "findings": crate::doctor::answered(
                "/bizkernel",
                crate::doctor::waiting_findings(std::slice::from_ref(&waiting), now),
            ).unwrap(),
            "waiting_on_human": [waiting], "fleet": {"alive": true}
        }}]})
    }

    fn reload(state: &State) -> State {
        serde_json::from_slice(&serde_json::to_vec(state).unwrap()).unwrap()
    }

    fn human_round(look: Value, items: Vec<Item>, episodes: Vec<String>, reported: bool) -> Watch {
        serde_json::from_value(json!({"id": "human-round", "directory": "/operator",
            "look": look, "items": items, "episodes": episodes, "started": HUMAN_SINCE + 34000,
            "harness": "codex", "model": null, "terminal": "watch", "transcript": "",
            "report": reported.then(|| json!({"held_back": ["quiet environment"], "did": [],
                "verified": [], "human": [{"question": "Provide a quiet environment?",
                "answer": "the current waiting_on_human.answer"}], "pending": []}))}))
        .unwrap()
    }

    #[test]
    fn first_long_wait_warning_and_new_answers_use_the_human_questions_episode() {
        let first = HUMAN_SINCE + 30 * 60;
        let mut state = State::default();
        let look = human_look(first);
        assert_eq!(look["checkouts"][0]["doctor"]["findings"], json!([]));
        state.observe(look.clone(), first);
        assert_eq!(state.pending.len(), 1);
        let episode = state.pending[0].episode.clone();
        let round = human_round(look, actionable(&human_look(first)), vec![episode.clone()], true);
        state.covered.extend(round.episodes.iter().cloned());
        state.history.push(round);
        let mut state = reload(&state);
        let later = HUMAN_SINCE + 9 * 3600 + 33 * 60;
        state.observe(human_look(later), later);
        assert!(state.pending.is_empty(), "first warning is the already-reported question");
        assert_eq!(state.observed.values().collect::<Vec<_>>(), [&episode]);
        for field in ["url", "since", "why", "answer"] {
            let mut next = human_look(later);
            let value = &mut next["checkouts"][0]["doctor"]["waiting_on_human"][0][field];
            *value = if field == "since" { json!(later) } else { json!("changed answer source") };
            next["checkouts"][0]["doctor"]["findings"] = json!([]);
            let mut changed = reload(&state);
            changed.observe(next, later);
            assert_eq!(changed.pending.len(), 1, "changed {field} requires a new answer");
        }
        let mut unrelated = human_look(later);
        let mut other = unrelated["checkouts"][0].clone();
        other["checkout"] = json!("/another-repo");
        unrelated["checkouts"].as_array_mut().unwrap().push(other);
        state.observe(unrelated, later);
        assert_eq!(state.pending.len(), 1, "another checkout is unrelated work");
    }

    #[test]
    fn reported_human_wait_keeps_one_episode_as_doctors_displayed_age_increases() {
        let first = HUMAN_SINCE + 9 * 3600 + 27 * 60;
        let later = HUMAN_SINCE + 9 * 3600 + 33 * 60;
        let look = human_look(first);
        assert!(
            look["checkouts"][0]["doctor"]["findings"][0]["what"]
                .as_str()
                .unwrap()
                .contains("9h 27m")
        );
        assert!(
            human_look(later)["checkouts"][0]["doctor"]["findings"][0]["what"]
                .as_str()
                .unwrap()
                .contains("9h 33m")
        );
        let mut state = State::default();
        state.observe(look, first);
        assert_eq!(state.pending.len(), 1, "one actionable condition for both representations");
        let episode = state.pending[0].episode.clone();
        state.covered.push(episode.clone());
        let mut state = reload(&state);
        state.observe(human_look(later), later);
        assert!(state.pending.is_empty(), "the report covers the older wait");
        assert_eq!(state.observed.values().collect::<Vec<_>>(), [&episode]);

        state.observe(json!({"until": later + 1}), later + 1);
        state.observe(human_look(later + 2), later + 2);
        assert_eq!(state.pending.len(), 1, "recovery and return is new work");
        assert_ne!(state.pending[0].episode, episode);
        let unreported = state.pending[0].episode.clone();
        let mut state = reload(&state);
        state.observe(human_look(later + 300), later + 300);
        assert_eq!(state.pending[0].episode, unreported, "unreported work stays eligible");
    }

    /// Uses a real interactive Codex in this worker's visible Terminal, with
    /// harmless bounded tasks and isolated durable state. Normal scheduling
    /// is covered by the host-runner CLI scenarios in tests/host_watch.rs.
    #[test]
    #[ignore = "requires real local Orca and Codex; set TMPDIR inside this worktree's .afkconvoy-tmp"]
    fn real_orca_resident_agent_reports_two_rounds_in_one_terminal() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let cache = cache().unwrap();
        assert!(
            cache.starts_with(directory.join(".afkconvoy-tmp")),
            "an isolated smoke cache is required"
        );
        assert!(read().unwrap().running.is_none() && watch_terminal::active().unwrap().is_none());
        let binary = directory.join("target/debug/afkconvoy");
        assert!(binary.is_file(), "build the CLI first");
        let watcher = Watcher {
            ended: Arc::new(AtomicBool::new(false)),
            shared: Shared::default(),
            say: Box::new(|what| println!("{}", serde_json::to_string(&what).unwrap())),
            looking: Failing::default(),
            checking: Failing::default(),
        };
        let make = |id: &str| -> Watch {
            serde_json::from_value(json!({"id": id, "session": "", "directory": directory,
                "look": {}, "items": [], "started": 0, "session_started": crate::clock(),
                "harness": "codex", "model": null, "terminal": "", "transcript": ""}))
            .unwrap()
        };
        let mut first = make(&format!("real-first-{}", crate::unique()));
        launch_session(&mut first).unwrap();
        struct Cleanup(Watch);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = stop_session(&self.0);
                let worktree = Worktree::at(self.0.directory.clone(), String::new());
                let _ = worktree.close_terminal(&self.0.terminal);
            }
        }
        let _cleanup = Cleanup(first.clone());
        let worktree = Worktree::at(directory.clone(), String::new());
        worktree.await_ready(&first.terminal, 60, first.harness, "codex").unwrap();
        let submit = |round: &mut Watch, number: u64| {
            let dir = crate::watch_report::directory(&round.id).unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("smoke-report.json");
            let report = json!({"held_back": [], "did": [format!("smoke round {number} only")],
                "verified": [format!("interactive round {number} received")], "human": [], "pending": []});
            std::fs::write(&file, report.to_string()).unwrap();
            let command = afkconvoy_core::line::Line::new()
                .fixed("XDG_CACHE_HOME=")
                .word(&cache.parent().unwrap().display().to_string())
                .fixed(" ")
                .word(&binary.display().to_string())
                .fixed(" watched --watch ")
                .word(&round.id)
                .fixed(" --text-file ")
                .word(&file.display().to_string())
                .own();
            let task = format!(
                "Bounded resident Terminal smoke test, round {number}. Only run this exact shell command, then say 'round {number} reported' and return to interactive input. Do not exit, poll or operate any fleet, repo, other session, network or user data. Wait for my next message in this same Agent session.\n\n{command}\n"
            );
            std::fs::write(brief_file(round).unwrap(), task).unwrap();
            let mut record = watch_terminal::read().unwrap().unwrap();
            record.watch = round.clone();
            record.status = LaunchState::Sending;
            record.request = Some(uuid::Uuid::new_v4().to_string());
            record.offered = Some(crate::clock());
            watch_terminal::save(&record).unwrap();
            let mut state = read().unwrap();
            state.running = Some(round.clone());
            watcher.save(&state).unwrap();
            let until = Instant::now() + Duration::from_secs(90);
            loop {
                match resume_delivery(round) {
                    Ok(()) => break,
                    Err(_) if crate::watch_report::read(&round.id).unwrap().is_some() => break,
                    Err(error) => {
                        assert!(Instant::now() < until, "{error:#}");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
            let mut state = read().unwrap();
            state.running = Some(round.clone());
            watcher.save(&state).unwrap();
        };
        let wait_report = |round: &Watch| {
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                watcher.check().unwrap();
                if read()
                    .unwrap()
                    .last
                    .as_ref()
                    .is_some_and(|last| last.id == round.id && last.report.is_some())
                {
                    break;
                }
                assert!(Instant::now() < deadline, "round {} did not report", round.id);
                std::thread::sleep(Duration::from_millis(500));
            }
        };
        let executor = |round: &Watch| {
            let marker = format!("{}{}", host_watch::SESSION_PROCESS, session_identity(round));
            let processes = crate::hot::listed_checked().unwrap();
            let pids: Vec<_> = processes
                .iter()
                .filter(|p| p.line.split_whitespace().any(|word| word == marker))
                .map(|p| p.pid)
                .collect();
            assert_eq!(pids.len(), 1, "one actual executor");
            pids[0]
        };
        submit(&mut first, 1);
        wait_report(&first);
        let pid = executor(&first);
        let first_runtime =
            worktree.terminal_health(&first.terminal, first.harness, crate::clock()).unwrap().1;
        let deadline = Instant::now() + Duration::from_secs(90);
        while !worktree.watch_input_ready(&first.terminal).unwrap() {
            assert!(Instant::now() < deadline, "first round did not return to input");
            std::thread::sleep(Duration::from_millis(500));
        }
        // Recover solely from durable state before the next round.
        let resident = read().unwrap().resident.unwrap();
        assert_eq!(watch_terminal::locate(&resident).unwrap().unwrap(), first.terminal);
        let mut second = make(&format!("real-second-{}", crate::unique()));
        second.session = resident.session;
        second.terminal = resident.terminal;
        second.transcript = resident.transcript;
        second.session_started = resident.session_started;
        watch_terminal::begin_round(&second).unwrap();
        submit(&mut second, 2);
        wait_report(&second);
        assert_eq!(executor(&first), pid, "same Agent wrapper process after both reports");
        assert!(worktree.live(&second.terminal).unwrap());
        assert_eq!(
            crate::watch_report::read(&first.id).unwrap().unwrap().did,
            ["smoke round 1 only"]
        );
        assert_eq!(
            crate::watch_report::read(&second.id).unwrap().unwrap().did,
            ["smoke round 2 only"]
        );
        let deadline = Instant::now() + Duration::from_secs(90);
        while !worktree.watch_input_ready(&second.terminal).unwrap() {
            assert!(Instant::now() < deadline, "second round did not return to input");
            std::thread::sleep(Duration::from_millis(500));
        }
        let second_runtime =
            worktree.terminal_health(&second.terminal, second.harness, crate::clock()).unwrap().1;
        let evidence = json!({"terminal": first.terminal, "session": first.session,
            "executor_pid": pid, "first": watch_json(&read().unwrap().history[0]),
            "second": watch_json(read().unwrap().last.as_ref().unwrap()),
            "first_runtime": first_runtime, "second_runtime": second_runtime,
            "screen_after_both_reports": worktree.screen(&second.terminal, 35).unwrap(),
            "input_ready_after_both_reports": true});
        std::fs::write(
            directory.join(".afkconvoy-tmp/real-resident-evidence.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
        println!(
            "real resident proof: terminal={}, session={}, executor_pid={pid}, first={}, second={}, reports=2; both reports left the same interactive Agent alive",
            first.terminal, first.session, first.id, second.id
        );
    }

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
            "failed_steps": []}, "watching": {"ticks": [{"instance": "a", "failed": false}]}}}]});
        assert!(actionable(&look).is_empty());
    }

    #[test]
    fn every_actionable_kind_has_an_identity_that_holds_through_its_episode() {
        use crate::doctor::{Kind, answered, finding};
        // Each finding as doctor makes it and answers it, said in the words
        // `say` gives.
        let look = |say: &dyn Fn(&str) -> String| {
            let made = |kind: Kind, identity: Value, what: &str| {
                finding(kind, identity, say(what), "seen".into(), "a fix".into())
            };
            json!({"findings": answered("host", vec![
                made(Kind::OrcaNotRunning, json!(["tool", "orca"]), "`orca` is not running"),
                made(Kind::WatchUnavailable, json!(["watch-unavailable"]),
                    "no harness can start this host's watch"),
                made(Kind::WatchBusy, json!(["watch-busy"]),
                    "the resident watch is busy; its round remains undelivered"),
                made(Kind::KitUnplaced, json!(["kit-unplaced", "codex"]),
                    "the harness `codex` has unplaced models in the host kit"),
                made(Kind::HealthOverdue, json!(["health-overdue", "/w"]),
                    "the worker in /w: its health observation is overdue"),
            ]).unwrap(),
            "checkouts": [{"checkout": "/r", "doctor": {
                "findings": answered("/r", vec![
                    made(Kind::StepUnrecovered, json!(["step", "dispatch", null, null]),
                        "`dispatch` failed in tick 4 and has not recovered"),
                    made(Kind::Broken, json!(["broken", 800]),
                        "the fleet instance of this checkout is broken: panic"),
                    made(Kind::TickFailed, json!(["tick", "alpha", []]),
                        "a tick failed since the last look: one of alpha"),
                    made(Kind::RetiredConcurrency, json!(["retired-concurrency"]),
                        "`concurrency: 2`, which nothing reads any more"),
                ]).unwrap(),
                "waiting_on_human": [{"issue": 7, "why": "escalated", "answer": "gh issue view 7"}],
            }}]})
        };
        let ids = |look: &Value| -> Vec<String> {
            actionable(look).into_iter().map(|item| item.id).collect()
        };
        let first = ids(&look(&|what| what.to_string()));
        assert_eq!(
            first,
            [
                r#"finding:host:["tool","orca"]"#,
                r#"kit-research:host:["kit-unplaced","codex"]"#,
                r#"finding:host:["health-overdue","/w"]"#,
                r#"finding:/r:["step","dispatch",null,null]"#,
                r#"finding:/r:["broken",800]"#,
                r#"finding:/r:["tick","alpha",[]]"#,
                r#"finding:/r:["human",{"answer":"gh issue view 7","issue":7,"since":null,"url":null,"why":"escalated"}]"#,
            ],
            "the watch's own findings and an `info` one are not its to act on"
        );
        let reworded = ids(&look(&|what| format!("{what}, said again in other words")));
        assert_eq!(reworded, first, "rewording a finding keeps its episode");
    }

    #[test]
    fn a_round_whose_look_an_older_afkconvoy_saved_unscoped_still_matches_its_human_conditions() {
        use crate::doctor::{Kind, answered, finding};
        let made = |kind: Kind, identity: Value, what: &str| {
            finding(kind, identity, what.into(), "seen".into(), "a fix".into())
        };
        let host = || {
            vec![
                made(Kind::OrcaNotRunning, json!(["tool", "orca"]), "`orca` is not running"),
                made(
                    Kind::KitUnplaced,
                    json!(["kit-unplaced", "codex"]),
                    "the harness `codex` has unplaced models in the host kit",
                ),
            ]
        };
        let own = || {
            vec![made(
                Kind::StepUnrecovered,
                json!(["step", "dispatch", null, null]),
                "`dispatch` failed in tick 4 and has not recovered",
            )]
        };
        let waiting = json!([{"issue": 7, "why": "escalated", "answer": "gh issue view 7"}]);
        let look = |host: Value, own: Value| {
            json!({"findings": host, "checkouts": [{"checkout": "/r", "doctor": {
                "findings": own, "waiting_on_human": waiting}}]})
        };
        // As the older afkconvoy saved it: each `watch_id` as judged.
        let old = look(json!(host()), json!(own()));
        assert_eq!(old["findings"][0]["watch_id"], r#"host:["tool","orca"]"#);
        let new = look(answered("host", host()).unwrap(), answered("/r", own()).unwrap());
        let ids = |look: &Value| -> Vec<String> {
            actionable(look).into_iter().map(|item| item.id).collect()
        };
        assert_eq!(ids(&old), ids(&new), "both forms are acted on by one identity");
        // A round started on the old look and reported after the host runner
        // moved: its human conditions name the identities the watch was given.
        let watch = human_round(old, actionable(&new), vec![], true);
        for condition in ids(&new) {
            let human = crate::watch_report::Human {
                condition: Some(condition.clone()),
                question: "Provide a quiet environment?".into(),
                answer: "no".into(),
            };
            assert_eq!(crate::watch_report::condition(&watch, &human).unwrap(), condition);
        }
    }

    #[test]
    fn the_watch_acts_on_every_kind_of_finding_by_the_identity_it_did_before_doctor_scoped_it() {
        use crate::doctor::{Finding, Kind, Severity, answered, finding, waiting_id};
        // The watch's own rule before doctor answered each finding scoped:
        // what every open question and ruling of a watch is keyed by.
        let before = |at: &str, found: &Finding| {
            let id = &found.watch_id;
            if found.kind == Kind::KitUnplaced {
                format!("{}{id}", host_watch::RESEARCH_ITEM)
            } else if let Some(identity) = id.strip_prefix("checkout:") {
                format!("finding:{at}:{identity}")
            } else if let Some(identity) = id.strip_prefix("host:") {
                format!("finding:host:{identity}")
            } else {
                id.to_string()
            }
        };
        let made = |kind: Kind| {
            let identity = json!([format!("{kind:?}"), "/w", 7]);
            finding(kind, identity, kind.symptom().into(), "seen".into(), "a fix".into())
        };
        // Every kind as doctor makes it, and those a resource episode names
        // by the episode's identity.
        let mut found: Vec<Finding> = Kind::ALL.iter().map(|kind| made(*kind)).collect();
        for kind in [Kind::AdmissionHeld, Kind::MemoryPressure] {
            found.push(Finding { watch_id: format!("{kind:?}-episode"), ..made(kind) });
        }
        let (of_host, own): (Vec<_>, Vec<_>) = found.into_iter().partition(|f| f.of_host);
        let waiting = json!({"issue": 7, "title": "seven", "why": "escalated", "url": null,
            "since": 10, "answer": "gh issue view 7"});
        let look = json!({
            "findings": answered("host", of_host.clone()).unwrap(),
            "checkouts": [{"checkout": "/r", "doctor": {
                "findings": answered("/r", own.clone()).unwrap(),
                "waiting_on_human": [waiting.clone()],
            }}],
        });
        let said = |list: &Value| -> Vec<String> {
            let found = list.as_array().unwrap().iter();
            found.map(|f| f["watch_id"].as_str().unwrap().to_string()).collect()
        };
        let was = |at: &str, found: &[Finding]| -> Vec<String> {
            found.iter().map(|f| before(at, f)).collect()
        };
        assert_eq!(said(&look["findings"]), was("host", &of_host), "the host's findings");
        let checkout = &look["checkouts"][0]["doctor"]["findings"];
        assert_eq!(said(checkout), was("/r", &own), "the checkout's findings");
        // A waiting issue is acted on by the identity its finding has once it
        // has waited long.
        let identity = json!(["human", {"issue": 7, "since": 10, "url": null, "why": "escalated",
            "answer": "gh issue view 7"}]);
        let long = Finding { watch_id: format!("checkout:{identity}"), ..made(Kind::WaitedLong) };
        let human = before("/r", &long);
        assert_eq!(waiting_id("/r", &waiting).unwrap(), human);
        let acted = |f: &&Finding| f.severity != Severity::Info && f.kind.watched();
        let mut expected = was("host", &of_host.iter().filter(acted).cloned().collect::<Vec<_>>());
        expected.extend(was("/r", &own.iter().filter(acted).cloned().collect::<Vec<_>>()));
        expected.push(human);
        let ids: Vec<String> = actionable(&look).into_iter().map(|item| item.id).collect();
        assert_eq!(ids, expected, "each kind the watch acts on keeps its identity");
        assert!(ids.iter().any(|id| id.starts_with(host_watch::RESEARCH_ITEM)), "{ids:?}");
        assert!(ids.iter().any(|id| id == "AdmissionHeld-episode"), "{ids:?}");
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

//! The host runner's watch of its host (docs/adr/0030). A look is the fleet's
//! own `doctor --all --since`, and starts no model; a look holding something
//! an operator would act on starts one bounded watch in the operator's
//! directory, continued on another harness where its own gives out. The
//! process holds nothing of it: the watch, its terminal and its transcript are
//! read back from the host's cache after every restart.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
use crate::stop::Stop;

const STATE: &str = "watch/state.json";
/// How often a running watch's terminal and transcript are read.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// How long a look may take before it is given up, and taken again later.
const LOOK_SECONDS: u64 = 300;
/// How many ended watches are kept, for what earlier watches on the same
/// items did.
const HISTORY: usize = 20;
/// What a watch's terminal is titled in orca.
const TITLE: &str = "afkconvoy watch";

/// A watch as the host keeps it: what a continuation of it needs too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watch {
    pub id: String,
    pub directory: PathBuf,
    /// What the look that started it answered.
    pub look: Value,
    /// Why it was started.
    pub items: Vec<Item>,
    pub started: u64,
    /// The session running now, or the last one.
    #[serde(deserialize_with = "crate::kit::harness")]
    pub harness: Harness,
    pub model: Option<String>,
    pub terminal: String,
    pub transcript: PathBuf,
    /// What earlier watches on the same items did.
    #[serde(default)]
    pub earlier: Vec<String>,
    /// What its sessions did, each line one thing.
    #[serde(default)]
    pub did: Vec<String>,
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
    pub terminal: String,
    pub why: String,
    #[serde(default)]
    pub did: Vec<String>,
}

/// A watch as `doctor` says it: all but the look, which is the watch's own.
pub fn watch_json(watch: &Watch) -> Value {
    json!({"id": watch.id, "directory": watch.directory, "items": watch.items,
        "started": watch.started, "harness": watch.harness, "model": watch.model,
        "terminal": watch.terminal, "transcript": watch.transcript, "did": watch.did,
        "attempts": watch.attempts, "ended": watch.ended, "outcome": watch.outcome})
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// The `until` of the last look: where the next starts.
    until: Option<u64>,
    /// The items a watch was started on, while their episodes last.
    watched: Vec<String>,
    running: Option<Watch>,
    last: Option<Watch>,
    history: Vec<Watch>,
    /// Why the last watch a look called for could not start or go on.
    unavailable: Option<String>,
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

/// The host runner's watch, on a thread of its own. Dropping it waits for that
/// thread, never for the watch's session nor ends it: the next host runner
/// keeps it.
pub struct HostWatch {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HostWatch {
    /// Watches the host until `stop` is told, saying each start, end and
    /// thing done to `say`.
    pub fn start(
        stop: Stop,
        shared: Shared,
        say: impl Fn(What) + Send + Sync + 'static,
    ) -> HostWatch {
        let thread = std::thread::spawn(move || {
            Watcher { stop, shared, say: Box::new(say), failed: None }.run();
        });
        HostWatch { thread: Some(thread) }
    }
}

impl Drop for HostWatch {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Watcher {
    stop: Stop,
    shared: Shared,
    say: Box<dyn Fn(What) + Send + Sync>,
    /// What failed last, said once until something else fails.
    failed: Option<String>,
}

impl Watcher {
    fn run(&mut self) {
        self.shared.set(read().ok().and_then(|state| state.running));
        let (mut next_look, mut next_check) = (Instant::now(), Instant::now());
        while !self.stop.told() {
            let now = Instant::now();
            if now >= next_check {
                next_check = now + CHECK_EVERY;
                let checked = self.check();
                self.failing(checked);
            }
            let settings = crate::host_settings::of_host().unwrap_or_default();
            match &settings.watch_dir {
                // A look is taken only while no watch runs: what it would
                // find waits for the first look after the watch ends.
                Some(directory) if now >= next_look && self.shared.running().is_none() => {
                    next_look = now + Duration::from_secs(settings.watch_every());
                    let looked = self.look(directory);
                    self.failing(looked);
                }
                Some(_) => {}
                None => next_look = now,
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn failing(&mut self, done: Result<()>) {
        let failed = done.err().map(|error| format!("{error:#}"));
        if let Some(said) = &failed
            && self.failed.as_ref() != Some(said)
        {
            (self.say)(What::WatchFailed { said: said.clone() });
        }
        self.failed = failed;
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
    fn look(&self, directory: &Path) -> Result<()> {
        let mut state = read()?;
        if state.running.is_some() {
            return Ok(());
        }
        if !directory.is_dir() {
            bail!(
                "the watch directory {} is not a directory: `afkconvoy run --watch-dir <dir>` \
                 names the operator's",
                directory.display()
            );
        }
        let look = doctor_look(state.until)?;
        state.until = look["until"].as_u64().or(state.until);
        let current = actionable(&look, look["until"].as_u64().unwrap_or_else(crate::clock));
        state.watched = host_watch::covered(&state.watched, &current);
        let pending = host_watch::pending(&current, &state.watched, false);
        if pending.is_empty() {
            state.unavailable = None;
        } else {
            self.start(&mut state, directory, look, pending)?;
        }
        self.save(&state)
    }

    /// Starts a watch on `items` on the first harness that can start it.
    fn start(
        &self,
        state: &mut State,
        directory: &Path,
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
                std::iter::once(ended).chain(watch.did.iter().map(|did| format!("- {did}")))
            })
            .collect();
        let now = crate::clock();
        let mut watch = Watch {
            id: format!("{now}-{}", crate::unique()),
            directory: directory.to_path_buf(),
            look,
            items,
            started: now,
            harness: Harness::DEFAULT,
            model: None,
            terminal: String::new(),
            transcript: PathBuf::new(),
            earlier,
            did: vec![],
            attempts: vec![],
            ended: None,
            outcome: None,
        };
        let mut failures = vec![];
        for candidate in crate::health::watch_candidates()? {
            watch.harness = candidate.harness;
            watch.model = candidate.model;
            match start_session(&watch) {
                Ok((terminal, transcript)) => {
                    (watch.terminal, watch.transcript) = (terminal, transcript);
                    state.watched.extend(watch.items.iter().map(|item| item.id.clone()));
                    state.watched.sort();
                    state.watched.dedup();
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
                Err(error) => failures.push(format!("{}: {error:#}", candidate.harness.key())),
            }
        }
        state.unavailable = Some(unstartable(&failures));
        Ok(())
    }

    /// Reads the running watch, and ends or continues it where its session
    /// is over, its harness gave out or its hour is up.
    fn check(&self) -> Result<()> {
        let mut state = read()?;
        let Some(watch) = state.running.take() else {
            self.shared.set(None);
            return Ok(());
        };
        let worktree = Worktree::at(watch.directory.clone(), String::new());
        let transcript = std::fs::read_to_string(&watch.transcript).unwrap_or_default();
        let exited = host_watch::exited(&transcript);
        let live = worktree.live(&watch.terminal)?;
        let now = crate::clock();
        if host_watch::expired(watch.started, now) {
            close(&worktree, &watch.terminal, live)?;
            self.finish(&mut state, watch, "time_limit", "it reached its hour, and was ended");
        } else if let Some(fault) = gave_out(watch.harness, &transcript, exited) {
            close(&worktree, &watch.terminal, live)?;
            self.go_on(&mut state, watch, fault, now)?;
        } else if exited.is_some() {
            self.finish(&mut state, watch, "ended", "its harness answered and exited");
        } else if !live {
            let said = "its terminal closed before its harness exited";
            self.finish(&mut state, watch, "closed", said);
        } else {
            state.running = Some(watch);
            self.shared.set(state.running.clone());
            return Ok(());
        }
        self.save(&state)
    }

    /// What the session of `watch` printed, said as what it did.
    fn did(&self, watch: &mut Watch) -> Vec<String> {
        let transcript = std::fs::read_to_string(&watch.transcript).unwrap_or_default();
        let did = host_watch::did(&transcript);
        for line in &did {
            (self.say)(What::WatchDid { id: watch.id.clone(), did: line.clone() });
        }
        watch.did.extend(did.iter().cloned());
        did
    }

    /// Continues `watch`, whose harness gave out, on the next harness that
    /// can start it, with the same look and what it did so far.
    fn go_on(&self, state: &mut State, mut watch: Watch, fault: Diagnosis, now: u64) -> Result<()> {
        let why = fault.evidence.join("; ");
        let did = self.did(&mut watch);
        crate::health::watch_fault(watch.harness, &fault, now)?;
        let from = watch.harness;
        watch.attempts.push(Attempt {
            harness: from,
            model: watch.model.clone(),
            terminal: watch.terminal.clone(),
            why: why.clone(),
            did,
        });
        let mut failures = vec![];
        for candidate in crate::health::watch_candidates()? {
            if watch.attempts.iter().any(|attempt| attempt.harness == candidate.harness) {
                continue;
            }
            watch.harness = candidate.harness;
            watch.model = candidate.model;
            match start_session(&watch) {
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
                Err(error) => failures.push(format!("{}: {error:#}", candidate.harness.key())),
            }
        }
        state.unavailable = Some(unstartable(&failures));
        let said = format!("{} gave out ({why}), and no harness can go on with it", from.key());
        self.finish_said(state, watch, "no_harness", &said);
        Ok(())
    }

    fn finish(&self, state: &mut State, mut watch: Watch, outcome: &str, said: &str) {
        self.did(&mut watch);
        self.finish_said(state, watch, outcome, said);
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

/// Whether the harness of a session gave out: its own words for a fault it
/// cannot go on past, or an exit with a failure.
fn gave_out(harness: Harness, transcript: &str, exited: Option<i32>) -> Option<Diagnosis> {
    let lines: Vec<String> = transcript.lines().map(str::to_string).collect();
    crate::health::terminal_fault(harness, &lines).or_else(|| {
        let status = exited.filter(|status| *status != 0)?;
        let mut evidence = host_watch::did(transcript);
        evidence.drain(..evidence.len().saturating_sub(3));
        evidence.push(format!("{} exited with status {status}", harness.key()));
        Some(crate::health::failed(evidence))
    })
}

/// Starts a session of `watch` on its harness: its brief written beside its
/// transcript, in the host's cache. Answers its terminal and transcript.
fn start_session(watch: &Watch) -> Result<(String, PathBuf)> {
    let session = watch.attempts.len() + 1;
    let dir = cache()?.join("watch").join(&watch.id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot make the watch's directory {}", dir.display()))?;
    let brief_file = dir.join(format!("brief-{session}.md"));
    std::fs::write(&brief_file, brief(watch))
        .with_context(|| format!("cannot write the watch's brief {}", brief_file.display()))?;
    let transcript = dir.join(format!("session-{session}.log"));
    // A watch is started as a worker of an unbound repo is: the operator's
    // directory holds no config, and no role.
    let repo = Repo::new(watch.directory.clone());
    let command = crate::launch::Launches::new(&repo, true)?.command(watch.harness)?;
    let kept = crate::kept::keep(watch.harness, true)?;
    let line = watch.harness.watch(
        command.as_str(),
        &watch.directory.display().to_string(),
        &kept.display().to_string(),
        watch.model.as_deref(),
        &brief_pointer(&brief_file.display().to_string()),
    );
    let line = host_watch::session(&line, &transcript.display().to_string());
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    Ok((worktree.start_watch(&line, TITLE)?, transcript))
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
                "A session on {} began this watch, and its harness gave out: {}. It printed:",
                attempt.harness.key(),
                attempt.why
            );
            std::iter::once(gave_out).chain(attempt.did.iter().map(|did| format!("    {did}")))
        })
        .collect();
    format!(
        "# A watch of this host\n\n\
         You are an afkconvoy **watch**: the operator on this host, started in the operator's \
         directory by the host runner, because its look found something an operator would act \
         on. Nobody reads this terminal: never wait for an answer, and ask nothing.\n\n\
         Find what holds this host's fleets and their work back, fix it — whatever that takes — \
         and check that the work goes on. You have what the operator at this host's keyboard \
         has, under the bounds of an inspection (`afkconvoy run --help`): only what the fleets \
         of this host started or own; never the host runner, the user's programs, other apps \
         or orca; never delete what cannot be rebuilt, nor a change no commit holds; no force \
         push, no push to a base branch, no merge outside the landing train. Follow this \
         directory's own rules for looking after the host's projects.\n\n\
         This watch has one hour from {}, its sessions together; at the hour it is ended. End \
         with what you did and what you left, a line each: what you print is its record.\n\n\
         ## Why it was started\n\n{}\n\n\
         ## What earlier watches on the same items did\n\n{}\n\n\
         ## What this watch did before this session\n\n{}\n\n\
         ## What the look answered\n\n`afkconvoy doctor --all --since <until of the look \
         before>` answered:\n\n```json\n{}\n```\n",
        crate::log::utc(watch.started),
        why.join("\n"),
        said(&watch.earlier),
        said(&sessions),
        serde_json::to_string_pretty(&watch.look).unwrap_or_default(),
    )
}

/// What `afkconvoy doctor --all --since <until>` answers: the fleet's own
/// look, which runs no model. One with something wrong exits non-zero, and is
/// read all the same.
fn doctor_look(until: Option<u64>) -> Result<Value> {
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
        if Instant::now() >= deadline {
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
        push(items, format!("finding:{at}:{what}"), reason);
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
                "the health check of the worker in {path} was due at {} UTC and is still \
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
                "the health check of the worker in {path} failed, and its retry is overdue: {}",
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
    json!({"running": running.map(watch_json), "last": last, "unavailable": unavailable})
}

/// The host's finding where a look called for a watch no harness could start.
pub fn finding() -> Option<crate::doctor::Finding> {
    let why = read().ok()?.unavailable?;
    Some(crate::doctor::finding(
        crate::doctor::Kind::WatchUnavailable,
        "no harness can start this host's watch".into(),
        why,
        "afkconvoy kit".into(),
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

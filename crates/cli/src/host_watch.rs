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
/// A checkout a watch makes for a change of a listed repo. Distinct from an
/// inspection's: starting an inspection must never reuse a watch's checkout.
pub const CHECKOUT: &str = "afkconvoy-watch";

/// The stock watch's whole instruction, kept as its directory's rules.
const STOCK_INSTRUCTION: &str = "\
# The stock watch

Find what holds this host's fleets and their work back, fix it with the watch's authority,
check that the work goes on, and say what you did.
Leave these with a human, never act on them, and list each in your printed record:
- spending money, or needing an account, a credential or a person's hands;
- what cannot be undone;
- taking a secret or private data off the host;
- changing what a product is for, or who it is for.
";

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
    json!({"id": watch.id, "directory": watch.directory, "stock": watch.stock, "items": watch.items,
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
    /// Last successful start that took kit research, retained across restarts.
    research_started: Option<u64>,
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
        let look = doctor_look(state.until, &self.ended)?;
        state.until = look["until"].as_u64().or(state.until);
        let current = actionable(&look, look["until"].as_u64().unwrap_or_else(crate::clock));
        state.watched = host_watch::covered(&state.watched, &current);
        let pending = host_watch::pending(
            &current,
            &state.watched,
            false,
            state.research_started,
            crate::clock(),
        );
        if pending.is_empty() {
            state.unavailable = None;
        } else {
            self.start(&mut state, &directory, settings.watch_dir.is_none(), look, pending)?;
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
                std::iter::once(ended).chain(watch.did.iter().map(|did| format!("- {did}")))
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
    if watch.stock {
        std::fs::create_dir_all(&watch.directory).with_context(|| {
            format!("cannot make the stock watch's directory {}", watch.directory.display())
        })?;
        // Each harness finds the same instruction by its own rules filename.
        // Refresh it at each start, including after moving onto a new release.
        for file in [afkconvoy_core::team::RULES, "AGENTS.md"] {
            std::fs::write(watch.directory.join(file), STOCK_INSTRUCTION)?;
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
    // A watch is started as a worker of an unbound repo is: the operator's
    // directory holds no config, and no role.
    let repo = Repo::new(watch.directory.clone());
    let command = crate::launch::Launches::new(&repo, true)?.command(watch.harness)?;
    let command = if watch.harness == Harness::Codex {
        format!("{} --search", command.as_str())
    } else {
        command.as_str().to_string()
    };
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
         You are an afkconvoy **watch**: the operator on this host, started in the watch's \
         directory by the host runner, because its look found something an operator would act \
         on. Nobody reads this terminal: never wait for an answer, and ask nothing.\n\n\
         Find what holds this host's fleets and their work back, fix it — whatever that takes — \
         and check that the work goes on. You have what the operator at this host's keyboard \
         has, under the bounds of an inspection (`afkconvoy run --help`): only what the fleets \
         of this host started or own; never the host runner, the user's programs, other apps \
         or orca; never delete what cannot be rebuilt, nor a change no commit holds; no force \
         push, no push to a base branch, no merge outside the landing train. Follow this \
         directory's own rules for looking after the host's projects.\n\n\
         Your authority covers every repo on this host's checkout list, not just the repo of \
         the fault or this directory. `afkconvoy checkouts` gives the current list; the look \
         below names each checkout and its repo. Read the help of each command you need.\n\n\
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
         An escalation that asks a human for a decision stays with the human. Do not invent \
         a ruling, pass a human stage, choose a workflow or clear its labels to make it run. \
         Leave it alone and list its repo, issue, question and answer line in your record.\n\n\
         ## Research the host kit's unplaced models\n\n\
         Take the host kit's unplaced models after anything holding work up. Read \
         `afkconvoy kit --help` for the research file's format and `afkconvoy doctor --all` \
         for the latest listed ids, names and research. Use your web tools to search primary \
         and high-trust sources: vendor model cards and release notes, SWE-bench, \
         Terminal-Bench and Aider. Research each pending identity once across harnesses; \
         use dated sources for its coding strengths, weaknesses and other names. Write a \
         JSON research file in preference order and run `afkconvoy kit --research <file>`. \
         A refusal names the fault: correct the file and run it again. Place what the \
         evidence supports; omitted models stay unplaced, and a harness with none placed \
         stays on its default model. Check the command's `placed` and the updated doctor \
         answer; record what you placed and what remains unplaced.\n\n\
         ## Change any repo on the checkout list\n\n\
         When a lasting fix needs code, make it yourself, even if that repo's fleet cannot \
         start a worker. In the affected repo create a linked worktree on a branch of your \
         own from its configured base branch. Name its directory `{CHECKOUT}` or \
         `{CHECKOUT}-<digits>` so `afkconvoy land --inspection` accepts it; never reuse an \
         inspection's or another worker's checkout. Work only in your own checkout of that \
         repo. Commit and push each completed piece before a long operation. Open its PR \
         against that repo's base branch, on no issue: no intake or flow label is needed, in \
         a bound repo too. Run `afkconvoy land --inspection` there: it takes a turn to merge, \
         gate and join on the train's green tip, recording green before releasing the turn. \
         Resolve conflicts by merging, committing and asking again; never rebase. This is the inspection's \
         landing path: its join names an inspection on this host and closes no issue. A \
         fleet lock standing joins nothing: record what holds it back. Keep the branch and \
         PR, and name the repo and PR in your record.\n\n\
         ## Cut an installed fix, then check it\n\n\
         Joining is not landing, and landing code is not fixing an installed tool. When the \
         fix must reach an installed tool — afkconvoy itself, for example — wait until the \
         change lands, then have that repo's fleet cut a version through its declared cut \
         steps (`afkconvoy cut --help`). Wake its checkout and read `afkconvoy plan`'s `cut`: \
         the next idle tick starts a due cut. Fix what holds that work back; do not bypass \
         the gate or train, tag by hand, or manufacture a cut record. If a cut failed for a \
         mechanical cause, fix it and continue with the `cut --from <step>` line its report \
         gives. If cutting is off or a human decision holds it back, say so.\n\n\
         Check that the cut published and installed the version containing your change. \
         For afkconvoy, let the host runner move onto it by itself; read `doctor --all`'s \
         `host.installed` for what holds the move back and `checkouts` for its version. Never \
         kill or restart the host runner. On the new version repeat the check that showed \
         the fault and check the fleets' work goes on. Record the version, the evidence and \
         whether the fault is gone or still present; an upgrade alone proves nothing.\n\n\
         This watch has one hour from {}, its sessions together; at the hour it is ended. End \
         with what held the work back, each thing you did and its evidence, each PR and cut, \
         and what you left — including every human decision — a line each: what you print \
         is its record. If landing, cutting or the move has not finished, say what is still \
         pending; never report the fix verified before checking it.\n\n\
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
        let id = if what.contains(crate::doctor::Kind::KitUnplaced.symptom()) {
            format!("{}{what}", host_watch::RESEARCH_ITEM)
        } else {
            format!("finding:{at}:{what}")
        };
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

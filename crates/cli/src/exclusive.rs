//! `afkconvoy exclusive`'s effect half (docs/adr/0019): a heavy command run
//! once no other `exclusive` command runs on the host, across all its fleets.
//!
//! The queue is the host runner's (docs/adr/0023), held in its process for
//! every fleet on the host whatever its repo (`Queue`). Each command waiting
//! or running asks it on its socket, naming its checkout, and stays on the
//! line: it is told there once its turn comes, and leaves the queue as its
//! line closes — its command ended, or its process did. Which ticket runs is
//! the core's to say (`afkconvoy_core::exclusive`).

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use afkconvoy_core::exclusive::{
    Gate, Note, Ticket, Work, at_once_line, ended_line, queued_since, rejoins_line, runs_line,
    waits_line,
};
use afkconvoy_core::role::ISSUE_ENV;
use afkconvoy_gate::holder::{self, Bound, Holder, Host};
use anyhow::{Result, bail};
use serde_json::json;

use crate::acting::Acting;
use crate::git::Repo;
use crate::runner_socket::{self, Conversation, Kept, Reached, Reply, Request};

/// How long a command whose host runner ended as it waited — or one that
/// finds a host runner starting or stopping — asks again for one to answer,
/// before it runs in no queue: a host runner replaced is back within it.
const REACHED_WITHIN: Duration = Duration::from_secs(60);
/// How often it asks again meanwhile.
const ASKS_EVERY: Duration = Duration::from_millis(200);
/// How long a host runner is given to say where a command stands in its
/// queue: it says it at once.
const PLACED_WITHIN: Duration = Duration::from_secs(10);

/// The default bound on time holding other commands up (docs/adr/0031).
pub const SECONDS: u64 = 1200;
/// Reserved for a command ended at the exclusive bound. A child's own use
/// of it is reported as exit 1, so this outcome is unambiguous.
pub const BOUND_CODE: u8 = 124;
const CHECK_EVERY: Duration = Duration::from_millis(100);

/// The worktree, rather than a caller flag, identifies a cut's commands.
fn is_cut(repo: &Repo) -> bool {
    repo.dir.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
        afkconvoy_core::worker::is_worktree_named(name, afkconvoy_core::cut::CHECKOUT)
    })
}

/// Which of the two gates (docs/adr/0047) the command `exclusive` runs is,
/// as `gate --fast` and `--slow` say it to the gate's command: `fast` takes
/// the fast lane (docs/adr/0049). It is said to that `exclusive` alone, and
/// not passed on to what its command runs.
pub const GATE_ENV: &str = "AFKCONVOY_GATE";

/// The gate `exclusive` was told it runs; `None` for any other command.
fn told_gate() -> Option<Gate> {
    match std::env::var(GATE_ENV).ok()?.as_str() {
        "fast" => Some(Gate::Fast),
        "slow" => Some(Gate::Slow),
        _ => None,
    }
}

/// Said by a gate's run to the `exclusive` its command takes a lane in: once
/// that lane is held, give the gate's tests their share of the host's cores
/// if another command runs in the host's lanes (`share`). Not passed on.
pub const SHARE_ENV: &str = "AFKCONVOY_GATE_SHARE";

/// The variable that keeps a gate's tests to its share of the host's cores,
/// where it runs beside others (sunfmin/control#245).
const SHARED: [&str; 1] = ["NEXTEST_TEST_THREADS"];

/// Gives a gate's tests, run as `command` in the lane of the process `own`,
/// their share of the host — the cores over every lane a gate may run in at
/// once — as `NEXTEST_TEST_THREADS`, unless the environment already says it,
/// and only while another command runs in a worker, fast or slow lane. Tests
/// run side by side would otherwise each take every core, and a test far
/// under the fast gate's line would cross its end on load alone
/// (sunfmin/control#245). A gate alone in the queue keeps every core: held to
/// a third of 14 cores beside lanes standing idle, a repo's tests took 461
/// seconds that had taken 311 with all of them (sunfmin/control#318). Its
/// build is not held to a share: a build is timed by no line of its own, and
/// one cold build on one core of eight passed the fast gate's limit before
/// its tests began (2026-10-29), as ADR-0019 says of a cap on build jobs.
pub(crate) fn share(command: &mut Command, own: u32) {
    command.env_remove(SHARE_ENV);
    if !beside_another(own) {
        return;
    }
    let Ok(settings) = crate::host_settings::of_host() else { return };
    let share = settings.gate_share().to_string();
    for name in SHARED {
        if std::env::var_os(name).is_none() {
            command.env(name, &share);
        }
    }
}

/// Whether a command other than the one of the process `own` runs in this
/// host's worker, fast or slow lanes; a light gate's repo lane is none of
/// them. A queue that cannot be read is taken as one beside it.
fn beside_another(own: u32) -> bool {
    standing().map_or(true, |tickets| {
        tickets.iter().any(|t| t.running() && t.repo_lane.is_none() && t.pid != own)
    })
}

/// Only the gate's descendants inherit this lease. Nested `exclusive`
/// wrappers use the already held lane instead of waiting on their parent.
const OWNER_ENV: &str = "AFKCONVOY_EXCLUSIVE_OWNER";
thread_local! { static HELD: Cell<bool> = const { Cell::new(false) }; }
thread_local! { static SLOT: RefCell<Option<crate::slots::Slot>> = const { RefCell::new(None) }; }

pub fn held() -> bool {
    HELD.get()
}

/// Pass the command's lease only into the gate it owns.
pub fn inherit(command: &mut Command, repo: &Repo) -> Result<()> {
    if held() {
        let scope = lease_scope(repo)?;
        command.env(OWNER_ENV, format!("{}:{scope}", std::process::id()));
        SLOT.with_borrow(|slot| {
            if let Some(slot) = slot {
                slot.inherit(command);
            }
        });
    }
    Ok(())
}

fn lease_scope(repo: &Repo) -> Result<String> {
    let repo = afkconvoy_core::gate::key(&canonical(repo.common_dir()?));
    let cache = afkconvoy_core::gate::key(&canonical(&crate::copies::cache()?));
    Ok(format!("{repo:016x}:{cache:016x}"))
}

/// The process whose lane this one runs in, as a descendant of it: `None`
/// where it holds none.
fn inherited(checkout: Option<&Repo>) -> Option<u32> {
    let (owner, scope) = std::env::var(OWNER_ENV).ok().and_then(|v| {
        let (owner, scope) = v.split_once(':')?;
        Some((owner.parse::<u32>().ok()?, scope.to_string()))
    })?;
    if checkout.and_then(|repo| lease_scope(repo).ok()).as_deref() != Some(&scope) {
        return None;
    }
    let processes = crate::hot::listed();
    let mut pid = std::process::id();
    for _ in 0..processes.len() {
        let parent = processes.iter().find(|p| p.pid == pid).map(|p| p.ppid)?;
        if parent == owner {
            return Some(owner);
        }
        if parent <= 1 || parent == pid {
            return None;
        }
        pid = parent;
    }
    None
}

/// A command owns its lane until this value drops, including merge, gate,
/// recording and join. The socket frees it on process death. Without a
/// runner, the OS holds the same lane across local processes.
pub struct Permit {
    stop: Arc<AtomicBool>,
    monitor: Option<std::thread::JoinHandle<()>>,
}

impl Permit {
    pub fn take(repo: &Repo, command: &str, heavy: bool, is_turn: bool) -> Result<Self> {
        Self::acquire(repo, command, heavy, is_turn, false, None)
    }

    pub(crate) fn candidate(repo: &Repo, command: &str, heavy: bool) -> Result<Self> {
        Self::acquire(repo, command, heavy, true, true, None)
    }

    /// A cut's gate of its own commit (docs/adr/0028), in a worker lane. Its
    /// worktree holds one commit nothing moves while it runs, as a
    /// candidate's does: it shares its repo with candidates, and so holds no
    /// turn of that repo up. It is no turn and no candidate on any chain.
    pub(crate) fn cut_gate(repo: &Repo, command: &str, heavy: bool) -> Result<Self> {
        Self::acquire(repo, command, heavy, false, true, None)
    }

    /// A round's slow gate, in the host's slow lanes, not its worker lanes.
    pub(crate) fn slow_gate(repo: &Repo, command: &str, heavy: bool) -> Result<Self> {
        Self::acquire(repo, command, heavy, true, true, heavy.then_some(Gate::Slow))
    }

    /// One of the two gates a worker runs alone, `gate --fast` or `--slow`,
    /// in its lane (docs/adr/0049): the fast lane, or the slow lanes. Taken
    /// before the gate's command starts, so that the wait for it uses none
    /// of the gate's limit, which times the command alone (sunfmin/control#822).
    pub(crate) fn alone(repo: &Repo, command: &str, gate: Gate) -> Result<Self> {
        Self::acquire(repo, command, true, false, false, Some(gate))
    }

    fn acquire(
        repo: &Repo,
        command: &str,
        heavy: bool,
        is_turn: bool,
        candidate: bool,
        gate: Option<Gate>,
    ) -> Result<Self> {
        if held() {
            bail!("this command already holds a turn");
        }
        let ticket = Ticket {
            number: 0,
            pid: std::process::id(),
            command: command.into(),
            dir: canonical(&repo.dir),
            cut: is_cut(repo),
            repo_lane: if heavy { None } else { Some(canonical(repo.common_dir()?)) },
            repo: Some(canonical(repo.common_dir()?)),
            is_turn,
            candidate,
            note: afkconvoy_core::exclusive::Note {
                issue: std::env::var(ISSUE_ENV).ok().and_then(|v| v.parse().ok()),
                validation: None,
            },
            gate,
            asked: crate::clock(),
            ran: None,
        };
        let acting = Acting::new("exclusive").on_own_issue().in_checkout(Some(repo.clone()));
        let stop = Arc::new(AtomicBool::new(false));
        let monitor = match turn(&ticket, &acting, &crate::clock)? {
            Turn::Queued(place, _) => {
                acting.say(crate::clock(), runs_line(&ticket, crate::clock(), 0));
                let watch = Watch::new(place)?;
                let stopping = stop.clone();
                let watching = ticket.clone();
                Some(std::thread::spawn(move || {
                    let began = Instant::now();
                    while !stopping.load(Ordering::Acquire) {
                        let heard = watch.read.recv_timeout(CHECK_EVERY);
                        if matches!(heard, Err(mpsc::RecvTimeoutError::Disconnected)) {
                            std::thread::sleep(CHECK_EVERY);
                        }
                        if let Ok(bound) = heard {
                            // Freeze each child before walking below it. The
                            // owning CLI remains able to reap and exit.
                            for child in
                                crate::hot::listed().iter().filter(|p| p.ppid == std::process::id())
                            {
                                end_descendants(child.pid);
                            }
                            let line = format!(
                                "ended at the exclusive bound of {}s after {}s with {} commands waiting: `{}`",
                                bound.bound,
                                began.elapsed().as_secs(),
                                bound.waiting,
                                watching.command
                            );
                            acting.say(crate::clock(), line.clone());
                            eprintln!("[afkconvoy] exclusive: {line}");
                            println!("{}", json!({"error": line}));
                            std::process::exit(i32::from(BOUND_CODE));
                        }
                    }
                }))
            }
            Turn::AtOnce(_) => None,
        };
        let slot = crate::slots::Slot::take(&ticket)?;
        SLOT.with_borrow_mut(|held| *held = Some(slot));
        HELD.set(true);
        Ok(Self { stop, monitor })
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
        HELD.set(false);
        SLOT.with_borrow_mut(|held| *held = None);
    }
}

/// The host's exclusive queue, as its host runner holds it: the gate
/// crate's holder (`afkconvoy_gate::holder`), told this host's lanes and
/// bound by its settings, the leases a gate's descendants inherit, and the
/// cleanup a candidate's lane waits on. A clone is the same.
#[derive(Clone)]
pub struct Queue {
    holder: Holder<Note, Hosted>,
}

impl Default for Queue {
    fn default() -> Self {
        Self::with_leases(true)
    }
}

/// What only afkconvoy knows of the queue its host runner holds.
struct Hosted {
    /// Whether the leases a gate's descendants inherit are counted: a test's
    /// queue counts none.
    leases: bool,
    /// The display copy of each landing turn's candidate, by its process,
    /// for an inherited lease of it too.
    validations: Mutex<BTreeMap<u32, afkconvoy_core::candidate::Validation>>,
    /// How long each command ran, by what it is called (`Ran::key`), its
    /// last `RAN_KEPT` runs: what admission estimates the queue's drain by.
    ran: Mutex<Ran>,
}

/// How many runs of one command the queue keeps the seconds of.
pub const RAN_KEPT: usize = 5;

/// How long the commands the queue ran took, in seconds, the last
/// `RAN_KEPT` runs of each, oldest first, by what each is called
/// (`Ran::key`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ran(pub BTreeMap<String, Vec<u64>>);

impl Ran {
    /// What a command is called by, whichever worktree runs it: its status
    /// command.
    pub fn key(ticket: &Ticket) -> String {
        afkconvoy_core::exclusive::status_command(&ticket.command)
    }

    /// Keeps one more run of `key`, of `seconds`.
    pub fn ran(&mut self, key: String, seconds: u64) {
        let runs = self.0.entry(key).or_default();
        runs.push(seconds);
        if runs.len() > RAN_KEPT {
            runs.remove(0);
        }
    }

    /// How long `ticket` takes by its recent runs: their median. `None`: no
    /// run of it is kept.
    pub fn takes(&self, ticket: &Ticket) -> Option<u64> {
        let mut runs = self.0.get(&Self::key(ticket))?.clone();
        runs.sort_unstable();
        runs.get(runs.len() / 2).copied()
    }
}

impl Host<Note> for Hosted {
    fn now(&self) -> u64 {
        crate::clock()
    }

    fn lanes(&self) -> afkconvoy_core::exclusive::Lanes {
        crate::host_settings::of_host().unwrap_or_default().lanes()
    }

    // A changed host setting applies to the command already running. An
    // unreadable file leaves the default in force.
    fn bound(&self) -> u64 {
        crate::host_settings::of_host().unwrap_or_default().exclusive_seconds()
    }

    fn beside(&self) -> std::result::Result<Vec<Ticket>, String> {
        if !self.leases {
            return Ok(vec![]);
        }
        let validations = self.validations.lock().unwrap_or_else(PoisonError::into_inner);
        let mut leases = crate::slots::standing().map_err(|error| format!("{error:#}"))?;
        for lease in &mut leases {
            if let Some(validation) = validations.get(&lease.pid) {
                lease.note.validation = Some(validation.clone());
            }
        }
        Ok(leases)
    }

    // A landing turn's candidate, as it last reported it.
    fn shown(&self, ticket: &mut Ticket) {
        if ticket.is_turn
            && let Some(validation) =
                self.validations.lock().unwrap_or_else(PoisonError::into_inner).get(&ticket.pid)
        {
            ticket.note.validation = Some(validation.clone());
        }
    }

    fn grantable(&self, ticket: &Ticket) -> bool {
        crate::slots::repo_available(ticket).unwrap_or(false)
    }

    fn leaving(&self, ticket: &Ticket) -> bool {
        if !ticket.candidate {
            return true;
        }
        let checkout = ticket
            .repo
            .as_deref()
            .and_then(|path| Path::new(path).parent())
            .and_then(Path::to_str)
            .unwrap_or(&ticket.dir);
        match crate::candidates::abandoned(checkout) {
            Ok(_) => true,
            Err(error) => {
                eprintln!("[afkconvoy] candidate cleanup unconfirmed; lane retained: {error}");
                false
            }
        }
    }
}

impl Queue {
    fn with_leases(leases: bool) -> Self {
        let hosted = Hosted { leases, validations: Mutex::default(), ran: Mutex::default() };
        Self { holder: Holder::new(hosted) }
    }

    /// Display copies never authorize a grant, invalidation or publication.
    pub fn validation(&self, pid: u32, validation: afkconvoy_core::candidate::Validation) {
        self.holder
            .host()
            .validations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pid, validation);
    }

    /// Sends waiting commands to the next host runner. None is running when
    /// a release move calls this; closing also works when that move fails.
    pub fn close(&self) {
        self.holder.close();
    }

    /// Pauses new commands atomically with reading those already running.
    /// Waiting commands rejoin the replacement with their original ask time.
    pub fn pause(&self) -> usize {
        self.holder.pause()
    }

    /// Lets commands start again if the installed release changed before
    /// the host runner could move onto it.
    pub fn resume(&self) {
        self.holder.resume();
    }

    /// How long the commands it ran took (`Ran`).
    pub fn ran(&self) -> Ran {
        self.holder.host().ran.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Keeps how long the ticket `number` ran, where it ran, as it leaves.
    fn ended(&self, number: u64) {
        let Ok((tickets, _)) = self.holder.observation() else { return };
        let Some(ticket) = tickets.iter().find(|ticket| ticket.number == number) else { return };
        let Some(ran) = ticket.ran else { return };
        let seconds = crate::clock().saturating_sub(ran);
        let mut kept = self.holder.host().ran.lock().unwrap_or_else(PoisonError::into_inner);
        kept.ran(Ran::key(ticket), seconds);
    }

    /// Tickets and whether new grants are paused, under the same lock.
    pub fn observation(&self) -> Result<(Vec<Ticket>, bool)> {
        self.holder.observation().map_err(|error| anyhow::anyhow!(error))
    }

    /// Running tickets first, then waiters, each by ask time and number.
    pub fn standing(&self) -> Vec<Ticket> {
        self.observation().expect("test queue is readable").0
    }

    /// Answers `exclusive` asking for `ticket`: it joins the queue, and its
    /// asker is told at once how many are `ahead` of it, and later that it
    /// `runs` — on the line it is kept on, as long as it stays there.
    pub fn answer(&self, ticket: Ticket) -> Reply {
        if let Err(error) = crate::slots::recover() {
            return Reply::Line(
                json!({"error": format!("candidate cleanup unconfirmed: {error}")}),
            );
        }
        let (number, ahead) = self.holder.join(ticket);
        let queue = self.clone();
        Reply::Kept(json!({"ahead": ahead}), Box::new(move |kept| queue.hold(number, kept)))
    }

    /// Holds the ticket `number` for its asker on `kept`, telling it once
    /// none is ahead of it: it leaves the queue once the asker's line closes,
    /// or where it cannot be told.
    fn hold(&self, number: u64, mut kept: Kept) {
        let queue = self.clone();
        kept.on_close(move || {
            queue.ended(number);
            queue.holder.leave(number);
        });
        let ran = match self.holder.turn(number) {
            holder::Turn::Runs(ran) => ran,
            holder::Turn::Closed => {
                kept.close();
                return;
            }
            // Its asker left as it waited.
            holder::Turn::Left => return,
        };
        if !kept.say(json!({"runs": ran, "ahead_of": 0})) {
            self.holder.leave(number);
            return;
        }
        if let Some(bound) = self.holder.overdue(number) {
            // Do not give the next command its turn before cleanup.
            kept.say(json!(bound));
        }
    }
}

/// Listen while the command runs. Closing the conversation on drop both
/// releases its ticket and unblocks the reader; no thread or socket remains.
struct Watch {
    read: mpsc::Receiver<Bound>,
    end: Option<Box<dyn FnOnce() + Send>>,
    reader: Option<std::thread::JoinHandle<Conversation>>,
}

impl Watch {
    fn new(mut place: Conversation) -> Result<Self> {
        let end = Box::new(place.ender()?);
        let (send, read) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            while let Ok(Some(said)) = place.heard() {
                if let Ok(bound) = serde_json::from_value::<Bound>(said) {
                    let _ = send.send(bound);
                    break;
                }
            }
            // Keep the ticket even after the host runner's line closes or
            // it asks for the command to end: the owning thread cleans up.
            place
        });
        Ok(Self { read, end: Some(end), reader: Some(reader) })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(end) = self.end.take() {
            end();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// The host's exclusive queue, as its host runner says it, in the order it
/// runs; empty where no host runner runs. Errors: the host runner could not
/// be asked, or said no queue it holds.
pub fn standing() -> Result<Vec<Ticket>> {
    Ok(crate::host_runner::shared()?.map(|shared| shared.exclusive).unwrap_or_default())
}

/// The work `queue`'s running tickets run, as `procs` — one reading of the
/// process table — finds it: what admission discounts of the host's load
/// (docs/adr/0050) and the hot sampler of a worker's memory (docs/adr/0056).
pub(crate) fn running_work(queue: &[Ticket], procs: &[crate::hot::Proc]) -> Work {
    let table: Vec<(u32, u32)> = procs.iter().map(|proc| (proc.pid, proc.ppid)).collect();
    afkconvoy_core::exclusive::running_work(queue, &table)
}

/// Since when the worker in the worktree `dir` has waited in this host's
/// queue, or run what it waited for; `None`: it has nothing there. Soft: a
/// queue that cannot be read holds nothing.
pub fn queued_in(dir: &Path) -> Option<u64> {
    queued_since(&standing().ok()?, &canonical(dir))
}

/// `dir` as a ticket names it: whole, with no link in it.
fn canonical(dir: &Path) -> String {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()).to_string_lossy().into_owned()
}

/// How a command's turn came.
enum Turn {
    /// The host runner says it runs: the line, held open for as long as it
    /// runs, holds its place.
    Queued(Conversation, usize),
    /// It runs in no queue, as this says why.
    AtOnce(String),
}

/// Waits in the host runner's queue for `ticket`'s turn, saying to `acting`
/// and stderr how it waits. One whose host runner ends as it waits asks the
/// next, as it first asked, for `REACHED_WITHIN`; one that finds no host
/// runner, or one that cannot hold it, runs at once. Errors: the host runner
/// could not be asked, or what it said could not be read.
fn turn(ticket: &Ticket, acting: &Acting, now: &dyn Fn() -> u64) -> Result<Turn> {
    let asked = Request::Exclusive { ticket: ticket.clone() };
    // Since when it has asked with no host runner placing it.
    let mut lost: Option<Instant> = None;
    loop {
        let (reached, conversation) = runner_socket::keep_asking(&asked, PLACED_WITHIN)?;
        match (reached, conversation) {
            (Reached::Answer(answer), Some(mut conversation)) => {
                if let Some(error) = answer.get("error") {
                    let why = format!(
                        "the host runner cannot hold it: {}",
                        error.as_str().unwrap_or_default()
                    );
                    return Ok(Turn::AtOnce(why));
                }
                let waiting = answer["ahead"].as_u64().unwrap_or(0);
                if waiting > 0 {
                    let line = waits_line(ticket, usize::try_from(waiting).unwrap_or(usize::MAX));
                    eprintln!("[afkconvoy] exclusive: {line}");
                    acting.say(now(), line);
                }
                // What it cannot read, a newer host runner's, is passed over.
                while let Some(said) = conversation.heard()? {
                    if said.get("runs").is_some() {
                        let ahead_of = said["ahead_of"].as_u64().unwrap_or(0);
                        return Ok(Turn::Queued(
                            conversation,
                            usize::try_from(ahead_of).unwrap_or(usize::MAX),
                        ));
                    }
                }
                let line = rejoins_line(ticket);
                eprintln!("[afkconvoy] exclusive: {line}");
                acting.say(now(), line);
                lost = Some(Instant::now());
            }
            (Reached::Nobody, _) if lost.is_none() && !crate::host_runner::runs() => {
                return Ok(Turn::AtOnce("no host runner runs on this host".to_string()));
            }
            _ => {
                let since = *lost.get_or_insert_with(Instant::now);
                if since.elapsed() >= REACHED_WITHIN {
                    let why = format!(
                        "no host runner placed it in its queue within {}s",
                        REACHED_WITHIN.as_secs()
                    );
                    return Ok(Turn::AtOnce(why));
                }
                std::thread::sleep(ASKS_EVERY);
            }
        }
    }
}

/// `afkconvoy exclusive -- <command>`: runs `command` in the directory `dir`
/// once a worker lane is available on this host, waiting in the host runner's
/// queue until then → its exit status. Heavy commands use worker lanes;
/// light gates use a lane of their repo. `checkout` is the
/// checkout `dir` is in, where it is in one: its ticket names it, and its tick
/// log is where the queue's own words go, as well as to stderr. What it prints
/// is the command's. With no host runner, or one that cannot hold it, it runs
/// the command at once, saying so on stderr (`turn`). It fails before the
/// command runs with one JSON object and exit 125, and with 127 for a command
/// that cannot be started.
pub fn exclusive(
    command: &[OsString],
    dir: &Path,
    checkout: Option<Repo>,
    now: impl Fn() -> u64,
) -> ExitCode {
    if let Some(owner) = inherited(checkout.as_ref()) {
        let mut nested = Command::new(&command[0]);
        if std::env::var_os(SHARE_ENV).is_some() {
            share(&mut nested, owner);
        }
        return match crate::process::status(nested.args(&command[1..]).current_dir(dir)) {
            Ok(status) => ExitCode::from(ended(status).0),
            Err(error) => failed(&format!("the nested command could not start: {error}")),
        };
    }
    let shown: Vec<String> =
        command.iter().map(|word| word.to_string_lossy().into_owned()).collect();
    let ticket = Ticket {
        number: 0,
        pid: std::process::id(),
        command: shown.join(" "),
        dir: canonical(checkout.as_ref().map_or(dir, |repo| &repo.dir)),
        cut: checkout.as_ref().is_some_and(is_cut),
        repo_lane: None,
        repo: checkout.as_ref().and_then(|repo| repo.common_dir().ok()).map(canonical),
        is_turn: false,
        candidate: false,
        note: afkconvoy_core::exclusive::Note {
            issue: std::env::var(ISSUE_ENV).ok().and_then(|issue| issue.parse().ok()),
            validation: None,
        },
        gate: told_gate(),
        asked: now(),
        ran: None,
    };
    let acting = Acting::new("exclusive").on_own_issue().in_checkout(checkout);
    let place = match turn(&ticket, &acting, &now) {
        Ok(Turn::Queued(place, ahead_of)) => {
            let started = now();
            acting.say(started, runs_line(&ticket, started, ahead_of));
            Some(place)
        }
        Ok(Turn::AtOnce(why)) => {
            let line = at_once_line(&ticket, &why);
            eprintln!("[afkconvoy] exclusive: {line}");
            acting.say(now(), line);
            None
        }
        Err(error) => return failed(&format!("{error:#}")),
    };
    let ticket = Ticket { ran: Some(now()), ..ticket };
    let watch = match place.map(Watch::new).transpose() {
        Ok(watch) => watch,
        Err(error) => return failed(&format!("{error:#}")),
    };
    let slot = match crate::slots::Slot::take(&ticket) {
        Ok(slot) => slot,
        Err(error) => return failed(&format!("{error:#}")),
    };
    let status = run_command(command, dir, watch.as_ref(), &ticket, &acting, &now, &slot);
    // Its place is free the moment the command ends.
    drop(watch);
    let (code, ended) = match status {
        Ok(None) => return ExitCode::from(BOUND_CODE),
        Ok(Some(status)) => ended(status),
        Err(error) => {
            eprintln!("[afkconvoy] exclusive: `{}` could not be started: {error}", shown[0]);
            (127, "could not be started".to_string())
        }
    };
    acting.say(now(), ended_line(&ticket, &ended, now()));
    ExitCode::from(code)
}

/// Wait for the child or the bound, preserving its streams. On the bound,
/// end its descendants and reap it before releasing its ticket.
/// `None` is the bound's outcome; it has its own single log line.
fn run_command(
    command: &[OsString],
    dir: &Path,
    watch: Option<&Watch>,
    ticket: &Ticket,
    acting: &Acting,
    now: &dyn Fn() -> u64,
    slot: &crate::slots::Slot,
) -> std::io::Result<Option<ExitStatus>> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]).current_dir(dir).env_remove(GATE_ENV);
    if std::env::var_os(SHARE_ENV).is_some() {
        share(&mut cmd, ticket.pid);
    }
    slot.inherit(&mut cmd);
    let mut child = crate::process::spawn(&mut cmd)?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if let Some(bound) = watch.and_then(|watch| watch.read.try_recv().ok()) {
            end_descendants(child.id());
            child.wait()?;
            let line = format!(
                "ended at the exclusive bound of {}s after {}s with {} commands waiting: `{}`; \
                 run each step of the work as its own `afkconvoy exclusive`",
                bound.bound,
                started.elapsed().as_secs(),
                bound.waiting,
                ticket.command
            );
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            eprintln!("[afkconvoy] exclusive: {line}");
            acting.say(now(), line);
            return Ok(None);
        }
        std::thread::sleep(CHECK_EVERY);
    }
}

/// Stop each process before discovering its children, repeating until none
/// can fork another. Keep the inherited process group: a gate that reaches
/// its own limit must still be able to end exclusive and its command together.
fn end_descendants(root: u32) {
    let mut stopped = vec![root];
    // SAFETY: root is our unreaped child's pid, never the caller's group.
    unsafe {
        libc::kill(root as i32, libc::SIGSTOP);
    }
    loop {
        let procs = crate::hot::listed();
        let children: Vec<_> = procs
            .iter()
            .filter(|proc| stopped.contains(&proc.ppid) && !stopped.contains(&proc.pid))
            .map(|proc| proc.pid)
            .collect();
        if children.is_empty() {
            break;
        }
        for pid in children {
            // SAFETY: these are descendants of the stopped child.
            unsafe {
                libc::kill(pid as i32, libc::SIGSTOP);
            }
            stopped.push(pid);
        }
    }
    for pid in stopped.into_iter().rev() {
        // SAFETY: each pid was found under the stopped child above.
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

/// The exit code a command that ended with `status` is passed on with — a
/// shell's for one a signal ended, 128 and the signal — and how it ended, in
/// words.
fn ended(status: ExitStatus) -> (u8, String) {
    match (status.code(), status.signal()) {
        (Some(code), _) if code == i32::from(BOUND_CODE) => (
            1,
            format!("exit {code}, reported as 1: {BOUND_CODE} is reserved for the exclusive bound"),
        ),
        (Some(code), _) => ((code & 0xff) as u8, format!("exit {code}")),
        (None, Some(signal)) => {
            ((128 + signal).clamp(0, 255) as u8, format!("killed by signal {signal}"))
        }
        (None, None) => (1, "ended unexplained".to_string()),
    }
}

/// A failure before the command ran: of the queue's own, or of where it
/// would run.
pub fn failed(message: &str) -> ExitCode {
    println!("{}", json!({ "error": format!("exclusive: {message}; the command did not run") }));
    ExitCode::from(125)
}

/// What `afkconvoy exclusive --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

For work you know to be heavy: a build, a test suite or a benchmark that takes most of the host's cores, memory or disk for minutes. You know it from a watch that typed at you that the work really is that heavy, or from your role's rules that name the command. Put `afkconvoy exclusive --` in front of that command, every time you run it from then on:

```
afkconvoy exclusive -- cargo test --workspace
```

Not for everyday commands — `git`, a single test, a quick build: the queue makes each one wait for every heavy one before it, of every fleet on this host.

Split a long measurement up front. Run each step that needs a quiet host as its own `afkconvoy exclusive`, so landing turns can run between steps.

## Who runs it

A worker, in its own worktree. Anyone else on the host may too: the queue is the host's, not a fleet's.

## What it reads and writes

Commands use worker lanes, {const:GATE_LANES} by default. `run --gate-lanes N` sets the count live across the host, and `default` restores it. N must be above zero. Independent candidate worktrees of one repo may validate together; ordinary heavy commands retain same-repo exclusion. Heavy gates, tests, benchmarks and builds use these lanes in FIFO order; a light gate takes its repo's own lane. A candidate holds its worker lane while it builds and gates; a round's slow gate takes one of the host's slow lanes, 1 by default, which `run --slow-gate-lanes N` sets live. A gate alone in the queue keeps every core; one that starts while another command runs in a worker, fast or slow lane is given its share of the host's cores — its cores over those lanes together, at least 1 — as `NEXTEST_TEST_THREADS`, unless its environment already sets it, and its build every core (sunfmin/control#245, sunfmin/control#318). Inherited kernel leases keep a dead owner's still-running descendants from being replaced. A server shared by the host that inherited one is no such descendant: one handed to pid 1 that works outside the worktree, as sccache's server, which a gate's build starts, works in `/` for every worktree of the host, or one under it. A lease that only such servers hold once its owner ended is free, also across a host runner restarted (sunfmin/control#822).

A worker's fast gate, run by `afkconvoy gate --fast`, takes the fast lane instead: `gate --fast` tells its command's `exclusive` that it runs the fast gate, through `AFKCONVOY_GATE`, which `exclusive` does not pass on to its command. Fast gates contend only with each other, up to the host setting `fast_lanes`, {const:FAST_LANES} by default (`run --fast-lanes N`); no slow gate, turn or other command holds a place they need, and fast gates of one repo run side by side, each in its own worktree. A slow gate run alone, `gate --slow`, keeps the worker lanes.

A cut's gate and exclusive commands go ahead of waiting worker commands, known by the cut's own worktree. Cuts keep request order among themselves, and ordinary commands keep theirs. Priority never interrupts a running command or adds a lane. A worker may therefore wait behind a cut's gate or publish step. Its runs line says "runs as a cut, ahead of waiting worker commands".

The queue is the host runner's (`afkconvoy run --help`), held in its process: `exclusive` asks it on its socket, naming its checkout, and stays on the line while it waits and while its command runs. A process that leaves it — its command ended, or it was killed while waiting or running — frees its place for the next. The command runs in the directory `exclusive` is run in, or with `-C <dir>` in that one, as named: a checkout of the repo or a directory in one, and not that checkout's top.

It writes a line to the tick log of this host for this repo as it starts to wait, when another command is ahead of it; as its command runs — in its turn, or at once in no queue, with why; as it waits again, in the next host runner's queue; and as it ends, with its exit status. Its lines name the worker or repo lane. `doctor` and the panel show each lane's running and waiting commands, with the counts in force. A turn logs the commit it merged, whether its gate was green and its landing.

While it waits or runs, the fleet counts the worker busy, not silent: it is not nudged for that. Waiting for a turn or a lane uses none of a gate's limit (`gate_seconds`, or the `seconds` of the fast or slow gate `gate --fast` or `--slow` runs): the limit times the gate command once its turn or lane is held. A worker with confirmed live background work remains waiting even past the observation interval; silence alone does not authorize escalation or continuation.

## What it answers

A running command is ended once it has held other commands up for {const:EXCLUSIVE_SECONDS}s, or the seconds the host setting `afkconvoy run --exclusive-seconds N` says (`AFKCONVOY_EXCLUSIVE_SECONDS` names it too). `--exclusive-seconds default` changes it back; 0 ends a command as soon as another waits. Only time with another command waiting counts: time running alone never uses the bound, and gaps without waiters pause it. A changed host setting applies to the command already running. On reaching the bound it ends the command and its descendants before the next command starts, exits {const:EXCLUSIVE_BOUND_CODE}, and prints one line on stderr beginning `[afkconvoy] exclusive:` telling the worker to run each step as its own `afkconvoy exclusive`. The tick log records the command, its runtime and how many commands waited; `doctor` names each recent occurrence as a warning.

Unlike every other command, no JSON of its own: what the command prints is printed, and it exits with the command's exit status — 128 and the signal for a command a signal ended. While it waits it says so on stderr, in one line beginning `[afkconvoy] exclusive:`.

With no host runner on this host it runs the command at once, in no queue, and says so on stderr: `[afkconvoy] exclusive: runs `<command>` at once, in no queue: no host runner runs on this host`. So it does too where the host runner cannot hold it — one of an older afkconvoy, which cannot read the question — saying why. A host runner that is starting or stopping is waited for, as one replaced is.

When the host runner is replaced — an upgrade moves it (`afkconvoy run --help`) — or ends, a command that waits asks the next one, within a minute, keeping its lane and when it first asked: within its lane it waits before every command that asked after it, saying so on stderr. Where no host runner places it within the minute, it runs at once, saying so. A command that runs runs on to its end: the next host runner does not know of it, and may start the next command beside it.

Exit {const:EXCLUSIVE_BOUND_CODE} is reserved for the bound: a command that itself exits {const:EXCLUSIVE_BOUND_CODE} is reported as exit 1. Every other command exit status is passed through. It exits 127 when the command cannot be started, and 125, with one JSON object {"error"} on stdout, when the host runner cannot be asked — the cache directory is not known — or what it said cannot be read, or `-C` names no directory of a checkout: the command did not run, and nothing waited.

## What trips you up

- It may wait long: as long as every heavy command ahead of it runs. Run it as you would the gate — in the background, or with a time limit of the gate limit — not under a short tool time limit that kills it while it waits.
- Killing it kills its place in the queue, not only its wait: run it again to wait from the end.
- Only commands run through it wait for one another. A heavy command run without it runs beside the one that runs alone.
- The gate, run by `afkconvoy gate`, goes through the queue only when the config's `gate_command` does — `afkconvoy exclusive -- ./gate.sh`, for a gate that fills the host by itself. The turn owns the lane before it merges; the gate command's nested `exclusive` inherits that lease and never waits on itself. Existing `gate_command: afkconvoy exclusive -- ./gate.sh` configurations keep working. Never use `afkconvoy gate` or `land` as the gate_command: config and check name the build-and-test command to set."#;

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    use serde_json::Value;

    use super::*;

    fn ticket(command: &str, asked: u64) -> Ticket {
        Ticket {
            number: 0,
            pid: std::process::id(),
            command: command.into(),
            dir: "/w/issue-7".into(),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            note: afkconvoy_core::exclusive::Note { issue: Some(7), validation: None },
            gate: None,
            asked,
            ran: None,
        }
    }

    /// An asker on the queue's line: what it was told first, and the line.
    struct Asker {
        lines: BufReader<UnixStream>,
    }

    impl Asker {
        /// Joins the queue served in `dir` (`served`) with `ticket`.
        fn joins(dir: &Path, ticket: Ticket) -> (Asker, Value) {
            let stream = runner_socket::reached(dir, SOCKET, |path| UnixStream::connect(path));
            let stream = stream.unwrap();
            let asked = serde_json::to_string(&Request::Exclusive { ticket }).unwrap();
            writeln!(&stream, "{asked}").unwrap();
            let mut asker = Asker { lines: BufReader::new(stream) };
            let first = asker.next().unwrap();
            (asker, first)
        }

        /// What it is told next, waiting a second at most; `None`: nothing.
        fn next(&mut self) -> Option<Value> {
            self.lines.get_ref().set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut line = String::new();
            self.lines.read_line(&mut line).ok()?;
            serde_json::from_str(&line).ok()
        }
    }

    fn commands(queue: &Queue) -> Vec<String> {
        queue.standing().into_iter().map(|ticket| ticket.command).collect()
    }

    /// The socket a test's queue is served on.
    const SOCKET: &str = "q.sock";

    /// Serves `queue` on a socket in `dir` as a host runner does.
    fn served(dir: &Path, queue: &Queue) {
        let listener = runner_socket::reached(dir, SOCKET, |path| UnixListener::bind(path));
        let queue = queue.clone();
        runner_socket::serve(listener.unwrap(), json!({}), move |request| match request {
            Request::Exclusive { ticket } => queue.answer(ticket),
            _ => Reply::Line(json!({})),
        });
    }

    /// Waits a while for `queue` to hold `commands`, in the order it runs.
    fn holds(queue: &Queue, commands_now: &[&str]) {
        let until = Instant::now() + Duration::from_secs(5);
        while commands(queue) != commands_now && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(commands(queue), commands_now);
    }

    #[test]
    fn one_runs_at_a_time_in_the_order_asked_and_each_leaves_as_its_line_closes() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::with_leases(false);
        served(dir.path(), &queue);
        let (mut first, said) = Asker::joins(dir.path(), ticket("a", 100));
        assert_eq!(said["ahead"], 0);
        assert!(first.next().unwrap()["runs"].is_u64(), "it runs at once");
        let (mut second, said) = Asker::joins(dir.path(), ticket("b", 101));
        assert_eq!(said["ahead"], 1);
        let (mut third, said) = Asker::joins(dir.path(), ticket("c", 102));
        assert_eq!(said["ahead"], 2);
        assert_eq!(commands(&queue), ["a", "b", "c"]);
        assert!(queue.standing()[0].running() && !queue.standing()[1].running());
        assert_eq!(second.next(), None, "waits while the first runs");

        drop(first);
        assert!(second.next().unwrap()["runs"].is_u64(), "its turn, once the first left");
        holds(&queue, &["b", "c"]);
        assert_eq!(third.next(), None);
        drop(second);
        assert!(third.next().unwrap()["runs"].is_u64());
        drop(third);
        holds(&queue, &[]);
    }

    #[test]
    fn fast_gates_of_one_repo_run_beside_a_slow_gate_and_each_other_up_to_the_fast_lanes() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::with_leases(false);
        served(dir.path(), &queue);
        let of_a = |command: &str, asked: u64, gate: Option<Gate>| Ticket {
            repo: Some("/a/.git".into()),
            gate,
            ..ticket(command, asked)
        };
        let (mut slow, _) = Asker::joins(dir.path(), of_a("slow", 100, Some(Gate::Slow)));
        assert!(slow.next().unwrap()["runs"].is_u64());
        let (mut other, said) = Asker::joins(dir.path(), of_a("cargo test", 101, None));
        assert_eq!(said["ahead"], 0, "a slow gate in its own lane holds no worker lane");
        assert!(other.next().unwrap()["runs"].is_u64());
        let (mut first, said) = Asker::joins(dir.path(), of_a("fast 1", 102, Some(Gate::Fast)));
        assert_eq!(said["ahead"], 0, "nothing in the worker lane is ahead of a fast gate");
        assert!(first.next().unwrap()["runs"].is_u64(), "it runs beside the slow gate");
        let (mut second, _) = Asker::joins(dir.path(), of_a("fast 2", 103, Some(Gate::Fast)));
        assert!(second.next().unwrap()["runs"].is_u64(), "and beside a fast gate of its repo");
        let (mut third, said) = Asker::joins(dir.path(), of_a("fast 3", 104, Some(Gate::Fast)));
        assert_eq!(said["ahead"], 2);
        assert_eq!(third.next(), None, "the fast lanes are full");
        drop(first);
        assert!(third.next().unwrap()["runs"].is_u64(), "a fast lane's place is free");
        drop((slow, other));
    }

    #[test]
    fn one_whose_process_dies_as_it_waits_leaves_and_one_that_asked_again_keeps_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::with_leases(false);
        served(dir.path(), &queue);
        let (mut running, _) = Asker::joins(dir.path(), ticket("a", 300));
        assert!(running.next().unwrap()["runs"].is_u64());
        let (waiting, _) = Asker::joins(dir.path(), ticket("b", 301));
        // One that asked of the host runner before this one: before b.
        let (mut rejoined, said) = Asker::joins(dir.path(), ticket("c", 200));
        assert_eq!(said["ahead"], 1, "behind the one running alone");
        assert_eq!(commands(&queue), ["a", "c", "b"]);
        drop(waiting);
        holds(&queue, &["a", "c"]);
        drop(running);
        assert!(rejoined.next().unwrap()["runs"].is_u64());
    }

    #[test]
    fn a_release_move_pauses_queue_grants_and_closes_waiters_without_running_them() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::with_leases(false);
        served(dir.path(), &queue);
        let (mut running, _) = Asker::joins(dir.path(), ticket("a", 100));
        assert!(running.next().unwrap()["runs"].is_u64());
        let (mut waiting, _) = Asker::joins(dir.path(), ticket("b", 101));
        assert_eq!(queue.pause(), 1);
        drop(running);
        holds(&queue, &["b"]);
        assert_eq!(queue.pause(), 0, "the waiter is not a running command");
        assert_eq!(waiting.next(), None, "the move holds new commands");
        queue.close();
        assert_eq!(waiting.next(), None, "the waiter rejoins, never granted here");
        holds(&queue, &[]);
    }

    #[test]
    fn an_ended_command_is_passed_on_as_a_shell_would() {
        let run =
            |line: &str| crate::process::status(Command::new("sh").args(["-c", line])).unwrap();
        assert_eq!(ended(run("exit 3")), (3, "exit 3".to_string()));
        assert_eq!(ended(run("kill -9 $$")), (137, "killed by signal 9".to_string()));
    }
}

//! The host runner: the one process of the fleet on a host, holding a fleet
//! instance for each checkout on the host's checkout list (docs/adr/0023);
//! `run`, which hands it a checkout, starting it where none runs, and moving
//! it onto its own afkconvoy where it is of an older release; `stop`, which
//! takes one off the list, or with `--all` stops the host runner; and the
//! fleet instances it holds, as `checkouts` asks after them.
//!
//! The host runner is whoever holds the lock `host-runner.lock` in the cache
//! directory, so there is one for each cache directory, by construction, and
//! one socket (`runner_socket`): every command that speaks to a running fleet
//! asks it there, naming its checkout, and it answers for the fleet instance
//! of that checkout's repo — a wake, the opening of its panel — with no
//! socket of the instance's own. It never ticks: each fleet instance it
//! holds ticks on a thread of its own, on its own time, and keeps its own
//! tick log as before. What it keeps itself is its log, `host-runner.log`:
//! one line for each thing that happened to it — its start and its stop, the
//! older host runner it replaced, a checkout added, refused or removed, a
//! fleet instance started, broken, back or ended, a panic — and the lines a
//! fleet instance says that no tick log holds. A fleet instance that breaks
//! harms no other: its thread holds it again after a backoff, and it stays on
//! the list.
//!
//! `run --foreground` holds the host the same way — its lock and its socket —
//! with one fleet instance, its own checkout's, in the process that ran it,
//! and leaves the checkout list alone (`foreground`).

use std::any::Any;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use afkconvoy_core::checkouts::{OnList, on_list};
use afkconvoy_core::host_runner::{Found, backoff, found};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::checkout_list::{self, Entry, List};
use crate::copies::cache;
use crate::exclusive::Queue;
use crate::git::Repo;
use crate::host_sleep::Standings;
use crate::inspection::Turn;
use crate::instance::{self, Named, Start};
use crate::log::{self, What};
use crate::opening::Opener;
use crate::runner_socket::{self, Reached, Reply, Request};
use crate::stop::Stop;
use crate::tick::{Echo, Holder, Line};
use crate::wake::{self, Wakes};
use crate::{VERSION, host_settings, kit, launch, probe};

/// The lock whose holder is the host runner, in the cache directory. It
/// holds the holder's {pid, version, started}.
pub(crate) const LOCK: &str = "host-runner.lock";

/// The host runner's log, in the cache directory: its stdout and stderr.
const LOG: &str = "host-runner.log";

/// How long a host runner starting waits for the one before it to let the
/// host go: one that is stopping finishes the step each fleet instance is in.
const UP_WITHIN: Duration = Duration::from_secs(30);

/// How long `run` waits for its checkout's first tick: a first start may
/// write the host kit and make the startup probe before it.
const FIRST_TICK_WITHIN: Duration = Duration::from_secs(30 * 60);

/// How long `run` waits for the host runner to say its version, and
/// `checkouts` for it to say which fleet instances it holds, and anyone for
/// what its fleets share: it answers each at once, whatever its fleet
/// instances are doing.
const SAID_WITHIN: Duration = Duration::from_secs(10);

/// How long `stop` waits for a fleet instance to finish the step it is in.
const STOPPED_WITHIN: Duration = Duration::from_secs(10 * 60);

/// How long a host runner whose fleet instances have all stopped waits to
/// answer whoever asked it before it ends.
const ANSWERED_WITHIN: Duration = Duration::from_secs(10);

/// How long `doctor` waits for the host runner to say whether it holds a
/// fleet instance: it answers at once, whatever its fleet instances are doing.
const HOLDS_WITHIN: Duration = Duration::from_secs(3);

/// How often the host runner looks whether its lock file is still there.
const WATCHED_EVERY: Duration = Duration::from_millis(250);

/// The size past which the host runner's log is moved aside, to `.1`, as a
/// host runner starts.
const LOG_ROLLS_AT: u64 = 1 << 20;

/// The host runner as it holds its checkouts.
struct Runner {
    list: List,
    log: Arc<RunnerLog>,
    /// The process's: SIGTERM, SIGINT, the lock file gone.
    stop: Stop,
    /// Every fleet instance this host runner holds, by checkout root.
    held: Mutex<BTreeMap<PathBuf, Held>>,
    shared: Shared,
}

/// What the fleet instances of a host share, held by its host runner for
/// them all (docs/adr/0023): how each stands for host sleep, the one
/// inspection on the host and who waits for it, and the exclusive queue. A
/// clone is the same.
#[derive(Clone, Default)]
pub struct Shared {
    pub standings: Standings,
    pub turn: Turn,
    pub queue: Queue,
}

impl Shared {
    /// What `shared` is answered: {"exclusive": [ticket], in the order the
    /// queue runs, "inspection": the one running, or null}.
    fn said(&self) -> Value {
        json!({"exclusive": self.queue.standing(), "inspection": self.turn.running()})
    }
}

/// What the host's fleets share, as its host runner says it (`Shared`);
/// `None`: no host runner listens. Errors: the one there did not answer
/// within `SAID_WITHIN`, or answered an error — an older one, that holds none
/// of it, among them.
pub fn shared() -> Result<Option<Value>> {
    match runner_socket::ask(&Request::Shared, SAID_WITHIN)? {
        Reached::Answer(answer) => match answer.get("error") {
            Some(error) => bail!("the host runner says: {}", error.as_str().unwrap_or_default()),
            None => Ok(Some(answer)),
        },
        Reached::Nobody => Ok(None),
        Reached::Closed => bail!("the host runner ended as it was asked what its fleets share"),
        Reached::Unanswered => bail!(
            "the host runner said nothing within {}s of what its fleets share: its log, `afkconvoy \
             doctor` says where, may say why",
            SAID_WITHIN.as_secs()
        ),
    }
}

/// One checkout's fleet instance, from its start until its thread ends.
struct Held {
    /// The repo it is of (`checkout_list::repo_of`).
    repo: String,
    seen: Arc<Seen>,
    /// The stop of its start now, which `stop` tells: the thread of a broken
    /// one puts here that of each start of it again (`rested`).
    stop: Stop,
    reach: Reach,
}

/// What reaches one fleet instance through the host runner's socket: the
/// wakes heard for it, and the opening of its panel. A clone is the same.
#[derive(Clone, Default)]
struct Reach {
    wakes: Wakes,
    opener: Opener,
}

/// How a host runner holds the fleet instance of one repo, as a question
/// about it finds it.
enum HeldAs {
    /// It runs, reached so.
    Running(Reach),
    /// It broke, and waits to be started again (`Came::Broken`).
    Broken,
    /// It holds none.
    Nothing,
}

/// Answers `request`, a question about the fleet instance of the repo its
/// checkout is of — a wake, the opening of its panel, whether one runs — for
/// the one `held` finds of that repo: any worktree of it names it as well as
/// its checkout. One of a repo no fleet instance of runs is refused, saying
/// how to have one run.
fn about_instance(request: &Request, held: impl FnOnce(&str) -> HeldAs) -> Result<Value> {
    let checkout = match request {
        Request::Wake { checkout, .. }
        | Request::Panel { checkout, .. }
        | Request::Holds { checkout } => checkout,
        Request::Version
        | Request::Add { .. }
        | Request::Stop { .. }
        | Request::StopAll
        | Request::Checkouts
        | Request::Exclusive { .. }
        | Request::Shared => {
            bail!("{request:?} is no question about one fleet instance")
        }
    };
    let key = checkout_list::repo_of(&Repo::at(checkout)?)?;
    match (request, held(&key)) {
        (Request::Holds { .. }, held_as) => {
            Ok(json!({"holds": matches!(held_as, HeldAs::Running(_))}))
        }
        (Request::Wake { by, .. }, HeldAs::Running(reach)) => {
            Ok(wake::answer_json(reach.wakes.hear(by.clone())))
        }
        (Request::Panel { port, .. }, HeldAs::Running(reach)) => reach.opener.open(*port),
        (_, HeldAs::Broken) => bail!(
            "the fleet instance of {key} is broken, and waits to be started again: `afkconvoy \
             doctor` says why, and `afkconvoy run {}` starts it again now",
            checkout.display()
        ),
        _ => bail!(
            "this host runner (pid {}) holds no fleet instance of {key}: `afkconvoy run {}` adds \
             it",
            std::process::id(),
            checkout.display()
        ),
    }
}

/// How far a fleet instance has come, for whoever waits on it.
#[derive(Default)]
struct Seen {
    now: Mutex<Stage>,
    changed: Condvar,
}

#[derive(Default)]
struct Stage {
    /// Whether a `run` asked for this start: one refused then is taken off
    /// the list.
    asked: bool,
    /// Whether `stop` took its checkout off the list: its first tick, where
    /// it has yet to end, puts it back on it never.
    removed: bool,
    /// Its id, once this start is kept in its tick log.
    instance: Option<String>,
    /// What it warned of before this start's first tick ended: its startup
    /// probe's warnings among them.
    warnings: Vec<String>,
    /// Its `stop:` line, once its tick log keeps it.
    stop_line: Option<String>,
    came: Came,
    /// Whether its thread has ended: it is held no more.
    ended: bool,
    /// When it broke, while it has not been back since: a start of it again
    /// that ends its first tick is back.
    broken_since: Option<u64>,
    /// What a `run` asked of it while it was broken: it is started again
    /// now, as this asks.
    again: Option<Start>,
}

#[derive(Default)]
enum Came {
    #[default]
    Starting,
    /// Its first tick ended, and the checkout is on the list: that tick's line.
    Ticked { first_tick: String },
    /// Ended before its first tick, as a command prints its error
    /// (`error_json`); or its first tick ended and the list could not be
    /// written.
    Refused(Value),
    /// Its loop ended after its first tick, or its thread did, and why.
    Ended(String),
    /// Broken: it waits to be started again, after a backoff or once a
    /// `run` asks for it.
    Broken,
}

impl Seen {
    fn stage(&self) -> MutexGuard<'_, Stage> {
        self.now.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Its stage once its thread has ended.
    fn over(&self) -> MutexGuard<'_, Stage> {
        let mut stage = self.stage();
        while !stage.ended {
            stage = self.changed.wait(stage).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        stage
    }
}

/// The host runner's own log: its stdout, which the `run` that started it
/// points at host-runner.log. One line for each thing that happened to it,
/// one at a time whichever thread says it (`stamped`).
#[derive(Default)]
struct RunnerLog {
    one: Mutex<()>,
}

impl RunnerLog {
    fn say(&self, line: &str) {
        let line = stamped(line);
        let _one = self.one.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // A log that cannot be written costs its lines, never the host runner.
        let _ = writeln!(std::io::stdout(), "{line}");
    }
}

/// `line` as the host runner's log keeps it: stamped in UTC, and what spans
/// several lines, as an error may, folded into one.
fn stamped(line: &str) -> String {
    let parts: Vec<&str> = line.lines().map(str::trim).filter(|part| !part.is_empty()).collect();
    format!("{} {}", log::utc(crate::clock()), parts.join(" "))
}

/// An answer the host runner gave that is an error: `run` prints it as it
/// is, `host_runner` and all.
#[derive(Debug)]
pub struct Answered(pub Value);

impl std::fmt::Display for Answered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0["error"].as_str().unwrap_or_default())
    }
}

impl std::error::Error for Answered {}

/// The host runner's log: host-runner.log in the cache directory.
pub fn log_path() -> Result<PathBuf> {
    Ok(cache()?.join(LOG))
}

/// The hidden `afkconvoy host-runner`, as `run` starts it: takes the host,
/// starts every checkout on its list, and answers on its socket, until
/// SIGTERM or SIGINT, `stop --all`, or until its lock file is gone. Then it
/// lets every fleet instance finish the step it is in and keep its `stop`
/// line, answers whoever asked it, and ends, leaving the list as it is →
/// {"stopped", "pid", "checkouts"}; or, where another host runner held the
/// host all the while, {"started": false}.
pub fn serve() -> Result<Value> {
    // Before any thread starts: nothing moves it after, and nothing the
    // fleet does leans on it.
    std::env::set_current_dir("/").context("the host runner cannot work from /")?;
    let dir = cache()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let log = Arc::new(RunnerLog::default());
    {
        // A panic is a line of the log, as any thing that happened: the
        // fleet instance it broke says so after it.
        let log = Arc::clone(&log);
        std::panic::set_hook(Box::new(move |panic| log.say(&panic.to_string())));
    }
    let Some((lock, before)) = took_host(&dir.join(LOCK))? else {
        let holder = holder_of(&dir.join(LOCK));
        let said = holder.map_or("another".to_string(), |pid| format!("pid {pid}"));
        log.say(&format!("host runner not started: {said} holds this host"));
        return Ok(json!({"started": false, "holder": holder}));
    };
    let ours = identity(&lock.metadata()?);
    let stop = Stop::listen();
    let (list, pid) = (List::of_host()?, std::process::id());
    let started = runner_socket::listen().and_then(|listener| Ok((listener, list.read()?)));
    let (listener, listed) = started.inspect_err(|error| {
        log.say(&format!("host runner not started: {error:#}"));
    })?;
    log.say(&format!(
        "host runner started: afkconvoy {VERSION}, pid {pid}, {} checkouts on the list",
        listed.checkouts.len()
    ));
    let older = before["version"].as_str().filter(|was| found(VERSION, was) == Found::Older);
    if let Some(older) = older {
        log.say(&format!(
            "host runner upgraded: from afkconvoy {older} (pid {}) to afkconvoy {VERSION} (pid \
             {pid})",
            before["pid"]
        ));
    }
    let (held, shared) = (Mutex::default(), Shared::default());
    let runner = Arc::new(Runner { list, log, stop: stop.clone(), held, shared });
    runner.start_listed(listed.checkouts);
    let about = json!({"pid": pid, "version": VERSION, "log": dir.join(LOG)});
    let answerer = Arc::clone(&runner);
    let answering = runner_socket::serve(listener, about, move |request| answerer.answer(request));

    let lock_file = dir.join(LOCK);
    let still_ours = || std::fs::metadata(&lock_file).is_ok_and(|now| identity(&now) == ours);
    while !stop.told() {
        std::thread::sleep(WATCHED_EVERY);
        if !stop.told() && !still_ours() {
            runner.log.say(
                "host runner stopping: its lock file is gone, so another host runner may start",
            );
            stop.tell();
        }
    }
    if still_ours() {
        runner.log.say("host runner stopping: told to stop, each fleet instance ends its step");
        // A `run` from now on starts a host runner, which waits for this one.
        let _ = std::fs::remove_file(dir.join(runner_socket::SOCKET));
    }
    while !runner.holding().is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
    // `stop --all` among them, which is answered once they all stopped.
    answering.settled(ANSWERED_WITHIN);
    let kept = runner.list.read().map_or(0, |listed| listed.checkouts.len());
    runner.log.say(&format!("host runner stopped: {kept} checkouts kept on the list"));
    drop(lock);
    Ok(json!({"stopped": true, "pid": pid, "checkouts": kept}))
}

/// Takes the lock in `file`, waiting `UP_WITHIN` for a host runner that
/// holds it to let it go, and writes this process into it → the lock, held
/// until it is dropped or the process ends, with what the host runner before
/// wrote into it — {pid, version, started}, or null where none did; `None`
/// where it was held all the while.
fn took_host(file: &Path) -> Result<Option<(File, Value)>> {
    let cannot = || format!("cannot take the host runner's lock {}", file.display());
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(file)
        .with_context(cannot)?;
    let until = Instant::now() + UP_WITHIN;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error).with_context(cannot),
        }
    }
    let before = std::fs::read_to_string(file).ok();
    let before = before.and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let before = before.unwrap_or_default();
    let holder = json!({"pid": std::process::id(), "version": VERSION, "started": crate::clock()});
    lock.set_len(0)
        .and_then(|()| (&lock).write_all(holder.to_string().as_bytes()))
        .with_context(cannot)?;
    Ok(Some((lock, before)))
}

/// The pid the lock in `file` says holds it, read without taking it.
fn holder_of(file: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(file).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["pid"].as_u64()
}

/// Which file this is, wherever it is reached from.
fn identity(file: &std::fs::Metadata) -> (u64, u64) {
    (file.dev(), file.ino())
}

impl Runner {
    fn holding(&self) -> MutexGuard<'_, BTreeMap<PathBuf, Held>> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Starts the fleet instance of each checkout `listed`, as the `run`
    /// that added it named it, and as a `run` that named nothing else would
    /// in this process's environment. One whose repo cannot be read stays on
    /// the list, said in the log.
    fn start_listed(self: &Arc<Self>, listed: Vec<Entry>) {
        let unflagged = crate::unflagged_start();
        let mut held = self.holding();
        for Entry { checkout, named } in listed {
            let start =
                unflagged.as_ref().map_err(|error| anyhow!("{error:#}")).and_then(|start| {
                    let key = checkout_list::repo_of(&Repo::at(&checkout)?)?;
                    Ok((key, Start { named, ..start.clone() }))
                });
            match start {
                Ok((key, start)) => {
                    self.start(&mut held, checkout, key, start, false);
                }
                Err(why) => self.log.say(&format!(
                    "fleet instance not started: {}: {why:#} — it stays on the checkout list",
                    checkout.display()
                )),
            }
        }
    }

    /// Answers one question asked on the socket.
    fn answer(self: &Arc<Self>, request: Request) -> Reply {
        let held = |key: &str| {
            let holding = self.holding();
            match holding.values().find(|held| held.repo == key) {
                Some(held) if matches!(held.seen.stage().came, Came::Broken) => HeldAs::Broken,
                Some(held) => HeldAs::Running(held.reach.clone()),
                None => HeldAs::Nothing,
            }
        };
        let answered = match request {
            Request::Version => Ok(json!({})),
            Request::Add { checkout, start } => self.add(&checkout, *start),
            Request::Stop { checkout } => self.stop_checkout(&checkout),
            Request::StopAll => self.stop_all(),
            Request::Checkouts => Ok(self.checkouts()),
            Request::Exclusive { ticket } => return self.shared.queue.answer(ticket),
            Request::Shared => Ok(self.shared.said()),
            about => about_instance(&about, held),
        };
        Reply::Line(answered.unwrap_or_else(|error| crate::error_json(&error)))
    }

    /// The checkouts it holds a fleet instance of → {"held": [{checkout,
    /// instance}]}: each by its root, with that fleet instance's id, null
    /// until its start is logged. One `run` is adding is held before its
    /// first tick puts it on the list, and one `stop` took off it until it
    /// has stopped.
    fn checkouts(&self) -> Value {
        let holding = self.holding();
        let held = holding
            .iter()
            .map(|(root, held)| json!({"checkout": root, "instance": held.seen.stage().instance}));
        json!({"held": held.collect::<Vec<_>>()})
    }

    /// Puts the checkout `checkout` on the list and starts its fleet
    /// instance as `start` asks → once its first tick ended, `run`'s answer;
    /// or the refusal of a checkout of a repo held from another, of one that
    /// is no checkout, or of one whose fleet instance ended before its first
    /// tick. A checkout whose fleet instance runs already is answered with
    /// that instance, and `start` changes nothing; one on the list whose
    /// fleet instance does not run, or is broken, starts now with what the
    /// list kept of its `run`, and what `start` names laid over that.
    fn add(self: &Arc<Self>, checkout: &Path, start: Start) -> Result<Value> {
        if self.stop.told() {
            bail!(
                "the host runner (pid {}) is stopping: run again once it has stopped; its log, \
                 {}, says how far it is",
                std::process::id(),
                log_path()?.display()
            );
        }
        let repo = Repo::at(checkout)?;
        let (root, key) = (repo.dir.clone(), checkout_list::repo_of(&repo)?);
        let tick_log = log::Log::of(&repo)?.file;
        let (seen, added) = {
            let mut held = self.holding();
            let listed = self.list.read()?.checkouts;
            let known = known(&listed, &held);
            match on_list(&known, &root.display().to_string(), &key) {
                OnList::Other(first) => {
                    self.log.say(&format!(
                        "checkout refused: {}: {key} is on the checkout list as {first}",
                        root.display()
                    ));
                    bail!(
                        "{key} is on this host's checkout list already, as {first}: a host runs \
                         one fleet instance per repo. Run `afkconvoy run {first}` to have it \
                         worked from there"
                    );
                }
                OnList::Itself => {
                    let kept = listed.into_iter().find(|entry| entry.checkout == root);
                    let kept = kept.map(|entry| entry.named).unwrap_or_default();
                    let start = Start { named: start.named.or(kept), ..start };
                    match held.get(&root) {
                        Some(running) => {
                            let mut stage = running.seen.stage();
                            if stage.removed {
                                bail!(
                                    "the fleet instance of {} is stopping, told by `afkconvoy \
                                     stop`: run again once it has stopped",
                                    root.display()
                                );
                            }
                            stage.asked = true;
                            // A broken one is started again now, not after its backoff.
                            if matches!(stage.came, Came::Broken) {
                                stage.again = Some(start);
                                stage.came = Came::Starting;
                                running.seen.changed.notify_all();
                            }
                            drop(stage);
                            (Arc::clone(&running.seen), false)
                        }
                        None => {
                            (self.start(&mut held, root.clone(), key.clone(), start, true), false)
                        }
                    }
                }
                OnList::No => (self.start(&mut held, root.clone(), key.clone(), start, true), true),
            }
        };
        let mut stage = seen.stage();
        // Its probe's warnings are this `run`'s where it waited for its start.
        let waited = matches!(stage.came, Came::Starting);
        while matches!(stage.came, Came::Starting) {
            stage = seen.changed.wait(stage).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        match &stage.came {
            Came::Ticked { first_tick } => Ok(json!({
                "checkout": root,
                "repo": key,
                "added": added,
                "instance": stage.instance,
                "tick_log": tick_log,
                "first_tick": first_tick,
                "warnings": if waited { stage.warnings.clone() } else { vec![] },
            })),
            Came::Refused(refused) => Ok(refused.clone()),
            Came::Ended(why) => {
                bail!("its fleet instance ended before this run was answered: {why}")
            }
            Came::Broken => bail!(
                "its fleet instance broke before this run was answered: the host runner's log, \
                 {}, says why",
                log_path()?.display()
            ),
            Came::Starting => unreachable!("waited for above"),
        }
    }

    /// Starts the fleet instance of the checkout `root`, of the repo `key`,
    /// on a thread of its own, as `start` asks → how far it has come. Called
    /// holding `held`, so a thread that ends at once finds itself there.
    fn start(
        self: &Arc<Self>,
        held: &mut BTreeMap<PathBuf, Held>,
        root: PathBuf,
        key: String,
        start: Start,
        asked: bool,
    ) -> Arc<Seen> {
        let seen = Arc::new(Seen::default());
        seen.stage().asked = asked;
        let (stop, reach) = (self.stop.instance(), Reach::default());
        let holding = Held {
            repo: key.clone(),
            seen: Arc::clone(&seen),
            stop: stop.clone(),
            reach: reach.clone(),
        };
        held.insert(root.clone(), holding);
        let (runner, holding) = (Arc::clone(self), Arc::clone(&seen));
        std::thread::spawn(move || runner.hold(&root, &key, start, &holding, stop, reach));
        seen
    }

    /// Holds the fleet instance of the checkout `root`, of the repo `key`, on
    /// this thread, as `start` asks and `stop` tells, reached by `reach`,
    /// until it ends (`ended`). One that breaks is held again after a backoff
    /// that grows at each break in a row (`backoff`), or as soon as a `run`
    /// asks for it; every other fleet instance goes on meanwhile.
    fn hold(
        self: Arc<Self>,
        root: &Path,
        key: &str,
        mut start: Start,
        seen: &Arc<Seen>,
        mut stop: Stop,
        mut reach: Reach,
    ) {
        // What it waited before its last start, where it broke before it.
        let mut waited = None;
        // The last fleet instance it started as: the one that broke.
        let mut broke_as = String::new();
        loop {
            let begun = Instant::now();
            let heard = Arc::new(Heard {
                runner: Arc::clone(&self),
                root: root.to_path_buf(),
                key: key.to_string(),
                seen: Arc::clone(seen),
                named: start.named.clone(),
                stop: stop.clone(),
            });
            let echo: Echo = Arc::new(move |line| heard.line(line));
            let held = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let Reach { wakes, opener } = reach.clone();
                let shared = self.shared.clone();
                let holder = Holder { stop: stop.clone(), echo, wakes, opener, shared };
                Repo::at(root).and_then(|repo| instance::hold(&repo, &start, holder))
            }));
            let held = held.unwrap_or_else(|panic| {
                Err(anyhow!("it panicked: {}", panic_said(panic.as_ref())))
            });
            // However its loop ended, the threads it started end with it: a
            // start of it again starts its own.
            stop.tell();
            let Some(why) = self.ended(root, held) else { return };
            // Measured in the seconds its sleeps take.
            let second = start.knobs.second_ms;
            let up = begun.elapsed().as_millis() / u128::from(second.max(1));
            let wait = backoff(waited, u64::try_from(up).unwrap_or(u64::MAX));
            waited = Some(wait);
            let now = start.knobs.now.unwrap_or_else(crate::clock);
            let next = now.saturating_add(wait);
            let since = {
                let mut stage = seen.stage();
                stage.warnings.clear();
                broke_as = stage.instance.take().unwrap_or(broke_as);
                *stage.broken_since.get_or_insert(now)
            };
            let said = log::broken(&why, since, next, now);
            let said = format!("fleet instance broken: {}: {said}", root.display());
            let host = start.named.host.clone().unwrap_or_else(crate::this_host);
            let what = What::Broken { why, since, next };
            let entry = log::Entry { at: now, instance: broke_as.clone(), host, what };
            self.said_of(root, &entry, &said);
            let until = Instant::now() + Duration::from_millis(wait.saturating_mul(second));
            let Some((anew, reached, again)) = self.rested(root, seen, until) else {
                self.let_go(root, seen, &start, broke_as);
                return;
            };
            (stop, reach) = (anew, reached);
            if let Some(again) = again {
                start = again;
            }
        }
    }

    /// Rests the thread of the broken fleet instance of `root` until its
    /// backoff is over, at `until`, or a `run` asks for it → the stop of its
    /// next start, kept where `stop` finds it, how that start is reached, kept
    /// where a question about it finds it, and what that `run` asked; `None`
    /// where `stop` took its checkout off the list, or the host runner was
    /// told to stop, first.
    fn rested(
        &self,
        root: &Path,
        seen: &Seen,
        until: Instant,
    ) -> Option<(Stop, Reach, Option<Start>)> {
        let again = {
            let mut stage = seen.stage();
            loop {
                if stage.removed || self.stop.told() {
                    return None;
                }
                if let Some(again) = stage.again.take() {
                    break Some(again);
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    // A `run` that asks from now on waits for this start.
                    (stage.came, stage.asked) = (Came::Starting, false);
                    break None;
                }
                let rested = seen.changed.wait_timeout(stage, left.min(WATCHED_EVERY));
                stage = rested.map_or_else(|poisoned| poisoned.into_inner().0, |(stage, _)| stage);
            }
        };
        // Under the lock `stop` takes: one before this ends it here, one
        // after tells this start.
        let mut holding = self.holding();
        let held = holding.get_mut(root)?;
        if seen.stage().removed || self.stop.told() {
            return None;
        }
        (held.stop, held.reach) = (self.stop.instance(), Reach::default());
        Some((held.stop.clone(), held.reach.clone(), again))
    }

    /// Lets go of the broken fleet instance of `root`, `broke_as` as it last
    /// broke under `start`, at rest when `stop` took its checkout off the
    /// list or the host runner was told to stop: it is held no more. One
    /// `stop` took off keeps a `stop:` line in its tick log, after which it is
    /// broken no more.
    fn let_go(&self, root: &Path, seen: &Seen, start: &Start, broke_as: String) {
        let mut holding = self.holding();
        holding.remove(root);
        let mut stage = seen.stage();
        let why = if stage.removed {
            "it was broken, and `afkconvoy stop` took its checkout off the list"
        } else {
            "it was broken, and the host runner stops"
        };
        let said = format!("fleet instance ended: {}: {why}", root.display());
        if stage.removed {
            let at = start.knobs.now.unwrap_or_else(crate::clock);
            let host = start.named.host.clone().unwrap_or_else(crate::this_host);
            let what = What::Stop { line: why.to_string() };
            let entry = log::Entry { at, instance: broke_as.clone(), host, what };
            stage.stop_line = Some(entry.said());
            self.said_of(root, &entry, &said);
        } else {
            self.log.say(&said);
        }
        stage.instance = Some(broke_as).filter(|instance| !instance.is_empty());
        (stage.came, stage.ended) = (Came::Ended(why.to_string()), true);
        drop(stage);
        drop(holding);
        seen.changed.notify_all();
    }

    /// The loop of the fleet instance of `root` ended, as `held` says → why
    /// it broke, where it did: it is still held, now `Came::Broken`, and its
    /// thread holds it again. It broke where its loop ended in an error or a
    /// panic while the host runner goes on and `stop` did not take its
    /// checkout off the list — after its first tick, or before it where
    /// nobody asked for it. Otherwise it ended, and is held no more: one that
    /// ended before its first tick was refused, and a `run` that asked for it
    /// is told why and its checkout taken off the list; one `stop` took off
    /// before its first tick, it is told so.
    fn ended(&self, root: &Path, held: Result<Value>) -> Option<String> {
        let mut holding = self.holding();
        let why = match &held {
            Ok(answer) => answer.to_string(),
            Err(error) => format!("{error:#}"),
        };
        // Decided and marked under one lock: a `run` that asks meanwhile
        // finds it broken, or asked for the start that ended.
        let broke = holding.get(root).is_some_and(|holds| {
            let mut stage = holds.seen.stage();
            let at_work = match stage.came {
                Came::Starting => !stage.asked,
                Came::Ticked { .. } => true,
                Came::Refused(_) | Came::Ended(_) | Came::Broken => false,
            };
            let broke = at_work && held.is_err() && !stage.removed && !self.stop.told();
            if broke {
                stage.came = Came::Broken;
            }
            broke
        });
        if broke {
            return Some(why);
        }
        let gone = holding.remove(root)?;
        let mut stage = gone.seen.stage();
        stage.ended = true;
        let at = root.display();
        match stage.came {
            Came::Starting if stage.removed => {
                stage.came = Came::Refused(json!({"error": STOPPED_BEFORE_FIRST_TICK}));
                self.log.say(&format!("fleet instance ended: {at}: {why}"));
            }
            Came::Starting => {
                let error = held.err().unwrap_or_else(|| anyhow!("it ended before its first tick"));
                stage.came = Came::Refused(crate::error_json(&error));
                if stage.asked {
                    let removed = self.list.change(|listed| {
                        let before = listed.checkouts.len();
                        listed.checkouts.retain(|entry| entry.checkout != root);
                        listed.checkouts.len() < before
                    });
                    self.log.say(&match removed {
                        Ok(true) => format!("checkout removed: {at}: refused: {why}"),
                        Ok(false) => format!("checkout refused: {at}: {why}"),
                        Err(error) => format!(
                            "checkout refused: {at}: {why}; it stays on the checkout list, which \
                             could not be written: {error:#}"
                        ),
                    });
                } else {
                    self.log.say(&format!(
                        "fleet instance not started: {at}: {why} — it stays on the checkout \
                         list, and `afkconvoy run` there starts it again"
                    ));
                }
            }
            Came::Ticked { .. } => {
                self.log.say(&format!("fleet instance ended: {at}: {why}"));
                stage.came = Came::Ended(why);
            }
            Came::Refused(_) | Came::Ended(_) | Came::Broken => {
                self.log.say(&format!("fleet instance ended: {at}: {why}"));
            }
        }
        drop(stage);
        gone.seen.changed.notify_all();
        None
    }

    /// Says `said` in the host runner's log, and keeps `entry` in the tick
    /// log of the checkout `root`: a line the host runner writes of one of
    /// its fleet instances. A tick log that cannot keep it costs that line.
    fn said_of(&self, root: &Path, entry: &log::Entry, said: &str) {
        let kept = Repo::at(root).and_then(|repo| log::Log::of(&repo)?.append(entry));
        match kept {
            Ok(()) => self.log.say(said),
            Err(error) => {
                self.log.say(&format!("{said}; its tick log could not be written: {error:#}"))
            }
        }
    }

    /// Takes the checkout `checkout` off the list and stops its fleet
    /// instance, which finishes the step it is in → once it has stopped,
    /// `stop`'s answer. One neither on the list nor held is answered so, and
    /// nothing changes; a second checkout of a repo on the list is refused,
    /// naming the first.
    fn stop_checkout(&self, checkout: &Path) -> Result<Value> {
        let repo = Repo::at(checkout)?;
        let (root, key) = (repo.dir.clone(), checkout_list::repo_of(&repo).ok());
        let at = root.display();
        let (removed, running) = {
            let held = self.holding();
            let running = held.get(&root).map(|held| (Arc::clone(&held.seen), held.stop.clone()));
            let removed = {
                // Its first tick, ending now, waits: it adds the checkout
                // before this takes it off, or sees it taken off.
                let stage = running.as_ref().map(|(seen, _)| seen.stage());
                let removed = taken_off(&self.list, &root)?;
                if let Some(mut stage) = stage {
                    stage.removed = true;
                }
                removed
            };
            if running.is_none() && !removed {
                let known = known(&self.list.read()?.checkouts, &held);
                return not_listed(&known, &root, key.as_deref());
            }
            (removed, running)
        };
        self.log.say(&match (removed, &running) {
            (true, Some(_)) => {
                format!("checkout removed: {at}: by `afkconvoy stop`, its fleet instance stops")
            }
            (true, None) => {
                format!("checkout removed: {at}: by `afkconvoy stop`, no fleet instance of it ran")
            }
            (false, _) => format!(
                "fleet instance stopping: {at}: by `afkconvoy stop`, before its first tick put \
                 it on the checkout list"
            ),
        });
        let Some((seen, stop)) = running else {
            return Ok(stopped(&root, key.as_deref(), removed, None));
        };
        stop.tell();
        Ok(stopped(&root, key.as_deref(), removed, Some(&seen.over())))
    }

    /// Stops every fleet instance, and then the host runner, keeping the
    /// list → once every fleet instance has stopped, `stop --all`'s answer:
    /// each one stopped, and how many checkouts the list keeps.
    fn stop_all(&self) -> Result<Value> {
        self.log
            .say("host runner told to stop by `afkconvoy stop --all`: it keeps the checkout list");
        // Let go of before waiting: each one that ends takes it.
        let held: Vec<(PathBuf, Arc<Seen>)> = {
            let holding = self.holding();
            holding.iter().map(|(root, held)| (root.clone(), Arc::clone(&held.seen))).collect()
        };
        self.stop.tell();
        let stopped: Vec<Value> = held
            .into_iter()
            .map(|(root, seen)| {
                let stage = seen.over();
                json!({"checkout": root, "instance": stage.instance, "stop": stage.stop_line})
            })
            .collect();
        let kept = self.list.read()?.checkouts.len();
        Ok(json!({"stopped": stopped, "checkouts": kept}))
    }
}

/// Why a `run` waiting on a fleet instance that `stop` stopped before its
/// first tick ended is refused.
const STOPPED_BEFORE_FIRST_TICK: &str =
    "`afkconvoy stop` took the checkout off the checkout list before its first tick ended";

/// Each checkout `listed` or `held`, with the repo it is of where that can be
/// read: what `on_list` holds a checkout against.
fn known(listed: &[Entry], held: &BTreeMap<PathBuf, Held>) -> Vec<(String, Option<String>)> {
    let repo_of = |root: &Path| Repo::at(root).and_then(|repo| checkout_list::repo_of(&repo)).ok();
    let listing = listed.iter().map(|entry| (entry.checkout.clone(), repo_of(&entry.checkout)));
    let holding = held.iter().map(|(root, held)| (root.clone(), Some(held.repo.clone())));
    listing.chain(holding).map(|(root, repo)| (root.display().to_string(), repo)).collect()
}

/// Takes `root` off `list` → whether it was on it.
fn taken_off(list: &List, root: &Path) -> Result<bool> {
    list.change(|listed| {
        let before = listed.checkouts.len();
        listed.checkouts.retain(|entry| entry.checkout != root);
        listed.checkouts.len() < before
    })
}

/// `stop`'s answer for the checkout `root`, of the repo `key`, on no list
/// and held by no fleet instance: that it is not on the list; or, where
/// another checkout of its repo is, the refusal that names it.
fn not_listed(known: &[(String, Option<String>)], root: &Path, key: Option<&str>) -> Result<Value> {
    if let Some(key) = key
        && let OnList::Other(first) = on_list(known, &root.display().to_string(), key)
    {
        bail!(
            "{key} is on this host's checkout list as {first}, not as {}: a host runs one fleet \
             instance per repo. Run `afkconvoy stop {first}` to stop it",
            root.display()
        );
    }
    Ok(stopped(root, key, false, None))
}

/// `stop`'s answer: the checkout `root`, of the repo `key`; whether it was
/// `removed` from the list; and the fleet instance stopped, as its `stage`
/// ended — its id and its `stop:` line — where one ran.
fn stopped(root: &Path, key: Option<&str>, removed: bool, stage: Option<&Stage>) -> Value {
    json!({
        "checkout": root,
        "repo": key,
        "removed": removed,
        "instance": stage.and_then(|stage| stage.instance.clone()),
        "stop": stage.and_then(|stage| stage.stop_line.clone()),
    })
}

/// What a panic said, from what it unwound with: its message, where it gave
/// one.
fn panic_said(panic: &(dyn Any + Send)) -> &str {
    let text = panic.downcast_ref::<&str>().copied();
    let text = text.or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    text.unwrap_or("it said nothing")
}

/// One fleet instance as the host runner hears its lines (`Echo`).
struct Heard {
    runner: Arc<Runner>,
    root: PathBuf,
    /// The repo it is of.
    key: String,
    seen: Arc<Seen>,
    /// What its `run` named for it alone: kept on the list.
    named: Named,
    /// Its own stop, told where the list cannot be written, and once its
    /// loop ended.
    stop: Stop,
}

impl Heard {
    fn line(&self, line: Line<'_>) {
        let at = self.root.display();
        match line {
            Line::Kept(entry) => self.kept(entry),
            Line::Said(text) => self.runner.log.say(&format!("{at}: {text}")),
            Line::Warning(warning) => {
                let mut stage = self.seen.stage();
                // Those of its start are the answer's.
                if matches!(stage.came, Came::Starting) {
                    stage.warnings.push(warning.to_string());
                }
                drop(stage);
                self.runner.log.say(&format!("{at}: warning: {warning}"));
            }
        }
    }

    /// A line it kept in its tick log, which it is not said again: its
    /// start, said in the log as a fleet instance started; its `stop:` line,
    /// for whoever stopped it; its first tick, which puts its checkout on the
    /// list — unless `stop` took it off before — and answers whoever waits.
    fn kept(&self, entry: &log::Entry) {
        let at = self.root.display();
        let mut stage = self.seen.stage();
        if let What::Start { .. } = entry.what
            && stage.instance.is_none()
        {
            stage.instance = Some(entry.instance.clone());
            let (instance, host) = (&entry.instance, &entry.host);
            self.runner.log.say(&format!("fleet instance started: {at}: {instance} on {host}"));
        }
        if let What::Stop { .. } = entry.what {
            stage.stop_line = Some(entry.said());
        }
        if !matches!(entry.what, What::Tick(_)) || !matches!(stage.came, Came::Starting) {
            return;
        }
        if stage.removed {
            stage.came = Came::Refused(json!({"error": STOPPED_BEFORE_FIRST_TICK}));
            drop(stage);
            self.seen.changed.notify_all();
            return;
        }
        let listed = self.runner.list.change(|listed| {
            let named = self.named.clone();
            match listed.checkouts.iter_mut().find(|entry| entry.checkout == self.root) {
                Some(entry) => {
                    entry.named = named;
                    false
                }
                None => {
                    listed.checkouts.push(Entry { checkout: self.root.clone(), named });
                    true
                }
            }
        });
        stage.came = match listed {
            Ok(added) => {
                if added {
                    self.runner.log.say(&format!("checkout added: {at} ({})", self.key));
                }
                if let Some(since) = stage.broken_since.take() {
                    let (instance, host) = (entry.instance.clone(), entry.host.clone());
                    let said = log::back(since, entry.at);
                    let said = format!("fleet instance back: {at}: {instance}, {said}");
                    let back =
                        log::Entry { at: entry.at, instance, host, what: What::Back { since } };
                    self.runner.said_of(&self.root, &back, &said);
                }
                Came::Ticked { first_tick: entry.said() }
            }
            Err(error) => {
                self.stop.tell();
                let why = format!("the checkout list could not be written: {error:#}");
                self.runner.log.say(&format!("checkout refused: {at}: {why}"));
                Came::Refused(json!({"error": format!("{why}; its fleet instance was stopped")}))
            }
        };
        drop(stage);
        self.seen.changed.notify_all();
    }
}

/// `afkconvoy run`: has the host say what `settings` names of its host
/// settings, hands the checkout `checkout` to this host's runner, starting
/// one detached where none answers, and waits for its fleet instance's first
/// tick → the host runner's answer, `host_runner.started` saying whether this
/// `run` started it, `host_runner.replaced` which host runner of an older
/// release it stopped first (`moved`), and `settings` what it changed of the
/// host settings. A refusal the host runner answered is `Answered`, printed
/// as it is.
pub fn run(
    checkout: &Repo,
    start: Start,
    settings: &host_settings::Named,
    now: u64,
) -> Result<Value> {
    let (log, lock) = (log_path()?, cache()?.join(LOCK));
    let pid = || holder_of(&lock).map(|pid| format!(" (pid {pid})")).unwrap_or_default();
    // Before anything changes: which afkconvoy the host runner is.
    let replaced = match runner_socket::ask(&Request::Version, SAID_WITHIN)? {
        Reached::Answer(said) => moved(&said["host_runner"])?,
        Reached::Nobody | Reached::Closed => None,
        Reached::Unanswered => bail!(
            "the host runner{} did not say its version within {}s: its log, {}, says why",
            pid(),
            SAID_WITHIN.as_secs(),
            log.display()
        ),
    };
    // A first start writes the host kit here, saying so to whoever ran `run`.
    kit::ensure(now)?;
    // Before the checkout is handed over: its fleet instance starts under
    // them, and every other on the host reads them as it goes.
    let changed = host_settings::change(&List::of_host()?, settings)?;
    let add = Request::Add { checkout: checkout.dir.clone(), start: Box::new(start) };
    let (reached, started) = match runner_socket::ask(&add, FIRST_TICK_WITHIN)? {
        Reached::Nobody => (started_and_asked(&add)?, true),
        reached => (reached, false),
    };
    let mut answer = match reached {
        Reached::Answer(answer) => answer,
        Reached::Nobody => bail!("no host runner answered: its log, {}, says why", log.display()),
        Reached::Closed => bail!(
            "the host runner{} ended before it answered: its log, {}, says why",
            pid(),
            log.display()
        ),
        Reached::Unanswered => bail!(
            "no first tick within {} minutes: the checkout may still be starting; its tick log, \
             {}, and the host runner's log, {}, say how far it got",
            FIRST_TICK_WITHIN.as_secs() / 60,
            log::Log::of(checkout)?.file.display(),
            log.display()
        ),
    };
    if let Some(about) = answer.get_mut("host_runner").and_then(Value::as_object_mut) {
        about.insert("started".into(), json!(started));
        about.insert("replaced".into(), json!(replaced));
    }
    answer["settings"] = Value::Object(changed);
    if answer.get("error").is_some() {
        return Err(Answered(answer).into());
    }
    let warnings = answer["warnings"].as_array().into_iter().flatten();
    for warning in warnings.filter_map(Value::as_str) {
        eprintln!("warning: {warning}");
    }
    Ok(answer)
}

/// `afkconvoy stop CHECKOUT`: has this host's runner take the checkout
/// `checkout` off the checkout list and stop its fleet instance → once it
/// has stopped, the host runner's answer. Where none answers, the list is
/// changed here, and said in the host runner's log: no fleet instance ran.
/// A refusal the host runner answered is `Answered`, printed as it is.
pub fn stop(checkout: &Repo) -> Result<Value> {
    let asked = Request::Stop { checkout: checkout.dir.clone() };
    match runner_socket::ask(&asked, STOPPED_WITHIN)? {
        Reached::Nobody => {}
        reached => return answered_stop(reached, "its fleet instance"),
    }
    let (list, root) = (List::of_host()?, &checkout.dir);
    let key = checkout_list::repo_of(checkout).ok();
    let mut answer = match taken_off(&list, root)? {
        true => {
            let line = format!(
                "checkout removed: {}: by `afkconvoy stop`, with no host runner running",
                root.display()
            );
            // A log that cannot be written costs its line, never the stop.
            let _ = opened_log(&log_path()?).and_then(|mut log| {
                writeln!(log, "{}", stamped(&line)).context("cannot write the host runner's log")
            });
            stopped(root, key.as_deref(), true, None)
        }
        false => {
            not_listed(&known(&list.read()?.checkouts, &BTreeMap::new()), root, key.as_deref())?
        }
    };
    answer["host_runner"] = Value::Null;
    Ok(answer)
}

/// `afkconvoy stop --all`: has this host's runner stop every fleet instance
/// and then itself, keeping the checkout list → once it has ended, its
/// answer. Where none answers, nothing changes.
pub fn stop_all() -> Result<Value> {
    let answer = match runner_socket::ask(&Request::StopAll, STOPPED_WITHIN)? {
        Reached::Nobody => {
            let kept = List::of_host()?.read()?.checkouts.len();
            return Ok(json!({"stopped": [], "checkouts": kept, "host_runner": null}));
        }
        reached => answered_stop(reached, "every fleet instance")?,
    };
    // It ends once it has answered: a `run` after this one starts a host
    // runner that need not wait for it.
    let lock = cache()?.join(LOCK);
    if !let_go(&lock, UP_WITHIN) {
        bail!(
            "the host runner (pid {}) stopped every fleet instance, and has not ended within {}s: \
             its log, {}, says how far it got",
            answer["host_runner"]["pid"],
            UP_WITHIN.as_secs(),
            log_path()?.display()
        );
    }
    Ok(answer)
}

/// What this host's runner said it holds (`holding`).
pub struct Holding {
    /// Its {pid, version, log}.
    pub about: Value,
    /// Each checkout it holds a fleet instance of, by its root, with that
    /// fleet instance's id, `None` until its start is logged.
    pub instances: BTreeMap<PathBuf, Option<String>>,
}

/// Asks this host's runner which checkouts it holds a fleet instance of →
/// what it said; `None` where no host runner runs. One that cannot say — of
/// an afkconvoy that cannot read the question, ended before it answered, or
/// silent past `SAID_WITHIN` — is an error saying so, and where to look.
pub fn holding() -> Result<Option<Holding>> {
    let (log, lock) = (log_path()?, cache()?.join(LOCK));
    let pid = || holder_of(&lock).map(|pid| format!(" (pid {pid})")).unwrap_or_default();
    let answer = match runner_socket::ask(&Request::Checkouts, SAID_WITHIN)? {
        Reached::Nobody => return Ok(None),
        Reached::Answer(answer) => answer,
        Reached::Closed => bail!(
            "the host runner{} ended before it answered: its log, {}, says why",
            pid(),
            log.display()
        ),
        Reached::Unanswered => bail!(
            "the host runner{} did not say which checkouts it holds within {}s: its log, {}, \
             says why",
            pid(),
            SAID_WITHIN.as_secs(),
            log.display()
        ),
    };
    let about = answer["host_runner"].clone();
    if let Some(error) = answer["error"].as_str() {
        let theirs = about["version"].as_str().unwrap_or_default();
        let fix = match found(VERSION, theirs) {
            Found::Older => format!(
                "; `afkconvoy run`, in any checkout, moves it onto this afkconvoy, {VERSION}"
            ),
            Found::Same | Found::Newer => String::new(),
        };
        bail!(
            "the host runner (pid {}), of afkconvoy {theirs}, cannot say which checkouts it \
             holds: {error}{fix}",
            about["pid"]
        );
    }
    let held = answer["held"].as_array().into_iter().flatten();
    let instances = held.filter_map(|held| {
        let root = PathBuf::from(held["checkout"].as_str()?);
        Some((root, held["instance"].as_str().map(str::to_string)))
    });
    Ok(Some(Holding { about, instances: instances.collect() }))
}

/// What the host runner answered `stop` or `stop --all`, `waited_for`
/// stopping: an error it answered is `Answered`; one that ended before it
/// answered, or answered nothing within `STOPPED_WITHIN`, is an error saying
/// so and where to look.
fn answered_stop(reached: Reached, waited_for: &str) -> Result<Value> {
    let log = log_path()?;
    let answer = match reached {
        Reached::Answer(answer) => answer,
        Reached::Nobody => bail!("no host runner answered: its log, {}, says why", log.display()),
        Reached::Closed => {
            let pid = holder_of(&cache()?.join(LOCK)).map(|pid| format!(" (pid {pid})"));
            bail!(
                "the host runner{} ended before it answered: its log, {}, says why",
                pid.unwrap_or_default(),
                log.display()
            )
        }
        Reached::Unanswered => bail!(
            "{waited_for} did not stop within {} minutes: it is still finishing the step it is \
             in, and the host runner's log, {}, says when it ends",
            STOPPED_WITHIN.as_secs() / 60,
            log.display()
        ),
    };
    if answer.get("error").is_some() {
        return Err(Answered(answer).into());
    }
    Ok(answer)
}

/// What `run` does with the host runner that said it is `about` — its pid
/// and version — before it hands it anything (docs/adr/0023) → the host
/// runner it replaced, if it did. One of this afkconvoy's release is left
/// as it is → `None`. One of an older release is stopped as SIGTERM stops
/// it — each fleet instance finishes the step it is in, and the checkout list
/// is kept — and waited for until it lets the host go → its {pid, version}.
/// One of a newer release refuses this `run`, which changes nothing, and so
/// does a fleet instance held in the foreground, whatever its release.
fn moved(about: &Value) -> Result<Option<Value>> {
    let (theirs, pid) = (about["version"].as_str().unwrap_or_default(), &about["pid"]);
    if about["foreground"] == true {
        bail!(
            "this host's runner is a fleet instance held in the foreground (`run --foreground`, \
             pid {pid}), which takes no other checkout, so this run changed nothing: run again \
             once it has ended"
        );
    }
    match found(VERSION, theirs) {
        Found::Same => Ok(None),
        Found::Newer => bail!(
            "this afkconvoy, {VERSION}, is older than the host runner's, {theirs} (pid {pid}), so \
             it changed nothing: run the newer afkconvoy — `which -a afkconvoy` lists each on your \
             PATH"
        ),
        Found::Older => {
            let Some(pid) = pid.as_u64() else {
                bail!("the host runner of afkconvoy {theirs} gave no pid, and cannot be stopped")
            };
            let told = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
            told.context("`kill` could not be run to stop the host runner")?;
            // However `kill` ended: one already gone has let the host go.
            if !let_go(&cache()?.join(LOCK), UP_WITHIN) {
                bail!(
                    "the host runner of afkconvoy {theirs} (pid {pid}) was told to stop, to make \
                     way for this afkconvoy, {VERSION}, and still holds the host after {}s: its \
                     log, {}, says how far it is. Run again once it has stopped",
                    UP_WITHIN.as_secs(),
                    log_path()?.display()
                );
            }
            Ok(Some(json!({"pid": pid, "version": theirs})))
        }
    }
}

/// Whether a host runner holds this host now.
pub fn runs() -> bool {
    cache().is_ok_and(|dir| held(&dir.join(LOCK)))
}

/// Whether a process holds the lock in `file`: one nobody holds is taken
/// and let go at once.
fn held(file: &Path) -> bool {
    let Ok(lock) = File::open(file) else { return false };
    matches!(lock.try_lock(), Err(TryLockError::WouldBlock))
}

/// Whether a host runner holds the host whose cache directory is `dir`.
pub(crate) fn held_in(dir: &Path) -> bool {
    held(&dir.join(LOCK))
}

/// Whether this host's runner holds a fleet instance of the repo the
/// checkout `repo` is of: the one sign on this host that one is alive. Soft:
/// a host runner that cannot be asked, or does not answer, holds none.
pub fn holds(repo: &Repo) -> bool {
    let asked = Request::Holds { checkout: repo.dir.clone() };
    match runner_socket::ask(&asked, HOLDS_WITHIN) {
        Ok(Reached::Answer(answer)) => answer["holds"] == true,
        _ => false,
    }
}

/// `run --foreground`: holds the fleet instance of the checkout `checkout`
/// in this process as `start` asks, as the host runner of this host — its
/// lock and its socket, on which every command reaches it as it reaches any
/// host runner — printing every line as `run` printed it, until SIGTERM or
/// SIGINT, or until it has made `start.knobs.ticks` → what its loop
/// answers. It holds no other checkout and leaves the checkout list alone:
/// a `run` or a `stop` while it holds the host is refused, and so is it,
/// where another host runner holds the host. What it names of the host
/// settings, `settings`, holds in this process alone, laid over the host's.
pub fn foreground(checkout: &Repo, start: &Start, settings: host_settings::Named) -> Result<Value> {
    host_settings::lay_for_this_process(settings);
    let dir = cache()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let Some((lock, _)) = took_host(&dir.join(LOCK))? else {
        let holder = holder_of(&dir.join(LOCK));
        let said = holder.map_or("another".to_string(), |pid| format!("pid {pid}"));
        bail!(
            "a host runner ({said}) holds this host, and a fleet instance held in the foreground \
             is held only where none does: `afkconvoy stop --all` stops it"
        );
    };
    let stop = Stop::listen();
    let (pid, root, reach) = (std::process::id(), checkout.dir.clone(), Reach::default());
    let key = checkout_list::repo_of(checkout).ok();
    // Its fleet instance's id, once its start is logged.
    let instance: Arc<Mutex<Option<String>>> = Arc::default();
    let (held, named, shared) = (reach.clone(), Arc::clone(&instance), Shared::default());
    let about = json!({"pid": pid, "version": VERSION, "foreground": true});
    let answerer = shared.clone();
    let answering = runner_socket::serve(runner_socket::listen()?, about, move |request| {
        let answered = match request {
            Request::Version => Ok(json!({})),
            Request::Exclusive { ticket } => return answerer.queue.answer(ticket),
            Request::Shared => Ok(answerer.said()),
            Request::Checkouts => {
                let instance = named.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                Ok(json!({"held": [{"checkout": root, "instance": *instance}]}))
            }
            Request::Add { .. } | Request::Stop { .. } | Request::StopAll => Err(anyhow!(
                "this host's runner is a fleet instance held in the foreground (`run \
                 --foreground`, pid {pid}), of {}: it takes no other checkout, and stops on \
                 SIGTERM or Ctrl-C alone",
                root.display()
            )),
            about => {
                let mine = |repo: &str| key.as_deref() == Some(repo);
                about_instance(&about, |repo| match mine(repo) {
                    true => HeldAs::Running(held.clone()),
                    false => HeldAs::Nothing,
                })
            }
        };
        Reply::Line(answered.unwrap_or_else(|error| crate::error_json(&error)))
    });
    let echo: Echo = Arc::new(move |line| match line {
        Line::Kept(entry) => {
            let mut named = instance.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            named.get_or_insert_with(|| entry.instance.clone());
            println!("{}", entry.said());
        }
        Line::Said(text) => println!("{text}"),
        Line::Warning(warning) => probe::announce(&[warning.to_string()]),
    });
    let Reach { wakes, opener } = reach;
    let holder = Holder { stop: stop.instance(), echo, wakes, opener, shared };
    let held = instance::hold(checkout, start, holder);
    // It holds the host no more: a `run` from now on starts a host runner,
    // which waits for the lock.
    let _ = std::fs::remove_file(dir.join(runner_socket::SOCKET));
    answering.settled(ANSWERED_WITHIN);
    drop(lock);
    held
}

/// Waits, `within` at most, for whoever holds the lock in `file` to let it
/// go → whether it did.
fn let_go(file: &Path, within: Duration) -> bool {
    let until = Instant::now() + within;
    while held(file) {
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Starts a host runner and asks it `add` once it answers, until
/// `UP_WITHIN` and a little more have passed. One that ends as it starts is
/// an error with the last lines of its log; one that ends saying another
/// holds the host leaves that one to answer.
fn started_and_asked(add: &Request) -> Result<Reached> {
    let mut child = start_detached()?;
    let log = log_path()?;
    let given = UP_WITHIN + Duration::from_secs(5);
    let until = Instant::now() + given;
    loop {
        match runner_socket::ask(add, FIRST_TICK_WITHIN)? {
            Reached::Nobody => {}
            reached => return Ok(reached),
        }
        if let Some(ended) = child.try_wait()?
            && !ended.success()
        {
            bail!(
                "the host runner ended as it started ({ended}): its log, {}, says why:\n{}",
                log.display(),
                last_lines(&log, 20)
            );
        }
        if Instant::now() >= until {
            let holder = holder_of(&cache()?.join(LOCK)).map(|pid| {
                format!(
                    ", though pid {pid} holds this host (it may be finishing a step as it stops)"
                )
            });
            bail!(
                "no host runner answered within {}s{}: its log, {}, says why",
                given.as_secs(),
                holder.unwrap_or_default(),
                log.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Starts the hidden `afkconvoy host-runner`, detached from this process: in
/// a process group of its own, so a terminal's Ctrl-C and hangup and a
/// harness that ends the caller's group all miss it; from /; with nothing on
/// stdin and its stdout and stderr appended to its log; and with the
/// environment of a fresh login shell, but for the cache and config
/// directories of this `run` (`pinned`). It is never waited on: once `run`
/// exits, it is nobody's child.
fn start_detached() -> Result<Child> {
    let dir = cache()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let env = launch::login_environment()
        .context("the host runner starts with the environment of a fresh login shell")?;
    let kit = kit::path()?;
    let config = kit.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
    let env = pinned(env, &dir, config);
    let log = opened_log(&dir.join(LOG))?;
    let program = std::env::current_exe().context("cannot find this afkconvoy")?;
    Command::new(program)
        .arg("host-runner")
        .env_clear()
        .envs(env)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .context("the host runner could not be started")
}

/// `env` with XDG_CACHE_HOME and XDG_CONFIG_HOME set to those of this
/// `run` — the cache directory `cache` and the config directory `config` —
/// whatever it says: the host runner reads the list, binds the socket and
/// reads the host kit exactly where the `run` that started it does.
fn pinned(
    env: Vec<(OsString, OsString)>,
    cache: &Path,
    config: &Path,
) -> Vec<(OsString, OsString)> {
    let pins = [
        ("XDG_CACHE_HOME", cache.parent().unwrap_or(cache).as_os_str()),
        ("XDG_CONFIG_HOME", config.as_os_str()),
    ];
    let unpinned = env.into_iter().filter(|(name, _)| !pins.iter().any(|(pin, _)| name == pin));
    unpinned.chain(pins.map(|(name, value)| (name.into(), value.to_os_string()))).collect()
}

/// The host runner's log, opened to append to; one past `LOG_ROLLS_AT` is
/// moved aside to `.1` first.
fn opened_log(file: &Path) -> Result<File> {
    if std::fs::metadata(file).is_ok_and(|kept| kept.len() > LOG_ROLLS_AT) {
        let _ = std::fs::rename(file, file.with_extension("log.1"));
    }
    let opened = OpenOptions::new().append(true).create(true).open(file);
    opened.with_context(|| format!("cannot open the host runner's log {}", file.display()))
}

/// The last `count` lines of `file`, or nothing where it cannot be read.
fn last_lines(file: &Path, count: usize) -> String {
    let text = std::fs::read(file).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let text = text.unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_runner_keeps_the_cache_and_config_directories_of_the_run_that_started_it() {
        let env = vec![
            ("PATH".into(), "/usr/bin".into()),
            ("XDG_CACHE_HOME".into(), "/elsewhere".into()),
            ("HOME".into(), "/Users/me".into()),
        ];
        let pinned = pinned(env, Path::new("/c/afkconvoy"), Path::new("/conf"));
        let pinned: Vec<(&str, &str)> = pinned
            .iter()
            .map(|(name, value)| (name.to_str().unwrap(), value.to_str().unwrap()))
            .collect();
        assert_eq!(
            pinned,
            [
                ("PATH", "/usr/bin"),
                ("HOME", "/Users/me"),
                ("XDG_CACHE_HOME", "/c"),
                ("XDG_CONFIG_HOME", "/conf")
            ]
        );
    }

    #[test]
    fn a_lock_is_taken_by_one_and_says_who_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(LOCK);
        assert!(!held(&file), "no lock file");
        let (lock, before) = took_host(&file).unwrap().expect("nobody held it");
        assert_eq!(before, Value::Null, "nobody held it before");
        assert_eq!(holder_of(&file), Some(u64::from(std::process::id())));
        assert!(held(&file), "held while kept");
        let other = OpenOptions::new().write(true).open(&file).unwrap();
        assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)), "held while kept");
        drop(lock);
        assert!(!held(&file), "let go when dropped");
        assert!(other.try_lock().is_ok(), "and asking took nothing");
        drop(other);

        // the next to take it is told who held it before
        let (_lock, before) = took_host(&file).unwrap().expect("nobody holds it");
        assert_eq!(before["pid"], std::process::id());
        assert_eq!(before["version"], VERSION);
    }

    #[test]
    fn the_end_of_a_log_is_its_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(LOG);
        assert_eq!(last_lines(&file, 2), "");
        std::fs::write(&file, "a\nb\nc\n").unwrap();
        assert_eq!(last_lines(&file, 2), "b\nc");
        assert_eq!(last_lines(&file, 5), "a\nb\nc");
    }
}

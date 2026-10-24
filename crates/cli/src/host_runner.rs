//! The host runner: the one process of the fleet on a host, holding a fleet
//! instance for each checkout on the host's checkout list (docs/adr/0023);
//! and `run`, which hands it a checkout, starting it where none runs, and
//! moving it onto its own afkconvoy where it is of an older release.
//!
//! The host runner is whoever holds the lock `host-runner.lock` in the cache
//! directory, so there is one for each cache directory, by construction. It
//! never ticks: each fleet instance it holds ticks on a thread of its own, on
//! its own time, and keeps its own tick log as before. What it keeps itself
//! is its log, `host-runner.log`: one line for each thing that happened to
//! it — its start and its stop, the older host runner it replaced, a
//! checkout added, refused or removed, a fleet instance started, broken, back
//! or ended, a panic — and the lines a fleet instance says that no tick log
//! holds. A fleet instance that breaks harms no other: its thread holds it
//! again after a backoff, and it stays on the list.

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
use crate::git::Repo;
use crate::instance::{self, Named, Start};
use crate::log::{self, What};
use crate::runner_socket::{self, Reached, Request};
use crate::stop::Stop;
use crate::tick::{Echo, Holder, Line};
use crate::{VERSION, kit, launch};

/// The lock whose holder is the host runner, in the cache directory. It
/// holds the holder's {pid, version, started}.
const LOCK: &str = "host-runner.lock";

/// The host runner's log, in the cache directory: its stdout and stderr.
const LOG: &str = "host-runner.log";

/// How long a host runner starting waits for the one before it to let the
/// host go: one that is stopping finishes the step each fleet instance is in.
const UP_WITHIN: Duration = Duration::from_secs(30);

/// How long `run` waits for its checkout's first tick: a first start may
/// write the host kit and make the startup probe before it.
const FIRST_TICK_WITHIN: Duration = Duration::from_secs(30 * 60);

/// How long `run` waits for the host runner to say its version:
/// it answers at once, whatever its fleet instances are doing.
const SAID_WITHIN: Duration = Duration::from_secs(10);

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
}

/// One checkout's fleet instance, from its start until its thread ends.
struct Held {
    /// The repo it is of (`checkout_list::repo_of`).
    repo: String,
    seen: Arc<Seen>,
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
    /// Its id, once this start is kept in its tick log.
    instance: Option<String>,
    /// What it warned of before this start's first tick ended: its startup
    /// probe's warnings among them.
    warnings: Vec<String>,
    came: Came,
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
}

/// The host runner's own log: its stdout, which the `run` that started it
/// points at host-runner.log. One line for each thing that happened to it,
/// stamped in UTC, one at a time whichever thread says it; what spans
/// several lines, as an error may, is folded into one.
#[derive(Default)]
struct RunnerLog {
    one: Mutex<()>,
}

impl RunnerLog {
    fn say(&self, line: &str) {
        let parts: Vec<&str> =
            line.lines().map(str::trim).filter(|part| !part.is_empty()).collect();
        let line = parts.join(" ");
        let _one = self.one.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // A log that cannot be written costs its lines, never the host runner.
        let _ = writeln!(std::io::stdout(), "{} {line}", log::utc(crate::clock()));
    }
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
/// SIGTERM or SIGINT, or until its lock file is gone. Then it lets every fleet
/// instance finish the step it is in and keep its `stop` line, and ends,
/// leaving the list as it is → {"stopped", "pid", "checkouts"}; or, where
/// another host runner held the host all the while, {"started": false}.
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
    let runner = Arc::new(Runner { list, log, stop: stop.clone(), held: Mutex::default() });
    runner.start_listed(listed.checkouts);
    let about = json!({"pid": pid, "version": VERSION, "log": dir.join(LOG)});
    let answering = Arc::clone(&runner);
    runner_socket::serve(listener, about, move |request| answering.answer(request));

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
    fn answer(self: &Arc<Self>, request: Request) -> Value {
        let answered = match request {
            Request::Version => Ok(json!({})),
            Request::Add { checkout, start } => self.add(&checkout, *start),
        };
        answered.unwrap_or_else(|error| crate::error_json(&error))
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
            let repo_of =
                |root: &Path| Repo::at(root).and_then(|repo| checkout_list::repo_of(&repo));
            let listing = listed.iter();
            let listing =
                listing.map(|entry| (entry.checkout.clone(), repo_of(&entry.checkout).ok()));
            let holding = held.iter().map(|(root, held)| (root.clone(), Some(held.repo.clone())));
            let known: Vec<(String, Option<String>)> = listing
                .chain(holding)
                .map(|(root, repo)| (root.display().to_string(), repo))
                .collect();
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
        held.insert(root.clone(), Held { repo: key.clone(), seen: Arc::clone(&seen) });
        let (runner, holding) = (Arc::clone(self), Arc::clone(&seen));
        std::thread::spawn(move || runner.hold(&root, &key, start, &holding));
        seen
    }

    /// Holds the fleet instance of the checkout `root`, of the repo `key`, on
    /// this thread, as `start` asks, until it ends (`ended`). One that breaks
    /// is held again after a backoff that grows at each break in a row
    /// (`backoff`), or as soon as a `run` asks for it; every other fleet
    /// instance goes on meanwhile.
    fn hold(self: Arc<Self>, root: &Path, key: &str, mut start: Start, seen: &Arc<Seen>) {
        // What it waited before its last start, where it broke before it.
        let mut waited = None;
        // The last fleet instance it started as: the one that broke.
        let mut broke_as = String::new();
        loop {
            let begun = Instant::now();
            let stop = self.stop.instance();
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
                let holder = Holder { stop: stop.clone(), echo };
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
            match self.rested(seen, until) {
                Rest::Due => {}
                Rest::Asked(again) => start = *again,
                Rest::Stopped => {
                    let mut holding = self.holding();
                    holding.remove(root);
                    seen.stage().came = Came::Ended("it was broken".to_string());
                    drop(holding);
                    seen.changed.notify_all();
                    let at = root.display();
                    self.log.say(&format!(
                        "fleet instance ended: {at}: broken since the host runner stops"
                    ));
                    return;
                }
            }
        }
    }

    /// Rests the thread of a broken fleet instance until `until` → `Due`;
    /// until a `run` asks for it → `Asked`, with what that asks; or until the
    /// host runner is told to stop → `Stopped`.
    fn rested(&self, seen: &Seen, until: Instant) -> Rest {
        let mut stage = seen.stage();
        loop {
            if self.stop.told() {
                return Rest::Stopped;
            }
            if let Some(again) = stage.again.take() {
                return Rest::Asked(Box::new(again));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // A `run` that asks from now on waits for this start.
                (stage.came, stage.asked) = (Came::Starting, false);
                return Rest::Due;
            }
            let rested = seen.changed.wait_timeout(stage, left.min(WATCHED_EVERY));
            stage = rested.map_or_else(|poisoned| poisoned.into_inner().0, |(stage, _)| stage);
        }
    }

    /// The loop of the fleet instance of `root` ended, as `held` says → why
    /// it broke, where it did: it is still held, now `Came::Broken`, and its
    /// thread holds it again. It broke where its loop ended in an error or a
    /// panic while the host runner goes on — after its first tick, or before
    /// it where nobody asked for it. Otherwise it ended, and is held no more:
    /// one that ended before its first tick was refused, and a `run` that
    /// asked for it is told why and its checkout taken off the list.
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
            let broke = at_work && held.is_err() && !self.stop.told();
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
        let at = root.display();
        match stage.came {
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
}

/// How the thread of a broken fleet instance ended its rest (`rested`).
enum Rest {
    /// Its backoff is over: it is started again as before.
    Due,
    /// A `run` asked for it: it is started again now, as that `run` asks.
    Asked(Box<Start>),
    /// The host runner is stopping: it is held no more.
    Stopped,
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
    /// start, said in the log as a fleet instance started; its first tick,
    /// which puts its checkout on the list and answers whoever waits.
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
        if !matches!(entry.what, What::Tick(_)) || !matches!(stage.came, Came::Starting) {
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

/// `afkconvoy run`: hands the checkout `checkout` to this host's runner,
/// starting one detached where none answers, and waits for its fleet
/// instance's first tick → the host runner's answer, `host_runner.started`
/// saying whether this `run` started it, and `host_runner.replaced` which
/// host runner of an older release it stopped first (`moved`). A refusal the
/// host runner answered is `Answered`, printed as it is.
pub fn run(checkout: &Repo, start: Start, now: u64) -> Result<Value> {
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
    if answer.get("error").is_some() {
        return Err(Answered(answer).into());
    }
    let warnings = answer["warnings"].as_array().into_iter().flatten();
    for warning in warnings.filter_map(Value::as_str) {
        eprintln!("warning: {warning}");
    }
    Ok(answer)
}

/// What `run` does with the host runner that said it is `about` — its pid
/// and version — before it hands it anything (docs/adr/0023) → the host
/// runner it replaced, if it did. One of this afkconvoy's release is left
/// as it is → `None`. One of an older release is stopped as SIGTERM stops
/// it — each fleet instance finishes the step it is in, and the checkout list
/// is kept — and waited for until it lets the host go → its {pid, version}.
/// One of a newer release refuses this `run`, which changes nothing.
fn moved(about: &Value) -> Result<Option<Value>> {
    let (theirs, pid) = (about["version"].as_str().unwrap_or_default(), &about["pid"]);
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
            let (lock, until) = (cache()?.join(LOCK), Instant::now() + UP_WITHIN);
            while held(&lock) {
                if Instant::now() >= until {
                    bail!(
                        "the host runner of afkconvoy {theirs} (pid {pid}) was told to stop, to \
                         make way for this afkconvoy, {VERSION}, and still holds the host after \
                         {}s: its log, {}, says how far it is. Run again once it has stopped",
                        UP_WITHIN.as_secs(),
                        log_path()?.display()
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
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

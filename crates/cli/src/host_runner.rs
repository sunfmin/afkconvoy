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
//! of that checkout's repo — a wake, whether one runs — with no socket of
//! the instance's own. It never ticks: each fleet instance it holds ticks on
//! a thread of its own, on its own time, and keeps its own tick log as
//! before. What it keeps itself is its log, `host-runner.log`: one line for
//! each thing that happened to it — its start and its stop, the older host
//! runner it replaced, a checkout added, refused or removed, a fleet instance
//! started, broken, back or ended, a panic, where it serves the panel — and
//! the lines a fleet instance says that no tick log holds. A fleet instance
//! that breaks harms no other: its thread holds it again after a backoff, and
//! it stays on the list. It serves the panel from its start (`host_panel`),
//! on the panel's port of the host settings, or on any free port where that
//! is taken.
//!
//! It moves onto a newer afkconvoy by itself (`Watched`, `handed_over`): it
//! looks at the file it was started from, and once that answers a newer
//! release and nothing holds the move back (`held_back`), it stops every
//! fleet instance between two ticks, lets the host go and starts the new
//! one's in its place, which starts every checkout on the list. One that
//! does not answer is ended, and this one takes the host back and holds
//! every checkout again, on its own release.
//!
//! `run --foreground` holds the host the same way — its lock, its socket and
//! its panel — with one fleet instance, its own checkout's, in the process
//! that ran it, and leaves the checkout list alone (`foreground`).

use std::any::Any;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::num::NonZeroUsize;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use afkconvoy_core::checkouts::{OnList, on_list};
use afkconvoy_core::exclusive::Ticket;
use afkconvoy_core::host_runner::{Found, HeldBack, Stands, backoff, found, held_back};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::checkout_list::{self, Entry, List};
use crate::copies::cache;
use crate::doctor::{Finding, Kind, finding};
use crate::exclusive::Queue;
use crate::git::Repo;
use crate::host_panel::HostPanel;
use crate::host_settings::To;
use crate::instance::{self, Named, Start};
use crate::log::{self, What};
use crate::panel::Broken;
use crate::published::Published;
use crate::runner_socket::{self, Reached, Reply, Request};
use crate::standings::Standings;
use crate::stop::Stop;
use crate::tick::{Echo, Holder, Line, Show};
use crate::tracker::Checked;
use crate::wake::{self, Wakes};
use crate::{VERSION, concurrency, host_settings, kit, launch, probe, tracker};

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

/// How often the host runner looks at the file it was started from, for a
/// newer afkconvoy installed there.
pub(crate) const INSTALLED_LOOKED_EVERY: Duration = Duration::from_secs(1);

/// How long the afkconvoy installed there has to say its version.
const VERSION_SAID_WITHIN: Duration = Duration::from_secs(10);

/// How long a host runner moving waits for the one it started to answer: as
/// long as `run` waits for one, the new one's wait for the host among it.
pub(crate) const MOVED_WITHIN: Duration = Duration::from_secs(UP_WITHIN.as_secs() + 5);

/// The maximum wait for ticks and a running exclusive command to end.
pub(crate) const MOVE_WAIT_WITHIN: Duration = Duration::from_secs(10 * 60);

/// The host runner as it holds its checkouts.
struct Runner {
    list: List,
    log: Arc<RunnerLog>,
    /// The process's: SIGTERM, SIGINT, the lock file gone.
    stop: Stop,
    /// Whether it is moving onto a newer afkconvoy: every fleet instance was
    /// told to stop, and none starts again.
    moving: AtomicBool,
    /// Every fleet instance this host runner holds, by checkout root.
    held: Mutex<BTreeMap<PathBuf, Held>>,
    /// The page it serves.
    panel: HostPanel,
    shared: Shared,
}

/// What the fleet instances of a host share, held by its host runner for
/// them all (docs/adr/0023): how each stands, which a move reads, the one
/// watch on the host and who waits for it, and the exclusive queue; and
/// the newer afkconvoy installed where it was started from, which it moves
/// onto; and the wake routes of open blockers across repos on one tracker.
/// A clone is the same.
#[derive(Clone, Default)]
pub struct Shared {
    pub standings: Standings,
    pub resources: crate::resources::Shared,
    pub queue: Queue,
    pub installed: Arc<Mutex<Option<Installed>>>,
    pub published: Arc<Mutex<Option<Published>>>,
    pub blockers: wake::Blockers,
    pub watch: crate::host_watch::Shared,
}

impl Shared {
    /// What it holds now: what `shared` is answered, and the panel the host
    /// runner serves shows.
    pub fn now(&self) -> SharedNow {
        self.checked_now().unwrap_or_default()
    }

    fn checked_now(&self) -> Result<SharedNow> {
        let (exclusive, exclusive_paused) = self.queue.observation()?;
        Ok(SharedNow {
            exclusive,
            exclusive_paused: Some(exclusive_paused),
            installed: self.installed().clone(),
            published: self.published.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            watch: self.watch.running(),
        })
    }

    fn installed(&self) -> MutexGuard<'_, Option<Installed>> {
        self.installed.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What `shared` is answered.
    fn said(&self) -> Value {
        match self.checked_now() {
            Ok(now) => serde_json::to_value(now).unwrap_or_default(),
            Err(error) => crate::error_json(&error),
        }
    }
}

/// What the host's fleets share, as their host runner says it on its socket.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SharedNow {
    /// The exclusive queue, in scheduler order.
    pub exclusive: Vec<Ticket>,
    /// Whether a release move has paused new grants; `None`: an older host
    /// runner did not say, and the observation keeps it unknown.
    #[serde(default)]
    pub exclusive_paused: Option<bool>,
    /// The afkconvoy installed where the host runner was started from, where
    /// it is newer than the host runner's or does not start; `None`: none is,
    /// or the host runner is of an afkconvoy that does not say it.
    #[serde(default)]
    pub installed: Option<Installed>,
    /// A release of afkconvoy newer than the host runner's, published to the
    /// tap it was installed from; `None`: none is, it is installed from no
    /// tap, or the host runner is of an afkconvoy that does not say it.
    #[serde(default)]
    pub published: Option<Published>,
    /// The watch running on the host; `None`: none.
    #[serde(default)]
    pub watch: Option<crate::host_watch::Watch>,
}

/// The afkconvoy installed where the host runner was started from, once that
/// file changed and is newer than the host runner's or does not start, and
/// how the host runner's move onto it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The file: the path the host runner was started as.
    pub path: PathBuf,
    /// The release it says it is, `--version`; `None` where it said none.
    pub version: Option<String>,
    /// The host runner's own.
    pub from: String,
    /// When this installed file answered, in epoch seconds.
    #[serde(default)]
    pub since: u64,
    /// The safe moment did not arrive within `MOVE_WAIT_WITHIN`: the old
    /// host runner works on between bounded attempts, and keeps trying.
    #[serde(default)]
    pub wait_expired: bool,
    /// The current bounded attempt's start, or the next one's earliest
    /// start after an expired wait. Process-local; the finding keeps `since`.
    #[serde(skip)]
    attempt_since: u64,
    /// What holds the move back now, each in words; empty as it moves, and
    /// where it failed.
    pub held_back: Vec<String>,
    /// Why it did not start, as the host runner moved onto it — or did not
    /// say its version: the host runner works on, on its own, and tries that
    /// file no more until it changes.
    pub failed: Option<String>,
}

/// What the host's fleets share, as its host runner says it (`Shared`);
/// `None`: no host runner listens. Errors: the one there did not answer
/// within `SAID_WITHIN`, or answered an error — an older one, that holds none
/// of it, among them.
pub fn shared() -> Result<Option<SharedNow>> {
    match runner_socket::ask(&Request::Shared, SAID_WITHIN)? {
        Reached::Answer(answer) => match answer.get("error") {
            Some(error) => bail!("the host runner says: {}", error.as_str().unwrap_or_default()),
            None => serde_json::from_value(answer)
                .map(Some)
                .context("what the host runner says its fleets share cannot be read"),
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

/// What `doctor` finds of the host runner's move onto a newer afkconvoy, as
/// the host runner says it (`shared`): the one installed where it was started
/// from did not start, or does not say its version. None where no host
/// runner runs, or it cannot say.
pub fn findings() -> Vec<Finding> {
    let Some(shared) = shared().ok().flatten() else { return vec![] };
    let moved = shared.installed.as_ref().and_then(not_moved);
    moved.into_iter().chain(shared.published.as_ref().map(not_upgraded)).collect()
}

/// The finding of `published`, a release newer than the host runner's that
/// it has not moved onto yet.
fn not_upgraded(published: &Published) -> Finding {
    let Published { formula, version, installed, from, since, failed } = published;
    let what = format!(
        "afkconvoy {version} is published, newer than the host runner's, afkconvoy {from}; \
         Homebrew has afkconvoy {installed} installed from {formula}"
    );
    let due = since + 2 * crate::published::LOOKED_EVERY.as_secs();
    let evidence = match failed {
        Some(why) => {
            format!("first seen at {since}; the last `brew upgrade {formula}` failed: {why}")
        }
        None => format!(
            "first seen at {since}; the host runner installs it by {due}, and moves onto it \
             between ticks"
        ),
    };
    finding(
        Kind::NotUpgraded,
        json!(["not-upgraded", formula, version]),
        what,
        evidence,
        format!("brew update && brew upgrade {formula}"),
    )
}

/// The finding of `installed`, where the host runner did not move onto it.
fn not_moved(installed: &Installed) -> Option<Finding> {
    if installed.wait_expired {
        let what = format!(
            "the host runner did not move onto afkconvoy {}, installed at {}: the wait for a \
             moment between ticks with no exclusive command running exceeded {}s",
            installed.version.as_deref().unwrap_or_default(),
            installed.path.display(),
            MOVE_WAIT_WITHIN.as_secs()
        );
        return Some(finding(
            Kind::NotMoved,
            json!(["not-moved", installed.path]),
            what,
            installed.held_back.join("; "),
            format!("{} --version", installed.path.display()),
        ));
    }
    let why = installed.failed.clone()?;
    let at = installed.path.display();
    let what = match &installed.version {
        Some(version) => format!(
            "the host runner did not move onto afkconvoy {version}, installed at {at}: it did not \
             start, and the host runner works on, of afkconvoy {}",
            installed.from
        ),
        None => format!(
            "the host runner did not move onto the afkconvoy installed at {at}: it does not say \
             its version, and the host runner works on, of afkconvoy {}",
            installed.from
        ),
    };
    Some(finding(
        Kind::NotMoved,
        json!(["not-moved", installed.path]),
        what,
        why,
        format!("{at} --version"),
    ))
}

/// One checkout's fleet instance, from its start until its thread ends.
struct Held {
    /// The repo it is of (`checkout_list::repo_of`).
    repo: String,
    /// The host its `run` named, else this machine's: its tick log's lines
    /// are of it.
    host: String,
    seen: Arc<Seen>,
    /// The stop of its start now, which `stop` tells: the thread of a broken
    /// one puts here that of each start of it again (`rested`).
    stop: Stop,
    reach: Reach,
}

/// What reaches one fleet instance through the host runner's socket: the
/// wakes heard for it. A clone is the same.
#[derive(Clone, Default)]
struct Reach {
    wakes: Wakes,
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
/// checkout is of — a wake, whether one runs — for
/// the one `held` finds of that repo: any worktree of it names it as well as
/// its checkout. One of a repo no fleet instance of runs is refused, saying
/// how to have one run.
fn about_instance(request: &Request, held: impl FnOnce(&str) -> HeldAs) -> Result<Value> {
    let checkout = match request {
        Request::Wake { checkout, .. } | Request::Holds { checkout } => checkout,
        Request::Version
        | Request::Add { .. }
        | Request::Stop { .. }
        | Request::StopAll
        | Request::Checkouts
        | Request::Exclusive { .. }
        | Request::Candidate { .. }
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
    /// `run` asks for it. Why, as a command prints its error (`error_json`).
    Broken(Value),
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

/// `line` as the host runner's log keeps it: stamped in UTC, the stamp naming
/// its zone (`2026-10-30T13:56:16Z`), and what spans
/// several lines, as an error may, folded into one.
fn stamped(line: &str) -> String {
    let parts: Vec<&str> = line.lines().map(str::trim).filter(|part| !part.is_empty()).collect();
    format!("{} {}", log::stamp(crate::clock()), parts.join(" "))
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

/// A line of the host's watch, as the host runner's log says it.
fn watch_said(what: &log::What) -> String {
    let entry = log::Entry {
        at: crate::clock(),
        instance: crate::host_watch::INSTANCE.into(),
        host: crate::this_host(),
        what: what.clone(),
    };
    entry.said()
}

/// The pid of this host's runner, as its lock says the last one to hold it;
/// `None` where none ever did, or the cache directory is not known.
pub fn pid() -> Option<u32> {
    let pid = holder_of(&cache().ok()?.join(LOCK))?;
    u32::try_from(pid).ok()
}

/// The release of this host's runner, as its lock says the last one to hold
/// it; `None` where none ever did, or it said none.
pub fn version() -> Option<String> {
    let text = std::fs::read_to_string(cache().ok()?.join(LOCK)).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["version"].as_str().map(str::to_string)
}

/// The host runner's log: host-runner.log in the cache directory.
pub fn log_path() -> Result<PathBuf> {
    Ok(cache()?.join(LOG))
}

/// A command or outbox transition that must be visible even outside the
/// runner's threads, using the same log and timestamp as the runner.
pub(crate) fn note(line: &str) -> Result<()> {
    writeln!(opened_log(&log_path()?)?, "{}", stamped(line))?;
    Ok(())
}

/// The hidden `afkconvoy host-runner`, as `run` starts it: takes the host,
/// serves the panel, starts every checkout on its list, and answers on its
/// socket, until
/// SIGTERM or SIGINT, `stop --all`, or until its lock file is gone. Then it
/// lets every fleet instance finish the step it is in and keep its `stop`
/// line, answers whoever asked it, lets the panel's port go, and ends,
/// leaving the list as it is →
/// {"stopped", "pid", "checkouts"}; or, where another host runner held the
/// host all the while, {"started": false}.
///
/// Meanwhile it looks at the file it was started from (`Watched`): once that
/// says a newer release and nothing holds the move back, every fleet
/// instance stops between two ticks, and the host runner of the newer
/// afkconvoy is started in this one's place, on the same list (`handed_over`)
/// → {"moved": true, "pid", "to"}, `to` being what that one says of itself;
/// the move and its two releases are a line of each checkout's tick log.
/// One that does not answer leaves this one holding the host again, as it
/// held it before, and trying that file no more until it changes.
pub fn serve() -> Result<Value> {
    // Test worlds put their test process in the login environment. A detached
    // runner must not survive a timeout or SIGKILL that bypasses World::drop.
    // Real login environments do not set this fixture-only lifeline.
    if let Some(owner) = std::env::var("AFKCONVOY_TEST_OWNER")
        .ok()
        .and_then(|value| value.parse::<libc::pid_t>().ok())
        .filter(|pid| *pid > 1)
    {
        std::thread::spawn(move || {
            loop {
                // SAFETY: signal zero only observes whether the test still exists.
                if unsafe { libc::kill(owner, 0) } != 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    std::process::exit(0);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }
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
    let mut watched = Watched::started();
    {
        let log = Arc::clone(&log);
        let published = Arc::clone(&watched.published);
        crate::published::learn(watched.path.clone(), published, move |line| log.say(line));
    }
    // Heard from the first hold of the host on, and across every hold after.
    let mut stop = None;
    loop {
        let Some((lock, before)) = took_host(&dir.join(LOCK))? else {
            let holder = holder_of(&dir.join(LOCK));
            let said = holder.map_or("another".to_string(), |pid| format!("pid {pid}"));
            log.say(&format!("host runner not started: {said} holds this host"));
            return Ok(json!({"started": false, "holder": holder}));
        };
        let stop = stop.get_or_insert_with(Stop::listen);
        let (lock, mut left) = match held_host(&dir, &log, (lock, &before), stop, &mut watched)? {
            Hold::Stopped(stopped) => return Ok(stopped),
            Hold::Moving { lock, left } => (lock, left),
        };
        let Some(path) = watched.path.clone() else { bail!("no file to move onto") };
        let pid = std::process::id();
        let to = watched.installed().as_ref().and_then(|installed| installed.version.clone());
        let to = to.unwrap_or_default();
        let at = path.display();
        // Read the durable work after every instance stopped, before the
        // replacement can advance it. This is evidence, never a move gate.
        for left in &mut left {
            left.carried = left
                .carrying()
                .unwrap_or_else(|why| vec![format!("could not list what carries over: {why:#}")]);
        }
        match handed_over(&path, lock) {
            Ok(about) => {
                let (moved_to, new) = (about["version"].as_str().unwrap_or(&to), &about["pid"]);
                log.say(&format!(
                    "host runner moved: from afkconvoy {VERSION} (pid {pid}) to afkconvoy \
                     {moved_to} (pid {new}), installed at {at}"
                ));
                let (from, to) = (VERSION.to_string(), moved_to.to_string());
                for left in &left {
                    left.told(
                        &log,
                        What::Moved {
                            from: from.clone(),
                            to: to.clone(),
                            path: path.clone(),
                            carried: left.carried.clone(),
                        },
                    );
                }
                return Ok(json!({"moved": true, "pid": pid, "to": about}));
            }
            Err(why) => {
                let why = format!("{why:#}");
                followed_log();
                log.say(&format!(
                    "host runner not moved: afkconvoy {to}, installed at {at}, did not start: \
                     {why} — it takes the host back, on afkconvoy {VERSION}, and tries that file \
                     no more until it changes"
                ));
                let (from, failed) = (VERSION.to_string(), why.clone());
                let not_moved = What::NotMoved { from, to, path: path.clone(), why: failed };
                left.iter().for_each(|left| left.told(&log, not_moved.clone()));
                watched.failed(why);
            }
        }
    }
}

/// How one hold of the host ended.
enum Hold {
    /// The host runner stopped, as told, and let the host go: its answer.
    Stopped(Value),
    /// Every fleet instance stopped for the move onto the newer afkconvoy
    /// installed: the lock, the host still held by it, and each checkout
    /// held, as its fleet instance left it.
    Moving { lock: File, left: Vec<Left> },
}

/// A checkout whose fleet instance stopped for the host runner's move.
struct Left {
    root: PathBuf,
    /// The fleet instance that stopped, and its host: the move is kept in
    /// its tick log as of it.
    instance: String,
    host: String,
    carried: Vec<String>,
}

impl Left {
    fn carrying(&self) -> Result<Vec<String>> {
        let repo = Repo::at(&self.root)?;
        let fleet = crate::fleet::Fleet::open(&repo, crate::clock())?;
        let mut carried = vec![];
        for claim in &fleet.scan.claims {
            if claim.owner.as_ref().is_some_and(|owner| owner.host == self.host) {
                carried.push(format!("claim #{} and its worker", claim.number));
            }
        }
        for run in &fleet.scan.runs {
            if run.owner.as_ref().is_some_and(|owner| owner.host == self.host) {
                carried.push(format!("run {} and its worker", run.key));
            }
        }
        let train = crate::legacy::Legacy::read(&repo, &fleet.config)?;
        for join in train.joins(&repo, &fleet.gh.tracker)? {
            carried.push(format!("legacy PR awaiting migration #{}", join.pr));
        }
        let source = crate::orca::Source::of(&repo)?;
        if let Some(checkout) =
            crate::orca::Worktree::named(&source, afkconvoy_core::cut::CHECKOUT)?
        {
            carried.push(format!("cut in {}", checkout.path.display()));
        }
        Ok(carried)
    }

    /// Keeps `what`, of the host runner's move, in its tick log, now.
    fn told(&self, log: &RunnerLog, what: What) {
        let (instance, host) = (self.instance.clone(), self.host.clone());
        let entry = log::Entry { at: crate::clock(), instance, host, what };
        if let Err(error) = kept_in(&self.root, &entry) {
            let at = self.root.display();
            log.say(&format!("{at}: its tick log could not keep `{}`: {error:#}", entry.said()));
        }
    }
}

/// Holds the host, its lock taken — `took` is the lock and what the host
/// runner before wrote into it: serves the panel, starts every checkout on
/// its list, and answers on its socket, until `stop` is told, its lock file
/// is gone, or nothing holds back the move onto a newer afkconvoy `watched`
/// finds → how it ended, once every fleet instance has.
fn held_host(
    dir: &Path,
    log: &Arc<RunnerLog>,
    (lock, before): (File, &Value),
    stop: &Stop,
    watched: &mut Watched,
) -> Result<Hold> {
    let ours = identity(&lock.metadata()?);
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
    let held = Mutex::default();
    let shared = Shared {
        installed: Arc::clone(&watched.installed),
        published: Arc::clone(&watched.published),
        ..Shared::default()
    };
    let panel = HostPanel::new(shared.clone());
    // Read once: a port the host settings change to holds from the next start.
    let port = host_settings::of_host().map_or(host_settings::PANEL_PORT, |said| said.panel_port());
    let address = match panel.open(port) {
        Ok((address, taken)) => {
            let taken = if taken { format!(", port {port} being taken") } else { String::new() };
            log.say(&format!("panel: {address}{taken}"));
            Some(address)
        }
        Err(error) => {
            log.say(&format!("panel: not served: {error:#}"));
            None
        }
    };
    let (log, stop, moving) = (Arc::clone(log), stop.clone(), AtomicBool::new(false));
    let runner = Arc::new(Runner { list, log, stop: stop.clone(), moving, held, panel, shared });
    // Every host has a watch, in the operator's directory or the stock one's.
    let watching = {
        let (list, log) = (runner.list.clone(), Arc::clone(&runner.log));
        crate::host_watch::HostWatch::start(
            runner.shared.watch.clone(),
            runner.panel.address(),
            move |what| {
                log.say(&watch_said(&what));
                crate::host_watch::append_to_tick_logs(&list, &what);
            },
        )
    };
    runner.start_listed(listed.checkouts);
    let about = json!({"pid": pid, "version": VERSION, "log": dir.join(LOG), "panel": address});
    let answerer = Arc::clone(&runner);
    let answering = runner_socket::serve(listener, about, move |request| answerer.answer(request));

    let lock_file = dir.join(LOCK);
    let still_ours = || std::fs::metadata(&lock_file).is_ok_and(|now| identity(&now) == ours);
    let mut left = None;
    while !stop.told() && left.is_none() {
        std::thread::sleep(WATCHED_EVERY);
        if !stop.told() && !still_ours() {
            runner.log.say(
                "host runner stopping: its lock file is gone, so another host runner may start",
            );
            stop.tell();
        }
        if !stop.told() && watched.looked(&runner.log) {
            left = runner.stopped_for_move();
        } else {
            runner.shared.standings.resume();
            runner.shared.queue.resume();
        }
    }
    if still_ours() {
        runner.log.say(match left {
            None => "host runner stopping: told to stop, each fleet instance ends its step",
            Some(_) => {
                "host runner moving: nothing holds it back, so each fleet instance stops \
                 between two ticks, and the newer afkconvoy's host runner is started in its \
                 place"
            }
        });
        // A `run` from now on starts a host runner, which waits for this one.
        let _ = std::fs::remove_file(dir.join(runner_socket::SOCKET));
    }
    while !runner.holding().is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(watching);
    // `stop --all` among them, which is answered once they all stopped.
    runner.shared.queue.close();
    answering.settled(ANSWERED_WITHIN);
    let kept = runner.list.read().map_or(0, |listed| listed.checkouts.len());
    // Before the host is let go: the host runner that takes it next serves on the port.
    runner.panel.close();
    // Told to stop as it moved, it stops.
    if let Some(left) = left.filter(|_| !stop.told() && still_ours()) {
        return Ok(Hold::Moving { lock, left });
    }
    runner.log.say(&format!("host runner stopped: {kept} checkouts kept on the list"));
    drop(lock);
    Ok(Hold::Stopped(json!({"stopped": true, "pid": pid, "checkouts": kept})))
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

/// The file the host runner was started from, as it looks at it for a newer
/// afkconvoy installed there: `brew upgrade` points the path at another file,
/// and an install in place writes it anew.
struct Watched {
    /// Its path, as the host runner was started as it — a link Homebrew
    /// keeps among them; `None` where the process cannot say it.
    path: Option<PathBuf>,
    /// The file there as it was last looked at: which file, when it was
    /// last written and how long it is; `None`: no file was.
    seen: Option<((u64, u64), i64, u64)>,
    looked: Instant,
    /// What it found there, where it is newer than this host runner's
    /// afkconvoy or does not say its version: said on the socket.
    installed: Arc<Mutex<Option<Installed>>>,
    /// A newer release published to the tap it was installed from, as the
    /// host runner's looks there find it (`published::learn`).
    published: Arc<Mutex<Option<Published>>>,
}

impl Watched {
    /// The file this process was started from, as it is now: the afkconvoy
    /// of this host runner, which it moves onto none of.
    fn started() -> Watched {
        let path = std::env::current_exe().ok();
        let seen = path.as_deref().and_then(file_at);
        let (installed, published) = (Arc::default(), Arc::default());
        Watched { path, seen, looked: Instant::now(), installed, published }
    }

    fn installed(&self) -> MutexGuard<'_, Option<Installed>> {
        self.installed.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Looks at the file again, once `INSTALLED_LOOKED_EVERY` has passed,
    /// and where it changed asks the afkconvoy there its version, saying in
    /// `log` what it found → whether the host runner moves onto it: one of a
    /// newer release than its own, and not one that did not start.
    fn looked(&mut self, log: &RunnerLog) -> bool {
        if self.looked.elapsed() >= INSTALLED_LOOKED_EVERY {
            self.looked = Instant::now();
            if let Some(path) = &self.path {
                let now = file_at(path);
                if now != self.seen {
                    self.seen = now;
                    let found = now.and_then(|_| found_at(path, log));
                    *self.installed() = found;
                }
            }
        }
        self.moves_at(crate::clock())
    }

    fn moves_at(&self, now: u64) -> bool {
        self.installed().as_ref().is_some_and(|installed| {
            installed.version.is_some()
                && installed.failed.is_none()
                && now >= installed.attempt_since
        })
    }

    /// The move onto what it found failed, `why`: the host runner works on,
    /// on its own, and tries that file no more until it changes.
    fn failed(&mut self, why: String) {
        if let Some(installed) = self.installed().as_mut() {
            (installed.failed, installed.held_back) = (Some(why), vec![]);
            installed.wait_expired = false;
        }
    }
}

/// The file at `path`, as `Watched` tells one from another: which file, when
/// it was last written and how long it is; `None` where there is none.
fn file_at(path: &Path) -> Option<((u64, u64), i64, u64)> {
    let file = std::fs::metadata(path).ok()?;
    Some((identity(&file), file.mtime(), file.len()))
}

/// What the afkconvoy at `path`, a file changed since it was last looked at,
/// is to this host runner, said in `log` → it, where it is of a newer release
/// than this host runner's, or does not say its version; `None` where it is of
/// this release or an older one, which the host runner never moves onto.
fn found_at(path: &Path, log: &RunnerLog) -> Option<Installed> {
    let at = path.display();
    let installed = |version, failed| Installed {
        path: path.to_path_buf(),
        version,
        from: VERSION.to_string(),
        since: crate::clock(),
        attempt_since: crate::clock(),
        wait_expired: false,
        held_back: vec![],
        failed,
    };
    let version = match version_of(path) {
        Ok(version) => version,
        Err(error) => {
            let why = format!("{error:#}");
            log.say(&format!(
                "{at}, which this host runner was started from, changed and does not say its \
                 version: {why} — the host runner works on, of afkconvoy {VERSION}, and looks again \
                 once it changes"
            ));
            return Some(installed(None, Some(why)));
        }
    };
    match found(VERSION, &version) {
        Found::Newer => {
            log.say(&format!(
                "afkconvoy {version} is installed at {at}, newer than this host runner's, \
                 {VERSION}: it moves onto it between ticks, once no exclusive command runs"
            ));
            Some(installed(Some(version), None))
        }
        Found::Older => {
            log.say(&format!(
                "afkconvoy {version} is installed at {at}, older than this host runner's, \
                 {VERSION}: it never moves onto an older one"
            ));
            None
        }
        Found::Same => None,
    }
}

/// The release the afkconvoy at `path` says it is, asked with `--version`
/// and given `VERSION_SAID_WITHIN`: `afkconvoy 0.6.0` is `0.6.0`. Errors: it
/// could not be started, said nothing in time and was ended, ended in an
/// error, or said no version.
pub(crate) fn version_of(path: &Path) -> Result<String> {
    let mut child = crate::process::spawn(
        Command::new(path)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .context("it could not be started")?;
    let until = Instant::now() + VERSION_SAID_WITHIN;
    while child.try_wait()?.is_none() {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            bail!("it said nothing within {}s, and was ended", VERSION_SAID_WITHIN.as_secs());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let said = child.wait_with_output()?;
    let (out, err) = (String::from_utf8_lossy(&said.stdout), String::from_utf8_lossy(&said.stderr));
    if !said.status.success() {
        bail!("`--version` ended {}: {}", said.status, err.trim());
    }
    let version = out.trim().strip_prefix("afkconvoy ").map(str::trim);
    let version = version.filter(|version| !version.is_empty() && !version.contains(' '));
    version.map(str::to_string).with_context(|| format!("`--version` said {:?}", out.trim()))
}

/// Starts the host runner of the afkconvoy at `path` as `run` starts one
/// (`start_detached`), lets the host go, `lock`, and waits `MOVED_WITHIN` for
/// a host runner to answer in this one's place → what it says of itself:
/// {pid, version, log, panel} — the one started, or another a `run` started
/// meanwhile. Errors: it could not be started, or ended before any answered;
/// or none answered in time, and the one started is ended. The host is then
/// free for this one to take back.
fn handed_over(path: &Path, lock: File) -> Result<Value> {
    let (lock_file, log) = (cache()?.join(LOCK), log_path()?);
    // What it says as it ends is what it wrote past here.
    let written = std::fs::metadata(&log).map_or(0, |kept| kept.len());
    let started = start_detached(path);
    // Once it can wait for it: it takes the host as this one lets it go.
    drop(lock);
    let mut child = started?;
    let (pid, until) = (std::process::id(), Instant::now() + MOVED_WITHIN);
    loop {
        if let Ok(Reached::Answer(said)) = runner_socket::ask(&Request::Version, SAID_WITHIN)
            && said["host_runner"]["pid"] != pid
        {
            return Ok(said["host_runner"].clone());
        }
        if let Some(ended) = child.try_wait()?
            && !held(&lock_file)
        {
            bail!("it ended as it started ({ended}): {}", last_lines(&log, written, 5));
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            bail!("it did not answer within {}s, and was ended", MOVED_WITHIN.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl Runner {
    fn holding(&self) -> MutexGuard<'_, BTreeMap<PathBuf, Held>> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether it moves onto a newer afkconvoy: each fleet instance was told
    /// to stop for it, and none starts again.
    fn moved(&self) -> bool {
        self.moving.load(Ordering::Relaxed)
    }

    /// Where nothing holds back the move onto the newer afkconvoy installed
    /// (`held_back`) → tells every fleet instance to stop for it, between two
    /// ticks, and answers each checkout held, as its fleet instance left it;
    /// else `None`. Either way the installed afkconvoy's `held_back` says
    /// what holds it back now. Decided and told while every fleet instance
    /// stands still between two ticks — none begins one meanwhile — and none
    /// is started or added.
    fn stopped_for_move(&self) -> Option<Vec<Left>> {
        self.stopped_for_move_at(crate::clock())
    }

    fn stopped_for_move_at(&self, now: u64) -> Option<Vec<Left>> {
        let holding = self.holding();
        // Prevent a fresh tick or queue grant from racing the safe moment.
        // Existing ticks and the running command finish as ever.
        self.shared.standings.pause();
        let exclusive = self.shared.queue.pause();
        let each: Vec<(String, Option<String>, Option<Stands>)> = holding
            .iter()
            .map(|(root, held)| {
                let stage = held.seen.stage();
                let stands = match stage.came {
                    Came::Broken(_) => Some(Stands::Broken),
                    Came::Ticked { .. } => None,
                    Came::Starting | Came::Refused(_) | Came::Ended(_) => Some(Stands::Starting),
                };
                (root.display().to_string(), stage.instance.clone(), stands)
            })
            .collect();
        let held = self.shared.standings.held_still(|stand| {
            let each: Vec<(String, Stands)> = each
                .iter()
                .map(|(root, instance, stands)| {
                    // One whose loop ended stands no more: it is ending.
                    let ticked = instance.as_ref().and_then(|instance| stand.get(instance));
                    (root.clone(), ticked.copied().or(*stands).unwrap_or(Stands::Starting))
                })
                .collect();
            let held = held_back(&each, exclusive);
            if held.is_empty() {
                self.moving.store(true, Ordering::Relaxed);
                holding.values().for_each(|held| held.stop.tell());
            }
            held
        });
        if let Some(installed) = self.shared.installed().as_mut() {
            installed.held_back = held.iter().map(HeldBack::said).collect();
            if !held.is_empty()
                && now.saturating_sub(installed.attempt_since) >= MOVE_WAIT_WITHIN.as_secs()
            {
                installed.wait_expired = true;
                installed.attempt_since = now.saturating_add(INSTALLED_LOOKED_EVERY.as_secs());
                self.log.say(&format!("host runner not moved: wait exceeded {}s: {} — it lets ticks and the queue run again, and keeps trying the move", MOVE_WAIT_WITHIN.as_secs(), installed.held_back.join("; ")));
                self.shared.standings.resume();
                self.shared.queue.resume();
            }
        }
        if !held.is_empty() {
            return None;
        }
        let left = holding.iter().map(|(root, held)| Left {
            root: root.clone(),
            instance: held.seen.stage().instance.clone().unwrap_or_default(),
            host: held.host.clone(),
            carried: vec![],
        });
        Some(left.collect())
    }

    /// Starts the fleet instance of each checkout `listed`, as the `run`
    /// that added it named it, and as a `run` that named nothing else would
    /// in this process's environment. One whose repo cannot be read stays on
    /// the list, said in the log.
    fn start_listed(self: &Arc<Self>, listed: Vec<Entry>) {
        let unflagged = crate::unflagged_start();
        let mut held = self.holding();
        for Entry { checkout, named, .. } in listed {
            let start =
                unflagged.as_ref().map_err(|error| anyhow!("{error:#}")).and_then(|start| {
                    let key = checkout_list::repo_of(&Repo::at(&checkout)?)?;
                    Ok((key, Start { named, ..start.clone() }))
                });
            match start {
                Ok((key, start)) => {
                    self.start(&mut held, checkout, key, start);
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
                Some(held) if matches!(held.seen.stage().came, Came::Broken(_)) => HeldAs::Broken,
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
            Request::Candidate { pid, validation } => {
                self.shared.queue.validation(pid, validation);
                Ok(json!({}))
            }
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
    /// tick — which leaves one on the list there, broken (`ended`). A
    /// checkout whose fleet instance runs already is answered with that
    /// instance, and `start` changes nothing but its concurrency, its
    /// tracker and its harness; one the host runner is starting is answered
    /// as that start comes; one on the list whose fleet instance does not
    /// run, or is broken, starts now with what the list kept of its `run`,
    /// and what `start` names laid over that. The concurrency, the tracker
    /// and the harness `start` says are the checkout's from now on,
    /// whichever it is (`said`, `tracker::say`, `named_harness::say`). A
    /// harness the host kit does not hold is refused before either of the
    /// others is written.
    fn add(self: &Arc<Self>, checkout: &Path, start: Start) -> Result<Value> {
        if self.stop.told() || self.moved() {
            bail!(
                "the host runner (pid {}) is stopping{}: run again once it has stopped; its log, \
                 {}, says how far it is",
                std::process::id(),
                if self.moved() { ", to move onto a newer afkconvoy" } else { "" },
                log_path()?.display()
            );
        }
        let repo = Repo::at(checkout)?;
        let (root, key) = (repo.dir.clone(), checkout_list::repo_of(&repo)?);
        let tick_log = log::Log::of(&repo)?.file;
        let (seen, added, concurrency, tracked, harness) = {
            let mut held = self.holding();
            let listed = self.list.read()?.checkouts;
            let known = known(&listed, &held);
            let on_list = on_list(&known, &root.display().to_string(), &key);
            // A new start asks nothing that a start of it before asked.
            if matches!(on_list, OnList::No) {
                checkout_list::forget(&root);
            }
            let (said, tracked, harness) = match on_list {
                OnList::Other(_) => (Value::Null, Value::Null, Value::Null),
                OnList::Itself | OnList::No => {
                    let kept = listed.iter().find(|entry| entry.checkout == root);
                    let named = kept.map(|entry| entry.named.clone()).unwrap_or_default();
                    let host = start.named.host.clone().or(named.host);
                    let host = host.unwrap_or_else(crate::this_host);
                    // Before the tracker and the concurrency, so a harness
                    // the host kit does not hold writes nothing.
                    let harness = match start.harness {
                        Some(to) => crate::named_harness::say(&self.list, &repo, to)?,
                        None => Value::Null,
                    };
                    let tracked = match start.tracker.clone() {
                        Some(to) => tracker::say(&self.list, &repo, to, &host)?,
                        None => Value::Null,
                    };
                    (self.said(&repo, start.concurrency)?, tracked, harness)
                }
            };
            let (seen, added) = match on_list {
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
                            // A broken one is started again now, not after its backoff.
                            if matches!(stage.came, Came::Broken(_)) {
                                stage.again = Some(start);
                                stage.came = Came::Starting;
                                running.seen.changed.notify_all();
                            }
                            drop(stage);
                            (Arc::clone(&running.seen), false)
                        }
                        None => (self.start(&mut held, root.clone(), key.clone(), start), false),
                    }
                }
                OnList::No => (self.start(&mut held, root.clone(), key.clone(), start), true),
            };
            (seen, added, said, tracked, harness)
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
                "concurrency": concurrency,
                "tracker": tracked,
                "harness": harness,
                "warnings": if waited { stage.warnings.clone() } else { vec![] },
            })),
            Came::Refused(refused) => Ok(refused.clone()),
            Came::Ended(why) => {
                bail!("its fleet instance ended before this run was answered: {why}")
            }
            Came::Broken(why) => {
                let mut broke = why.clone();
                let said = why["error"].as_str().unwrap_or_default();
                broke["error"] = json!(format!("{said} — {STAYS_BROKEN}"));
                Ok(broke)
            }
            Came::Starting => unreachable!("waited for above"),
        }
    }

    /// Has the checkout `repo` say `to` of its concurrency on this host
    /// (`concurrency::say`) → what that changed of the concurrency its fleet
    /// instance goes by, as {from, to}, each as `concurrency::json` says it;
    /// null where `to` names none, or the one in force. Its config is read
    /// before anything is said: one that cannot be read refuses it.
    fn said(&self, repo: &Repo, to: Option<To<NonZeroUsize>>) -> Result<Value> {
        let Some(to) = to else { return Ok(Value::Null) };
        let was = concurrency::json(&concurrency::config_of(repo)?);
        concurrency::say(&self.list, &repo.dir, to)?;
        let now = concurrency::json(&concurrency::config_of(repo)?);
        Ok(if was == now { Value::Null } else { json!({"from": was, "to": now}) })
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
    ) -> Arc<Seen> {
        let seen = Arc::new(Seen::default());
        let (stop, reach) = (self.stop.instance(), Reach::default());
        let holding = Held {
            repo: key.clone(),
            host: start.named.host.clone().unwrap_or_else(crate::this_host),
            seen: Arc::clone(&seen),
            stop: stop.clone(),
            reach: reach.clone(),
        };
        held.insert(root.clone(), holding);
        self.panel.starting(&root);
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
            let show: Show = {
                let (runner, root) = (Arc::clone(&self), root.to_path_buf());
                Arc::new(move |part| runner.panel.shown(&root, part))
            };
            let held = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let Reach { wakes } = reach.clone();
                let shared = self.shared.clone();
                let holder = Holder { stop: stop.clone(), echo, wakes, shared, show: Some(show) };
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
            self.panel.broke(root, Broken { why: why.clone(), since, next: Some(next) });
            let what = What::Broken { why, since, next };
            let entry = log::Entry { at: now, instance: broke_as.clone(), host, what };
            self.said_of(root, &entry, &said);
            // A `run` waiting on this start is answered once both logs say so.
            seen.changed.notify_all();
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
    /// told to stop or moves, first.
    fn rested(
        &self,
        root: &Path,
        seen: &Seen,
        until: Instant,
    ) -> Option<(Stop, Reach, Option<Start>)> {
        let again = {
            let mut stage = seen.stage();
            loop {
                if stage.removed || self.stop.told() || self.moved() {
                    return None;
                }
                if let Some(again) = stage.again.take() {
                    break Some(again);
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    // A `run` that asks from now on waits for this start.
                    stage.came = Came::Starting;
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
        if seen.stage().removed || self.stop.told() || self.moved() {
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
        self.panel.ended(root);
        let mut stage = seen.stage();
        let why = if stage.removed {
            "it was broken, and `afkconvoy stop` took its checkout off the list"
        } else if self.moved() {
            "it was broken, and the host runner moves onto a newer afkconvoy, which starts it"
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
    /// checkout off the list — after its first tick, or before it where its
    /// checkout is on the list, whoever started it: a `run` that waits for
    /// it is told why, and its checkout stays on the list. Otherwise it
    /// ended, and is held no more: one that ended before its first tick was
    /// refused, and a `run` that waits for it is told why; one `stop` took
    /// off before its first tick, it is told so.
    fn ended(&self, root: &Path, held: Result<Value>) -> Option<String> {
        let mut holding = self.holding();
        let why = match &held {
            Ok(answer) => answer.to_string(),
            Err(error) => format!("{error:#}"),
        };
        let listed = self
            .list
            .read()
            .is_ok_and(|listed| listed.checkouts.iter().any(|entry| entry.checkout == root));
        // Decided and marked under one lock: a `run` that asks meanwhile
        // finds it broken, or waits for the start that ended.
        let broke = holding.get(root).is_some_and(|holds| {
            let mut stage = holds.seen.stage();
            let at_work = match stage.came {
                Came::Starting => listed,
                Came::Ticked { .. } => true,
                Came::Refused(_) | Came::Ended(_) | Came::Broken(_) => false,
            };
            let removed = stage.removed || self.stop.told() || self.moved();
            match &held {
                Err(error) if at_work && !removed => {
                    stage.came = Came::Broken(crate::error_json(error));
                    true
                }
                _ => false,
            }
        });
        if broke {
            return Some(why);
        }
        let gone = holding.remove(root)?;
        self.panel.ended(root);
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
                self.log.say(&match listed {
                    true => format!(
                        "fleet instance not started: {at}: {why} — it stays on the checkout \
                         list, and `afkconvoy run` there starts it again"
                    ),
                    false => format!("checkout refused: {at}: {why}"),
                });
            }
            Came::Ticked { .. } => {
                self.log.say(&format!("fleet instance ended: {at}: {why}"));
                stage.came = Came::Ended(why);
            }
            Came::Refused(_) | Came::Ended(_) | Came::Broken(_) => {
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
        match kept_in(root, entry) {
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

/// Keeps `entry` in the tick log of the checkout `root`: a line the host
/// runner writes of one of its fleet instances.
fn kept_in(root: &Path, entry: &log::Entry) -> Result<()> {
    Repo::at(root).and_then(|repo| log::Log::of(&repo)?.append(entry))
}

/// What a `run` is told after why the fleet instance of its checkout, on the
/// list already, broke before it was answered: refused as it started, say.
const STAYS_BROKEN: &str = "its checkout stays on the checkout list, and its fleet instance is \
                            broken: the host runner starts it again after a backoff, and a `run` \
                            of it at once; `afkconvoy doctor` in the checkout says why and when";

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
                    // With the concurrency and the tracker its `run` asked,
                    // which this process forgets once the list holds them.
                    let checkout_list::Asked { concurrency, tracker, harness } =
                        checkout_list::asked_of(&self.root);
                    let checkout = self.root.clone();
                    listed.checkouts.push(Entry {
                        checkout,
                        named,
                        concurrency,
                        tracker,
                        harness,
                        harness_applied: None,
                    });
                    true
                }
            }
        });
        stage.came = match listed {
            Ok(added) => {
                checkout_list::forget(&self.root);
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
/// release it stopped first (`older_of`), and `settings` what it changed of
/// the host settings. A refusal the host runner answered is `Answered`,
/// printed as it is.
pub fn run(
    checkout: &Repo,
    mut start: Start,
    settings: &host_settings::Named,
    now: u64,
) -> Result<Value> {
    let (log, lock, list) = (log_path()?, cache()?.join(LOCK), List::of_host()?);
    let pid = || holder_of(&lock).map(|pid| format!(" (pid {pid})")).unwrap_or_default();
    // Before anything changes: which afkconvoy the host runner is, and
    // whether this `run` starts one — none answers, or one of an older
    // release.
    let (older, starts) = match runner_socket::ask(&Request::Version, SAID_WITHIN)? {
        Reached::Answer(said) => {
            let older = older_of(&said["host_runner"])?;
            let starts = older.is_some();
            (older, starts)
        }
        Reached::Nobody | Reached::Closed => (None, true),
        Reached::Unanswered => bail!(
            "the host runner{} did not say its version within {}s: its log, {}, says why",
            pid(),
            SAID_WITHIN.as_secs(),
            log.display()
        ),
    };
    // One this `run` starts starts every checkout on the list before it is
    // handed this one, which goes by its entry there: the tracker this `run`
    // says is kept with it first (`tracker_first`).
    let tracker = if starts { tracker_first(&list, checkout, &start)? } else { None };
    let replaced = older.map(Older::replaced).transpose()?;
    // A first start writes the host kit here, saying so to whoever ran `run`.
    kit::ensure(now)?;
    // Before the checkout is handed over: its fleet instance starts under
    // them, and every other on the host reads them as it goes.
    let changed = host_settings::change(&list, settings)?;
    host_settings::pin_host_name(&list)?;
    // What it declares of the agent setup changed: every held harness is
    // given it as it is declared now (sunfmin/control#528).
    let agent_setup = ["agent_instructions", "agent_skills", "agent_guards"]
        .iter()
        .any(|key| changed.contains_key(*key))
        .then(|| kit::give_every_agent_setup(now));
    // Kept once no host runner runs: one of an older release would write the
    // list over it, knowing no tracker. One whose entry is gone meanwhile is
    // the host runner's to say, as it is handed the checkout.
    let tracked = match tracker {
        Some(checked) => checked.kept(&list, checkout)?,
        None => None,
    };
    if tracked.is_some() {
        start.tracker = None;
    }
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
    if let Some(agent_setup) = agent_setup {
        answer["agent_setup"] = agent_setup;
    }
    if let Some(tracked) = tracked {
        answer["tracker"] = tracked;
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

/// `afkconvoy panel --open`: asks this host's runner where it serves the
/// panel → {panel, host_runner}. Nothing is started, and nothing changes.
pub fn panel() -> Result<Value> {
    let log = log_path()?;
    let about = match runner_socket::ask(&Request::Version, SAID_WITHIN)? {
        Reached::Answer(said) => said["host_runner"].clone(),
        Reached::Nobody | Reached::Closed => bail!(
            "no host runner runs on this host, so no panel is served: `afkconvoy run` starts one, \
             which serves it, and `afkconvoy panel` prints it once"
        ),
        Reached::Unanswered => bail!(
            "the host runner{} did not answer within {}s: its log, {}, says why",
            holder_of(&cache()?.join(LOCK)).map(|pid| format!(" (pid {pid})")).unwrap_or_default(),
            SAID_WITHIN.as_secs(),
            log.display()
        ),
    };
    let (theirs, pid) = (about["version"].as_str().unwrap_or_default(), &about["pid"]);
    match about["panel"].as_str() {
        Some(address) => Ok(json!({"panel": address, "host_runner": about})),
        None if about["foreground"] == true => bail!(
            "this host's runner is a fleet instance held in the foreground (`run --foreground`, \
             pid {pid}), which serves no panel: it warned why as it started"
        ),
        None if found(VERSION, theirs) == Found::Older => bail!(
            "the host runner (pid {pid}) is afkconvoy {theirs}, which serves no panel of the host: \
             `afkconvoy run` in any checkout moves it onto this afkconvoy, {VERSION}"
        ),
        None => bail!(
            "the host runner (pid {pid}) serves no panel: its log, {}, says why",
            log.display()
        ),
    }
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

/// What `run` makes of the host runner that said it is `about` — its pid and
/// version — before it hands it anything (docs/adr/0023) → one of an older
/// release, which it replaces (`Older::replaced`). One of this afkconvoy's
/// release is left as it is → `None`. One of a newer release refuses this
/// `run`, which changes nothing, and so does a fleet instance held in the
/// foreground, whatever its release.
fn older_of(about: &Value) -> Result<Option<Older>> {
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
            Ok(Some(Older { pid, version: theirs.to_string() }))
        }
    }
}

/// A host runner of an older release than this `run`'s, which it replaces.
struct Older {
    pid: u64,
    version: String,
}

impl Older {
    /// Stops it as SIGTERM stops it — each fleet instance finishes the step
    /// it is in, and the checkout list is kept — and waits until it lets the
    /// host go → its {pid, version}.
    fn replaced(self) -> Result<Value> {
        let Older { pid, version } = self;
        let told = crate::process::status(Command::new("kill").args(["-TERM", &pid.to_string()]));
        told.context("`kill` could not be run to stop the host runner")?;
        // However `kill` ended: one already gone has let the host go.
        if !let_go(&cache()?.join(LOCK), UP_WITHIN) {
            bail!(
                "the host runner of afkconvoy {version} (pid {pid}) was told to stop, to make way \
                 for this afkconvoy, {VERSION}, and still holds the host after {}s: its log, {}, \
                 says how far it is. Run again once it has stopped",
                UP_WITHIN.as_secs(),
                log_path()?.display()
            );
        }
        Ok(json!({"pid": pid, "version": version}))
    }
}

/// What the `run` of the checkout `checkout`, as `start` asks, says of its
/// tracker before it starts the host runner, which starts every checkout on
/// `list` as its entry has it before it is handed this one: where `start`
/// names a tracker and the list holds an entry of the checkout, that tracker
/// checked (`tracker::check`) → to keep with the entry; `None` where there
/// is nothing to say first. A tracker the check refuses refuses this `run`
/// before anything changes.
fn tracker_first(list: &List, checkout: &Repo, start: &Start) -> Result<Option<Checked>> {
    let Some(to) = start.tracker.clone() else { return Ok(None) };
    let listed = list.read()?.checkouts;
    let Some(entry) = listed.into_iter().find(|entry| entry.checkout == checkout.dir) else {
        return Ok(None);
    };
    // Its claims are stamped with the host its fleet instance starts on.
    let host = start.named.clone().or(entry.named).host.unwrap_or_else(crate::this_host);
    tracker::check(list, checkout, to, &host).map(Some)
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
/// host runner, and the panel, served as a host runner serves it and
/// said where on the socket alone — printing every line as `run` printed it, until SIGTERM or
/// SIGINT, or until it has made `start.knobs.ticks` → what its loop
/// answers. It holds no other checkout and leaves the checkout list alone:
/// a `run` or a `stop` while it holds the host is refused, and so is it,
/// where another host runner holds the host. What it names of the host
/// settings, `settings`, holds in this process alone, laid over the host's;
/// and what it says of its checkout's concurrency, its tracker and its
/// harness, where the list holds no entry of it.
pub fn foreground(checkout: &Repo, start: &Start, settings: host_settings::Named) -> Result<Value> {
    host_settings::lay_for_this_process(settings);
    if let Some(to) = start.harness {
        crate::named_harness::say(&List::of_host()?, checkout, to)?;
    }
    if let Some(to) = start.concurrency {
        concurrency::ask(&checkout.dir, to);
    }
    if let Some(to) = start.tracker.clone() {
        let host = start.named.host.clone().unwrap_or_else(crate::this_host);
        tracker::ask(&List::of_host()?, checkout, to, &host)?;
    }
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
    let panel = Arc::new(HostPanel::new(shared.clone()));
    let port = host_settings::of_host().map_or(host_settings::PANEL_PORT, |said| said.panel_port());
    let address = panel.open(port).map(|(address, _)| address);
    let address = address.inspect_err(|error| {
        probe::announce(&[format!("the panel is not served: {error:#}")]);
    });
    panel.starting(&root);
    let show: Show = {
        let (panel, root) = (Arc::clone(&panel), root.clone());
        Arc::new(move |part| panel.shown(&root, part))
    };
    let about = json!({"pid": pid, "version": VERSION, "foreground": true, "panel": address.ok()});
    let answerer = shared.clone();
    let answering = runner_socket::serve(runner_socket::listen()?, about, move |request| {
        let answered = match request {
            Request::Version => Ok(json!({})),
            Request::Exclusive { ticket } => return answerer.queue.answer(ticket),
            Request::Candidate { pid, validation } => {
                answerer.queue.validation(pid, validation);
                Ok(json!({}))
            }
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
    let Reach { wakes } = reach;
    let watching = {
        let list = List::of_host()?;
        crate::host_watch::HostWatch::start(shared.watch.clone(), panel.address(), move |what| {
            println!("{}", watch_said(&what));
            crate::host_watch::append_to_tick_logs(&list, &what);
        })
    };
    let holder = Holder { stop: stop.instance(), echo, wakes, shared, show: Some(show) };
    let held = instance::hold(checkout, start, holder);
    drop(watching);
    // It holds the host no more: a `run` from now on starts a host runner,
    // which waits for the lock.
    let _ = std::fs::remove_file(dir.join(runner_socket::SOCKET));
    answering.settled(ANSWERED_WITHIN);
    panel.close();
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
    let this = std::env::current_exe().context("cannot find this afkconvoy")?;
    let mut child = start_detached(&this)?;
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
                last_lines(&log, 0, 20)
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

/// Starts the hidden `afkconvoy host-runner` of the afkconvoy at `program` —
/// this `run`'s own, or the newer one a host runner moves onto — detached
/// from this process: in a process group of its own, so a terminal's Ctrl-C
/// and hangup and a harness that ends the caller's group all miss it; from /;
/// with nothing on stdin and its stdout and stderr appended to its log; and
/// with the environment of a fresh login shell, but for the cache and config
/// directories of this process (`pinned`). It is never waited on: once this
/// process exits, it is nobody's child.
fn start_detached(program: &Path) -> Result<Child> {
    let dir = cache()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let env = host_environment()
        .context("the host runner starts with the environment of a fresh login shell")?;
    let log = opened_log(&dir.join(LOG))?;
    crate::process::spawn(
        crate::process::inheriting_nothing(&mut Command::new(program))
            .arg("host-runner")
            .env_clear()
            .envs(env)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0),
    )
    .context("the host runner could not be started")
}

/// The host's own environment, as the host runner has it: a fresh login
/// shell's (`launch::login_environment`), but for the cache and config
/// directories of this process (`pinned`). Nothing a session set for itself
/// is in it — no worker sandbox's proxy, `TMPDIR` or tool homes — whichever
/// process asks.
pub(crate) fn host_environment() -> Result<Vec<(OsString, OsString)>> {
    let env = launch::login_environment()?;
    let kit = kit::path()?;
    let config = kit.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
    Ok(pinned(env, &cache()?, config))
}

/// `env` where it gives the cache directory `cache` and the config
/// directory `config` of this `run`, and otherwise with XDG_CACHE_HOME and
/// XDG_CONFIG_HOME set to them: the host runner reads the list, binds the
/// socket and reads the host kit exactly where the `run` that started it
/// does. A login shell that sets neither, under the HOME whose `.cache` and
/// `.config` they are, is given neither: every gate the host runner runs,
/// and every tool a gate calls, reads its config where the host user's own
/// shell does (sunfmin/control#765) — Homebrew reads `trust.json` from
/// `$XDG_CONFIG_HOME/homebrew` once the variable is set, and from
/// `~/.homebrew` while it is not.
fn pinned(
    env: Vec<(OsString, OsString)>,
    cache: &Path,
    config: &Path,
) -> Vec<(OsString, OsString)> {
    let cache = cache.parent().unwrap_or(cache);
    let given = |name: &str, under: &str| {
        let set = env.iter().find(|(set, _)| set == name).map(|(_, value)| PathBuf::from(value));
        set.or_else(|| {
            let home = env.iter().find(|(set, _)| set == "HOME");
            home.map(|(_, home)| Path::new(home).join(under))
        })
    };
    let pins: Vec<(&str, &Path)> =
        [("XDG_CACHE_HOME", cache, ".cache"), ("XDG_CONFIG_HOME", config, ".config")]
            .into_iter()
            .filter(|(name, wanted, under)| given(name, under).as_deref() != Some(*wanted))
            .map(|(name, wanted, _)| (name, wanted))
            .collect();
    let unpinned = env.into_iter().filter(|(name, _)| !pins.iter().any(|(pin, _)| name == pin));
    let pinned = pins.iter().map(|(name, value)| ((*name).into(), value.as_os_str().into()));
    unpinned.chain(pinned).collect()
}

/// Points this host runner's stdout and stderr, which are its log, at the
/// log as it is now: starting another host runner may have moved the file
/// they were aside (`opened_log`). Where it cannot be opened, they stay.
fn followed_log() {
    let Ok(log) = log_path().and_then(|file| opened_log(&file)) else { return };
    for to in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: both descriptors are open; `to` is pointed at the file
        // `log` holds, which stays open there as `log` is dropped.
        unsafe { libc::dup2(log.as_raw_fd(), to) };
    }
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

/// The last `count` lines of `file` written past its first `from` bytes —
/// of all of it, where it is shorter, having rolled — or nothing where it
/// cannot be read.
fn last_lines(file: &Path, from: u64, count: usize) -> String {
    let bytes = std::fs::read(file).unwrap_or_default();
    let from = usize::try_from(from).ok().filter(|from| *from <= bytes.len()).unwrap_or(0);
    let text = String::from_utf8_lossy(&bytes[from..]);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

/// What `afkconvoy stop --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const STOP_HELP: &str = r#"## When to run it

To stop working one repo on this host and leave the others worked: `afkconvoy stop` in its checkout, or naming it. To stop every repo — to start every fleet instance again with flags of its own, say — `afkconvoy stop --all`: the next `run`, in any checkout, starts every checkout on the list again. An upgrade needs none: the host runner moves onto the new release by itself between ticks (`afkconvoy run --help`).

## Who runs it

The operator, in a checkout on the list or naming one; with `--all`, anywhere on this host.

## What it reads and writes

The checkout list: checkouts.json in the cache directory. `stop` takes the checkout off it; `--all` leaves it as it is.

The host runner, spoken to on its socket in the cache directory (host-runner.sock), naming the checkout. Its fleet instance of the checkout — every one, with `--all` — stops as SIGTERM has it stop: it finishes the step it is in, makes none of the steps left in its tick, and ends — within a second when it sleeps. No claim is released and no worker is stopped: the workers work on. Holding no claim, it takes its heartbeat off the remote; holding one, it leaves it with the claims, and the next start on this host takes them back. Its tick log keeps its `stop:` line, as ever. With `--all`, the host runner ends once every fleet instance has, and `stop` answers once it has ended.

The host runner's log has a line of each, beside the line of each fleet instance that ended:

```
checkout removed: /Users/me/Developments/hkj: by `afkconvoy stop`, its fleet instance stops
fleet instance ended: /Users/me/Developments/hkj: {"instance":"me-h1-1a2b3c4d","stopped":true,"ticks":12}
host runner told to stop by `afkconvoy stop --all`: it keeps the checkout list
host runner stopping: told to stop, each fleet instance ends its step
host runner stopped: 2 checkouts kept on the list
```

With no host runner running, `stop` takes the checkout off the list itself, and says so in that log; `--all` changes nothing.

## What it answers

Once its fleet instance has stopped, with exit 0:

```json
{"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj", "removed": true,
 "instance": "me-h1-1a2b3c4d", "stop": "stop: holding no claim, it took its heartbeat with it",
 "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
                 "panel": "http://127.0.0.1:7070/"}}
```

- `removed`: false where the checkout was not on the list. Where no fleet instance of it ran either, nothing changed, and that is all it answers: a checkout not on the list exits 0.
- `instance` and `stop`: the fleet instance that stopped, and the `stop:` line its tick log keeps; null where none ran — the host runner could not start it, or no host runner runs.
- `host_runner`: the one that answered — its pid, its version, its log and where it serves the panel — or null where none runs. `<cache>` stands for the cache directory, whole in the answer.

With `--all`, once the host runner has ended:

```json
{"stopped": [{"checkout": "/Users/me/Developments/hkj", "instance": "me-h1-1a2b3c4d",
  "stop": "stop: it left its heartbeat with the claims it holds: the next start on this host takes them"}],
 "checkouts": 2,
 "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
                 "panel": "http://127.0.0.1:7070/"}}
```

- `stopped`: each fleet instance it stopped, with its checkout, its id and its `stop:` line; [] where no host runner runs.
- `checkouts`: how many the list keeps: the next `run` starts each.

Errors, as {"error": …} with a non-zero exit: a second checkout of a repo on the list, naming the checkout to stop it from; the checkout list cannot be read or written; the host runner ended before it answered; its fleet instance did not stop within 10 minutes — it is off the list, and still finishing the step it is in; the host runner did not end within 30 seconds of answering `--all`. Each error a host runner gave says which, as `host_runner`.

## What trips you up

- SIGTERM to the host runner's pid is `stop --all` but for the answer, and the line that says who told it. A second signal ends it at once, as SIGKILL does, with no goodbye.
- `stop` of the last checkout on the list leaves the host runner running, with none: the next `run` hands it its checkout.
- A `run` of a checkout whose fleet instance is stopping is refused until it has stopped. `stop` answers once it has: run it after.

## Releasing its claims: `--release`

Plain `stop` is the default: there is no drain. The claims stay on the remote, the workers keep working, and the next start on this host takes the claims back. An orphaned claim is still recovered by continuation, and never released.

`stop --release` is the drain an operator names. It is read while the checkout is still on the list, so the tracker is the one its entry says. While the fleet instance — `--instance`, or the one this host's runner holds of the checkout, else the newest its tick log names — still ticks, what it holds is read: a cut it holds is an error, and a claim whose worker declared a verdict, or whose issue is to be settled, is named in `kept` and keeps the checkout on the list, with nothing released. Otherwise the fleet instance is stopped as plain `stop` stops it, and then, in one push, each `claim/<owner>/<tracker>/<n>` it holds and each claim on its patrols' runs is deleted, each only while it is still that instance's, leased to the sha just read. One that moved since it was read is named in `kept`, nothing is released, and the checkout is back on the list: `afkconvoy run` starts its fleet instance again. Once they are gone, each worker is ended: its terminal is closed, and its worktree removed only where it holds no commit past its target and a clean tree; one that does stays, named in the answer. A patrol's checkout is removed as a run's end removes one, unless it left a commit or a change. The issues keep their labels, an open PR stays open, and an issue still ready is back on the frontier. Then the checkout leaves the list, and holding nothing, the fleet instance takes its heartbeat with it. Another host's claims are not touched.

```json
{"checkout": "/Users/me/Developments/hkj", "instance": "me-h1-1a2b3c4d", "removed": true,
 "released": [{"issue": 7, "claim": "refs/convoy/claim/acme/tracker/7",
               "worktree": "/Users/me/orca/hkj/acme-tracker-7-fix", "worktree_removed": false,
               "worktree_kept": "it holds an uncommitted change"}],
 "kept": [], "heartbeat_removed": true, "stop": "stop: …", "host_runner": null}
```

Errors of `--release`, as {"error": …} with a non-zero exit, each with nothing released and the checkout on the list as it was: the checkout is not on the list, so its tracker is not said; no fleet instance of it is known on this host — name one, `--instance`; the fleet instance holds the cut, which is a publish and no release ends; the fleet could not be read.

`--all` takes no `--release`: stopping the host runner keeps the list. Off the list the tracker is not said: `scan` and `release` then exit non-zero, saying so, while the remote holds claims named for a tracker."#;

/// What `afkconvoy run --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const RUN_HELP: &str = r#"## When to run it

To have a repo worked: after `config` answers and `plan` shows the frontier you expect. Run it again after the host runner was stopped or killed, or the host restarted: it starts every checkout on the list, each where its fleet instance left off. An upgrade of afkconvoy (`brew upgrade afkconvoy`) needs none: the host runner moves onto the new version itself between ticks, every checkout on the list with it (below).

## Who runs it

The operator, in a checkout of the repo or naming one. It returns once that checkout's fleet instance has ended its first tick; the host runner ticks on, in the background, until it is stopped.

## What it reads and writes

The checkout list: checkouts.json in the cache directory, kept on this host and never pushed. A checkout goes on it once its fleet instance's first tick ended, with the `--instance`, `--host` and `--worker-no-proxy` its `run` named; one refused before its first tick is not put there, and one there already stays there, broken (below). A `run` of a checkout on the list whose fleet instance does not run starts it with those, but for each that `run` names itself.

The checkout's concurrency, with its entry: how many claims its fleet instance holds at work at once on this host. `--concurrency 6` says it — a whole number, at least 1 — and `--concurrency default` changes it back to the default, {const:CONCURRENCY}; a `run` that names none leaves it as it is. Until the checkout says one, its fleet instance holds {const:CONCURRENCY}. It is said here and nowhere else: the repo config's `concurrency` is read no more, and `afkconvoy doctor` reports it as `info`, with the key to delete, first giving the `run` that keeps the config's number only where the checkout says none and the numbers differ. Unlike the `--instance`, `--host` and `--worker-no-proxy` it named, which hold from the start of its fleet instance, it is read at each tick, as the host settings are: a change holds at the next tick, and the fleet instance is never started again for it. Lowered below the claims held, it ends no worker: those claims are worked to the end — the tick's line counts those past it, as `holding 2/2 · 1 past the bound` — and no new one is taken until fewer are held. A checkout the list does not hold yet goes by it from its first tick, which puts it on the list with it. A host runner started again keeps it; `plan`, `doctor` and `checkouts` say the one in force and who says it.

The checkout's tracker, with its entry, read at each tick as its concurrency is: the repo its fleet reads and writes its issues in — the issues, their labels, comments, blockers, escalations and closing — while PRs, branches, the gate and their landing turns stay the checkout's own repo's. `--tracker me/issues` says it, and `--tracker default`, or the checkout's own repo, says none; a `run` that names none leaves it as it is. Until the checkout says one, its tracker is its own repo. A tracker another repo than the checkout's holds the issues of many repos: the fleet works the open issues there that carry its repo's repo label, `repo:<owner>/<repo>`, and the ready label — in a bound repo, a flow label; every issue it or a worker opens there carries the repo label, and `intake` and `lock` give it to one that lacks it; and a PR or a landing commit names its issue `me/issues#7`, never `#7`, which is the checkout's own repo's. `run` makes the repo label, and the ready, escalate and lock labels of the repo config, in the tracker where it lacks them. It refuses to change the tracker while the checkout's fleet holds a claim on an issue of the tracker it had, or a legacy line still carries a PR of one, naming them, and changes nothing: run it again once they are done. The issues of the tracker it had stay there. A tracker GitHub says is public is refused, and nothing is made in it or changed: whoever can comment on an issue there writes what the fleet reads as its state and what a worker reads as its task. So is the checkout's own repo, where it is public and the checkout says no tracker, or `default`: every start of its fleet is refused until it says a private one, `--tracker <owner>/<private repo>`. A `run` that starts the host runner — where none runs, or where it moves one of an older release (below) — says the tracker of a checkout on the list with its entry before it starts it, as that host runner starts every checkout on the list, as its entry has it, before it is handed this one: it asks GitHub and makes the labels before an older host runner is stopped, which changes nothing where it refuses, and keeps the tracker once that host runner has let the host go. So its checkout's fleet instance starts under the tracker this `run` names, never under the one its entry had, and a checkout of a public repo is taken at the first `run --tracker <owner>/<private repo>`, whichever host runner was there. `plan`, `doctor`, `checkouts` and the panel say the tracker.

The host settings, in the same file: what this host says for every fleet instance on it — the limits past which a worker is hot (`--hot-cores`, `--hot-memory`, `--hot-written`), the shares admission holds back at (`--admit-load`, `--admit-memory`, `--admit-disk`, `--admit-inspect-after`), the panel's port (`--panel`), the operator's directory a watch starts in and how often the host runner looks (`--watch-dir`, `--watch-every`; WATCH, below), and the command that prints the TypeSafe key Jev's worker-health judgments are asked with (`--jev-key-command`), and the shell command that sends human decisions (`--notify-command`; none by default), and the agent setup given to every harness the host kit holds: its instructions file, its skill list and its guard list (`--agent-instructions`, `--agent-skills`, `--agent-guards`; none by default; `afkconvoy kit --help`, The agent setup). A `run` that names one — by its flag, or by the variable of its environment that stands for it, the flag winning: AFKCONVOY_HOT_CORES, AFKCONVOY_HOT_MEMORY, AFKCONVOY_HOT_WRITTEN, AFKCONVOY_ADMIT_LOAD, AFKCONVOY_ADMIT_MEMORY, AFKCONVOY_ADMIT_DISK, AFKCONVOY_ADMIT_INSPECT_AFTER, AFKCONVOY_WATCH_DIR, AFKCONVOY_WATCH_EVERY, AFKCONVOY_JEV_KEY_COMMAND, AFKCONVOY_NOTIFY_COMMAND, AFKCONVOY_AGENT_INSTRUCTIONS, AFKCONVOY_AGENT_SKILLS, AFKCONVOY_AGENT_GUARDS — changes it for the host before it hands its checkout over. Every fleet instance on the host reads them as it goes — admission at each tick, the hot limits at each sample, the watch's before each look, the key command at each worker-health judgment — so a change holds for all of them at once, and none is started again for it; the host runner reads the panel's port as it starts. A `run` that names none changes none, and nothing else changes them: `tick`, `plan` and `doctor` read them as they are, and a host runner started again keeps them. `default` changes one back to its default (`--hot-cores default`). `afkconvoy doctor` says those in force, as `host.settings`.

The host runner: one process of the host, holding a fleet instance for each checkout on the list, each ticking as a fleet instance always has. `run` asks it on its socket in the cache directory (host-runner.sock) — the one socket of the host: `stop`, `wake`, `panel --open`, `exclusive` and `doctor` ask it there too, each naming its checkout, and no fleet instance or repo has one of its own; one nobody listens on, which a host runner killed with no goodbye left, is removed by whoever finds it — and starts it where none answers: in a process group of its own, so the terminal that ran `run` can close, in /, with a fresh login shell's environment — the names your login shell (`$SHELL -lic`) exports, and none of those of the shell that ran `run`. A variable exported only in that shell, or one a session set for itself (Claude Code's, orca's), reaches neither the host runner nor any worker it starts: put it in your login shell's rc files. It keeps the cache directory and the config directory of the `run` that started it. Started again, it starts every checkout on the list, each with what its `run` named, under the host settings the file keeps; a checkout whose fleet instance cannot start then is broken (below), and stays on the list.

Before anything else, `run` asks the host runner for its version. One of an older release than this `run`'s — after `brew upgrade afkconvoy` — is stopped as SIGTERM stops it: each fleet instance finishes the step it is in, and the checkout list is kept. Once it has let the host go, `run` starts a host runner of its own version, which starts every checkout on the same list, and then adds its own — saying its checkout's tracker with its entry there first, where it names one (above). What the old one held for every fleet instance goes with it: a watch goes on in its terminal with its original time limit, and an `exclusive` command waiting in the old one's queue waits in the next one's, from when it first asked (`afkconvoy exclusive --help`). One of a newer release refuses this `run`, which changes nothing. A build past a release (`afkconvoy --version` says `0.6.0+11.g9f58035`) is of that release, and one of the same release as the host runner's is handed the checkout as ever.

The host runner moves onto a newer release by itself, with nothing run by hand. It watches the file it was started from — the path the `run` that started it was run as, such as the link Homebrew keeps, /opt/homebrew/bin/afkconvoy — and within {const:INSTALLED_LOOKED_SECONDS}s of a change there, as `brew upgrade afkconvoy` makes, asks it `--version`. Once a newer release answers, it pauses new ticks and new exclusive commands, lets active ticks and the running exclusive command finish, and moves between ticks. A command waiting in the exclusive queue holds nothing back: it rejoins the new runner with when it first asked. Claims stamped with this host, workers in their terminals, PRs waiting to land, a cut and a watch all carry over; no worker is stopped. It waits at most {const:MOVE_WAIT_SECONDS}s for that moment. If it still cannot move, it lets ticks and the queue run again on the old release, and `afkconvoy doctor` gives a finding with what held it, as `host.installed.held_back`; it keeps trying the move automatically, while the finding stays visible. `host.installed.since` is when that file answered, in epoch seconds; `wait_expired` says that this wait expired. Then each fleet instance stops between two ticks, keeping its `stop` line, and the host runner starts the newer afkconvoy's in its place, with the same checkout list and host settings, and lets the host go to it. That one starts every checkout on the list, each where its fleet instance left off; `checkouts` and the next `run` answer its pid and version. It never moves onto an older release, nor onto a build of its own release. One that does not start, or does not answer within {const:MOVED_WITHIN_SECONDS}s, leaves the old host runner holding the host and starting every checkout on the list again; `afkconvoy doctor` then gives a finding naming it, as it does for one there that does not say its version, and the host runner tries that file again only once it changes. The move, with both releases and what carried over, is a line of the host runner's log and of each checkout's tick log — `moved`, or `not_moved` with why it did not start — which `doctor` lists among its events.

A host learns of a release it did not cut by itself, too. Only the host that cuts a release installs what it publishes (`afkconvoy cut --help`); every other host's Homebrew would learn of it only at a `brew update` nobody ran. So a host runner started from a Homebrew install from a tap looks at that tap's formula on GitHub as it starts and every {const:PUBLISHED_LOOKED_MINUTES} minutes after. Where it publishes a release newer than the host runner's, the look after the one that found it runs `brew update` and `brew upgrade <tap>/afkconvoy` — one look later, so that it never runs beside the cut's own install on the host that cuts — and the move above follows. A release published while a host runner runs is installed within twice {const:PUBLISHED_LOOKED_MINUTES} minutes of its publishing, and the host runner moves onto it within {const:MOVE_WAIT_SECONDS}s more, or `doctor` says what held it. Until it has moved, `afkconvoy doctor` gives a finding naming the release Homebrew has installed and the one published (`host.published`). What `brew` said is in brew-upgrade.log in the cache directory; an upgrade that failed is tried again at the next look. A host runner started from no Homebrew install, such as a build, looks at no tap.

The host runner's log: host-runner.log in the cache directory, moved aside to host-runner.log.1 once past a MiB. One line, stamped in UTC with its zone named (`2026-10-30T13:56:16Z host runner started: …`), as it starts and as it stops, as it replaces a host runner of an older release (`host runner upgraded: from afkconvoy 0.5.13 (pid 4100) to afkconvoy 0.6.0 (pid 4242)`), as it finds another release installed where it was started from, as nothing holds the move back any more, as it moves onto it (`host runner moved: from afkconvoy 0.6.0 (pid 4242) to afkconvoy 0.6.1 (pid 4300), installed at /opt/homebrew/bin/afkconvoy`) or that one does not start (`host runner not moved: …`, with what it said as it ended), as each fleet instance starts and ends, as one breaks and is back, as a checkout is added, refused or removed, as anything in it panics (`panicked at <file>:<line>: <what it said>`), where it serves the panel, as it starts (`panel: http://127.0.0.1:7070/`), and each line a fleet instance says outside its tick log, after its checkout: its warnings and the `start:` lines after its first:

```
fleet instance broken: /Users/me/Developments/hkj: it panicked: <what it said> · since 2026-10-24 09:12:00 UTC · started again at 2026-10-24 09:13:00 UTC, in 60s
fleet instance back: /Users/me/Developments/hkj: me-h1-5e6f7a8b, broken since 2026-10-24 09:12:00 UTC, for 75s
```

Two kinds of checkout are refused, with an error: one whose fleet instance is refused before its first tick — it fails the startup probe, say — which is not put on the list, and stays on it, broken (below), where it was on it already, whichever start was refused — the host runner's, as it starts or moves, or this `run`'s; and a second checkout of a repo already on the list, a clone or a worktree of it: a host runs one fleet instance per repo, and the error names the checkout to run it from.

A fleet instance is broken when its loop panics or ends in an error, or when a start of it ends before its first tick while its checkout is on the list — the host runner's, from the list or after it broke, or a `run`'s: its startup probe no longer passes, say. It harms no other: the host runner and every other fleet instance go on, and its checkout stays on the list. The host runner starts it again after a backoff: {const:BACKOFF_FIRST_MINUTES} minute after it first broke, twice the wait before at each break in a row — one within {const:BACKOFF_MOST_MINUTES} minutes of the start before it — and {const:BACKOFF_MOST_MINUTES} minutes at the most. A `run` of its checkout starts it at once, with what that `run` names laid over what the list kept; a start of it that ends before its first tick is then refused to that `run`, which is told so, and it stays on the list, broken. Only `stop` takes a checkout off the list. It is back once a start of it again has ended its first tick. Broken — with why, since when, and when it is started again — and back are lines of the host runner's log and of its tick log, and `afkconvoy doctor` in its checkout says it is broken, why, and when it is started again. A tick that fails is no break: it is a line of its own, and the loop goes on.

A repo can declare gate needs (`afkconvoy config --help`): the startup probe runs each, and a running fleet instance checks again within an hour. A failure breaks the instance. The refusal and `doctor` name the check, its status, the private host-cache file holding its last 20 stderr lines, and the line to fix it. Check output never appears in the tick log, `plan`, `doctor` or an issue; stdout is discarded.

Then everything a fleet instance does. It makes the startup probe — at every start, whatever probe passed before — takes every claim stamped with this host — on an issue, on a patrol's run or on a cut — removes the heartbeats that are dead and hold nothing (those of the instances it took from, this host's past the lease, anyone's older than a day) — then ticks: each tick rebuilds the working set from GitHub and the remote and makes every transition it calls for — claims and heartbeats on the remote, workers in orca worktrees, comments, labels and blocked-by edges on issues, status boards, the landing turn's worker. Between ticks it sleeps; a worker's `wake` ends the sleep, and a tick that left the next one a worker to start is followed with no sleep. Before each tick it asks GitHub again whether its tracker is public, once the check that found it private is an hour old — or at once, of a tracker said anew. Public, it ends before the tick, as its startup probe would refuse it, and is broken: it holds no move onto a newer release back, and each start of it again is refused until the tracker is private again or another is said (`--tracker`), which `afkconvoy doctor` names with that line.

A fleet never puts its host to sleep: an idle host stays awake until something else puts it to sleep. Idle is holding no claim — on an issue, a patrol's run, a cut — and an empty frontier; an escalated issue and a patrol not yet due leave a fleet idle. A cut starts when idle or landed work reaches its age bound, and keeps it from idle while it runs. Every fleet on this host is a fleet instance of its one host runner, which holds how each stands, as each says it as every tick of its begins and ends: in a tick, or between two, at work or quiet. No fleet instance asks another.

ADMISSION. A tick starts no new worker while its host is busy: its 1-minute load average, less the estimated share of running exclusive queue process trees, past 200% of its cores. The estimate apportions total load by the trees’ share of observed CPU; if the queue or process table cannot be read, it subtracts nothing. `doctor` says total, queue and other load. Its estimate uses current CPU and may differ from the minute average; memory and disk include everything. It also holds if, less than 10% of its memory free (what the kernel counts as available: `sysctl kern.memorystatus_level`, or MemAvailable in /proc/meminfo), or less than 1% of the disk under the checkout free (`df`). The issue stays on the frontier for the next tick, and so do a stale claim and a due patrol. Workers already running are left alone, and so is the worker a declared verdict starts. The host says its own shares, each a whole percent, in its host settings: `--admit-load` (of its cores), `--admit-memory` and `--admit-disk` (free) — `--admit-load 400` lets the load reach four times the cores, and `--admit-disk 0` never holds for disk. A value that is no whole percent is refused, and `default` changes a share back. What cannot be read holds nothing back. `plan` and `doctor` say the thresholds in force and what the host reads.

When admission holds back work in a checkout for one reason — load, memory or disk — at every tick for ten minutes (`--admit-inspect-after` sets the host's span), doctor names a finding that starts the host's watch at its next look. The finding names the held work. Reasons that change start none. Memory pressure (macOS's level at least 2) held for the same span is its own finding, even while admission admits, and never makes a worker hot. Each episode ends after five minutes of recovery. The watch receives the condition, how long it held, admission's reading and shares, and host evidence: for disk, each fleet's worktrees and caches and the largest directories outside them; for load or memory, the fleet's worker process trees and, where `hostvitals` is on the host runner's `PATH` and `hostvitals now` answers, its whole-host findings first, then its rankings of processes and Apps and its remaining readings. Otherwise it receives the fleet's hottest processes outside those trees, with one line saying why hostvitals could not answer. The evidence names its source. hostvitals is optional and is never installed as a dependency; hot-worker sampling stays the fleet's own. It checks admission or pressure itself after acting. Resource readings and scheduled looks continue during a watch; new episodes queue for a later round, and an episode whose watch reported starts nothing again, including across a host runner move. A watch that ends without reporting leaves the episode eligible at the next look that still finds it.

WATCH. Every host watches itself. Where its host settings name an operator's directory — `--watch-dir ~/Developments/control`, known to orca — the watch runs there under that directory's own rules, with none of the stock instruction added. Otherwise the stock watch runs in the fleet's own directory, `stock-watch/` under the cache directory (`afkconvoy help cache` says where it is), made, kept and registered with orca when it starts. Its short instruction carried in the binary says: find what holds the fleets' work back, fix it with the watch's authority, check that the work goes on and say what it did. Every watch leaves spending money or needing an account, a credential or a person's hands; what cannot be undone; taking a secret or private data off the host; and changing what a product is for or who it is for with a human, acts on none and lists each in its command report, which `doctor` shows as `report`. Unplaced host kit models are low priority: a watch started for other work takes them too, and they alone start a watch at most once a day, across runner restarts. The watch researches them with its web tools and places them through `afkconvoy kit --research <file>` (`afkconvoy kit --help` gives the checked file format). Its host runner takes a look every {const:WATCH_EVERY} seconds (`--watch-every`): `afkconvoy doctor --all --since <until of its last look>`, the fleet's own code, which runs no model, keeping `until` itself. A look that holds something an operator would act on — an issue waiting on a human, or a finding that is not `info` and not of the watch itself: a fleet instance not alive or broken, a failed tick or step, a worker whose health observation is overdue among them — starts one watch. Each condition is one finding of the look, and the watch acts on it by its `kind` and `watch_id`, never its words: a finding said in other words is the same episode. Doctor finds an observation overdue only past a tick's chance to make it: past its time by more than the sleep and the time of the last tick of the fleet instance whose claim holds the worker's worktree, as that instance's `doctor.fleet.last_tick` says them (`sleep`, `took`), or the longest any instance on the host says, for a worktree no claim holds. A fleet that stopped ticking, or a tick that hangs, leaves it missed past that grace. Its episodes stay covered while it runs and after it reports; an episode lasts while each look still holds it. An unreported watch leaves its episodes and daily research reservation eligible at the next look that still finds the work, including after a restart. A watch retains one resident interactive Agent session in the watch directory across independent watch rounds. Each round has its own ID, delivery receipt, report and execution hour, excluding queueing, standby and earlier rounds. It is given its brief as a file in the cache directory — what the look answered, why it started, and what each earlier watch on the same items did — with authority over the fleets of this host and an hour, its continuations together. It runs on the harness this host last started a worker on that took its brief, then the host kit's order, never one paused or with a fault on record, started trusted with prompts skipped, interactively in its terminal titled "afkconvoy watch", with the brief as its first message, as a worker is. A human may watch it and type to it. Its last step is the exact `afkconvoy watched --watch <id> --text-file <report.json>` line at the end of its brief (`afkconvoy watched --help`): the report records what held work back, each action, what it verified, each human question and answer line, and what remains pending. Reporting completes only that round and leaves the healthy Agent and Terminal standing by for the next message. Standby without pending work sends no prompt and runs no model. Only an exit, proven harness or context failure, or explicit reset replaces the owned executor, after confirming it ended. A watch ended without that command says it did not report. Printed output is no record. Worker health observes its own terminal; a stopped watch with no report is nudged. A watch whose harness gives out — a usage limit, capacity stop, refusal or failed exit — is ended and continued on the next harness that can start it, with the same look and terminal evidence of its work so far, one session at a time; the fault is recorded (`host_kit.worker_health`), and that harness is skipped until its hourly probe answers. Looks continue while a round runs. The host runner durably queues new episodes and merges repeated observations of queued, executing and reported episodes. Before the next round, a fresh look removes recovered conditions. Delivery waits for the previous report and the same Terminal's input readiness; busy input is left untouched. An observed recovery and return is a new episode, even during an older round. A host runner started again — after moving onto a newer release, say — keeps the watch it finds running. Its start, its end and each thing it did are lines of every tick log on the checkout list and of the host runner's log; `doctor` says the watch running and the last one, as `host.watch`, and a critical finding where no harness could start or continue it. `--watch-dir default` returns to the stock watch; a running watch and its continuations keep their starting directory and rules. `doctor.host.watch.directory` says the directory the next watch runs in, and `stock` whether it is the stock one, even before a watch starts; each running or last watch says its own directory and `stock` too.

In a repo worked by several hosts, each human wait is answered by one host's watch. Before queueing it the host runner takes a claim on that repo's remote; the other hosts leave it alone. Its holder keeps a heartbeat published while the wait stands, even with no worker claim. Once the heartbeat is past the lease, {const:LEASE_MINUTES} minutes, another host's next look takes the wait over. The holder's next look removes the claim when the issue no longer waits on a human. Only the holding host carries it into a round and its report outboxes. Each sending through `--notify-command` and each asking-session question re-reads the claim immediately before sending; a host that lost it drops the outbox item and logs the holder's name. Host conditions remain their own host's.

ASKING SESSION. Every human entry of a recorded watch report becomes a durable question in a second resident interactive Agent and Terminal beside the watch, in the watch's directory, titled "afkconvoy asks". It starts when the first question is pending and does no watch's work: it runs no watch round, takes no look and reports nothing through `afkconvoy watched`. What the operator tells it in the session (a host fix or install, a ruling, a hand-back, a ticket) it carries out at once, as the operator acting, within the watch's bounds. Its questions use the same episode identities as `--notify-command`: a human wait's `waiting_on_human` identity, or a host condition's finding identity, once per episode across reworded reports, host runner restarts and upgrades. One question is delivered at a time, only when input is ready, with ADR-0040's request key and session-turn proof. Each delivery is marked as the host runner's and names the condition, issue link, watch question and answer line, and how many more wait. Recovery before delivery removes a question; recovery afterwards tells the session it is withdrawn. The session ends its asking turn with the question first, at most 180 characters, so Orca's phone push shows it and a tap opens this Terminal. It clarifies, investigates and consults with the operator, then acts with their authority through the same commands as a human. A settled question is reported by `afkconvoy asked` with the link to the operator's quoted words and their time on its issue; the session's own words establish no human intent. Only then is the next delivered. A question put aside by the operator stays pending and is listed on request. Standby without pending work calls no model; the resident is replaced only on exit, a proven harness fault, an unusable context or an explicit reset. `doctor.host.asking` and the panel show its state, current question and those waiting.

With `--notify-command <COMMAND>`, the host runner runs that shell command afresh for each human entry of a recorded report, with one JSON object on stdin: `host`, `watch` (the round id), `condition` (the identity from the round's look), `question`, `answer` (the answer line), `url` (the issue source URL, null for a host condition) and `panel` (the URL actually served, null if unavailable). The question and answer leave the host only through the command the operator chose; no channel is built in. With none named nothing is sent; `--notify-command default` removes it. A successful sending is kept beside the reports across runner restarts and upgrades. The same condition is sent once per observed episode, even when later reports reword its question and answer or omit it. A human wait uses the identity of `waiting_on_human` (its issue and source); a host condition uses its finding identity. Only observed recovery ends an episode. A failed command, or one past {const:NOTIFY_SECONDS} seconds, is tried again at the next look, on a separate sender that holds back no look, watch round or tick. {const:NOTIFY_FAILURES} consecutive failures for one entry are a doctor finding. `doctor.host.notify` says the command in force and the last sending: watch, condition, answer, command, start and end times, and outcome.

A repo nobody works is reported across hosts: every look reads the trackers its checkout list names for ready repo-labelled issues, including repos absent from that list. After more than an hour with no open blocker or claim, counted from the later of readiness and the last blocker closing, and with no remote heartbeat within the 1800-second lease, the host runner opens one tracker issue per repo carrying its repo and escalate labels. A tracker claim settles simultaneous looks; only the opening host raises the finding and keeps its watch episode until recovery. The report names the waiting issues, wait and newest heartbeat host and age, or none. A later claim or live heartbeat closes it and names the taking host. Read failures are findings, never evidence that nobody works a repo. The watch must ask which host should take it and must never add the repo to its own checkout list.

WATCH RECOVERY. The fixed Terminal name is "{const:WATCH_TITLE}". The host runner records a launch intent before creation and the exact Terminal handle and public pane identity before sending its brief. Normal checks use that exact ID first; a restarted or upgraded runner reuses the confirmed session, its input and unfinished progress. If the ID is missing or stale, it queries Orca's public Terminal list and accepts only a unique candidate proven by its saved pane identity or its own startup acknowledgement. The name alone proves no ownership or uniqueness. Uncertain creation is never repeated. Each round's prompt intent saves a UUID request key before delivery; recovery replays the exact Terminal submission with Orca's supported --retry-request protocol and retains its receipt. An unproven or unsupported delivery pauses rather than sending fresh input; another harness or runner restart cannot bypass that pause. The existing command, model, directory rules and watch guard remain in force. A confirmed missing Terminal ends its watch after the marked executor is confirmed stopped; the next look may start eligible work as usual.

`doctor.host.watch.association` shows {title, status, session, watch, directory, terminal, identity, request, receipt, problem}: the fixed name, launch status, watch ID, starting directory, exact handle, public Orca metadata and recovery problem. status is unassociated, creating, ready, sending, running, idle, ended or uncertain; absent fields mean no association has been saved. The association record is `watch/terminal.json` under the cache directory; briefs, exit logs, startup acknowledgements and execution claims remain beside each watch. For an unverified or ambiguous candidate, inspect `orca terminal list --worktree path:<watch-directory> --json` and `orca terminal show --terminal <handle> --json`. Rename unrelated same-name sessions with `orca terminal rename --terminal <handle> --title <title> --json` and let the runner check again; those sessions stay open and unassociated. For an uncertain creation or first prompt, inspect the retained session and record a complete report with `afkconvoy watched --watch <id> --text-file <report.json>` (see its help). State what remains pending; this settles only the named round. The healthy executor stays; unresolved association still pauses further delivery. Keep its files and execution claim. A live Terminal, an idle Agent and printed output do not finish a watch; its explicit report does. The resident session is shown separately as host.watch.session with its id, Terminal, status (active, standby or unavailable) and current round. host.watch.pending lists queued episode/item pairs; host.watch.rounds lists retained rounds and their reports; each round includes delivery (its archived request, receipt and execution evidence) and deadline (null before execution); looked and until retain the look interval. An idle resident is never nudged for an absent report.

The watch operates for every repo on the checkout list (`afkconvoy checkouts`), bound or unbound. It may trust a worktree for a harness, restart, nudge, continue or switch a worker and change host settings. Once the cause of a fleet fault is gone it hands the escalated issue back with its current `doctor.waiting_on_human.answer` line, naming that repo's tracker, wakes the checkout after a `gh` change and checks that the next tick dispatches or continues it and its worker takes its brief. The watch answers escalations with `rule`, handing back, `send`, closing or changing blocked-by, using the same commands as a human and starting rework counts over. Both watch directories leave only the four human kinds above, listing each question and answer line in the watch's record; `doctor` lists what it did.

A lasting fix may be code in any listed repo, even one whose fleet cannot start workers. The watch makes it in a linked worktree of its own from that repo's configured base branch, named `{const:WATCH_CHECKOUT}` or `{const:WATCH_CHECKOUT}-<digits>`, on its own branch; never in another worker's or watch's checkout. It commits and pushes, opens the PR against that repo's base branch on no issue, then takes a turn with `afkconvoy land --watch` to merge, gate, record green and land. This reuses a watch's landing path and attribution, with no intake or flow label, and respects fleet locks. It records the repo and PR.

When the fix must reach an installed tool, the watch waits for landing and has that repo's fleet cut a version through its declared cut steps (`afkconvoy cut --help`): it wakes the checkout, reads `plan.cut` and fixes what holds back the next idle tick. A mechanically failed cut is continued with its report's `cut --from <step>` line once its cause is fixed. It checks publication and installation; for afkconvoy, it lets the host runner move onto the version itself, reading `doctor`'s `host.installed` and `checkouts`'s host runner version, never killing or restarting it. Then it repeats the check that showed the fault and checks that the work goes on. Its report records each fix, PR, cut, version and evidence, the four human kinds left alone and anything still pending; landing or installing alone proves no recovery. All fits within the watch's one hour: anything unfinished is recorded as pending.

It samples each worker every ten seconds. A hot worker becomes a doctor finding and starts the host's watch once per episode. Its look carries the worker, issue, worktree, terminal, condition, duration and the evidence taken down as it turned hot: process tree, five-second sample, resident memory, disk writes and largest directories. The watch takes no slot of concurrency. There is one watch per host, with one hour shared by its continuations.

Each fleet instance also keeps a log on this host, its tick log, in a file under the cache directory (runs/; `afkconvoy help cache` says where it is and what moves it): one line when it starts, one as each tick starts — with why, and the command whose wake started it — and one as it ends — what `tick` answers for it — and one of each event that reached it: a wake heard during a tick, the host waking, being told to stop. Every command that changes fleet state writes a line of its own to the same file as it acts (its `--help` says so under "What it logs", and [`afkconvoy help tick-log`](help:tick-log) says what the line holds). It keeps every line of the last {const:LOG_KEPT_HOURS} hours, and never fewer than {const:LOG_KEPT_LEAST} lines nor more than {const:LOG_KEPT_MOST}. A line is written whole as it happens, so killing the host runner loses none, and none a command appends while the fleet instance cuts the file back is lost. Nothing reads the log to decide anything; `afkconvoy doctor` reads it to say what the last ticks did, which steps failed, and the wakes, starts, stops and host wakes of late.

## What it answers

Before a first start: a checkout off the checkout list must name `--tracker <owner>/<repo>` or `--tracker default` (its own repo, which must be private). Missing answers refuse with a non-zero exit and write nothing: {"error", "asks", "see"}. Each `asks` item has `setting`, `question`, `default`, `recommend`, and `options`, each with `option` and `line`; `see` prints [`afkconvoy help first-start`](help:first-start). Settle these questions with the user before running the chosen lines. Existing checkouts and saved host settings run as before.

Once its checkout's fleet instance has ended its first tick, with exit 0:

```json
{"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj", "added": true,
 "instance": "me-h1-1a2b3c4d",
 "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
                 "panel": "http://127.0.0.1:7070/", "started": true, "replaced": null},
 "tick_log": "<cache>/runs/hkj-0123456789abcdef.jsonl",
 "first_tick": "tick 1: dispatched #12, #15 · holding 2/6 · frontier 4 · next in 30s",
 "concurrency": {"from": {"in_force": 3, "said_by": "default"},
                 "to": {"in_force": 6, "said_by": "checkout"}},
 "tracker": {"from": null, "to": "me/issues"},
 "harness": {"from": {"in_force": "claude-code", "said_by": "host_kit"},
             "to": {"in_force": "kimi", "said_by": "checkout"}},
 "settings": {"hot_cores": {"from": null, "to": 4.0}},
 "warnings": []}
```

- `repo`: the repo the checkout is of, as its remote `origin` names it.
- `added`: false when the checkout was on the list already. Where its fleet instance runs, the flags of this `run` that are its fleet instance's own change nothing, and the answer is that instance, with its own first tick.
- `host_runner`: the one that answered — its pid, its version, its log and where it serves the panel, null where it serves none — and whether this `run` started it. `replaced`: the host runner of an older release this `run` stopped to start it, as {"pid": 4100, "version": "0.5.13"}; null where it stopped none.
- `tick_log` and `first_tick`: the fleet instance's tick log, and the line that ended its first tick. `<cache>` stands for the cache directory, whole in the answer.
- `concurrency`: what this `run` changed of its checkout's concurrency, as {from, to}, each the one in force before and after, as `plan` says it: {in_force, said_by} — said_by `checkout` or `default`. null where it named none, or named the one in force.
- `tracker`: what this `run` changed of its checkout's tracker, as {from, to}, each the repo named, `<owner>/<repo>`, or null for the checkout's own. null where it named none, or named the one it has. A `run` that kept it with its checkout's entry before it started the host runner says it in an error the host runner answered too: the tracker stays said.
- `harness`: what this `run` changed of its checkout's harness, as {from, to}, each the one in force before and after, as `plan` says it: {in_force, said_by} — said_by `checkout` or `host_kit`. null where it named none, or named the one in force.
- `settings`: what this `run` changed of the host settings, each by its key — its flag's name, as `hot_cores` for `--hot-cores` — as {from, to}, in its flag's units; null is the default. {} where it named none, or named each as it was.
- `warnings`: what its fleet instance warned of until its first tick ended — the startup probe's warnings among them — also on stderr; empty where this `run` waited on no start.

Each fleet instance's lines — `run` prints none of them. Its tick log holds them, and `afkconvoy doctor` reads them back, but for the `start:` lines after the first — the claims its start took — which are lines of the host runner's log, after the checkout (`/Users/me/Developments/hkj: start: took #4, #7 stamped with this host`):

```
start: run as tester-h1-1a2b3c4d on h1, its workers started with `claude`
start: took #4, #7 stamped with this host        only when a start took or lost claims
start: took #4, the run of the patrol sweep stamped with this host
start: took #4 stamped with this host · swept 1 dead heartbeat(s) that held nothing
stop: holding no claim, it took its heartbeat with it
stop: it left its heartbeat with the claims it holds: the next start on this host takes them
tick 1: started (start)
tick 1: dispatched #12, #15 · holding 2/3 · frontier 4 · next in 30s
tick 2: started (timer)
woken by park #46 during tick 2: tick 3 follows
tick 2: nothing to do · holding 2/3 · frontier 4 · next in 60s
tick 3: advanced #12 → review · dispatched #12 · holding 2/3 · frontier 0 · next in 90s
tick 4: advanced #12 → land · holding 1/3 · frontier 0 · next now
tick 5: dispatched #12 · holding 2/3 · frontier 0 · next in 90s
tick 6: escalated #15 (workup: consult let it through) · holding 1/3 · frontier 0 · next now
tick 7: dispatched #71 · skipped #68 (blocked by #71) · holding 2/3 · frontier 0 · next in 90s
```

Every tick has a line as it starts, before it does anything, naming why: `timer` (its sleep ran out), `woken` (a wake ended the sleep, or was heard during the tick before: `woken by` the command that sent it, and what that acted on, where it named them), `start` (the fleet instance's first) or `follow-up` (the tick before left a worker to start, or could not reach GitHub, so it runs with no sleep). A wake heard during a tick is a line as it is heard — the first of each tick — and the next tick starts as that one ends: ticks never overlap. The host waking from a sleep is a line, as the fleet instance notices it — a rest that took a minute past its length — and being told to stop:

```
tick 4: started (woken by verdict #121)
tick 4: started (woken)                          a wake that named nothing
tick 5: started (follow-up)
host woke: after 3600s asleep
```

The host runner keeps two lines of its own in a fleet instance's tick log: as it breaks, and as it is back.

```
broken: it panicked: <what it said> · since 2026-10-24 09:12:00 UTC · started again at 2026-10-24 09:13:00 UTC, in 60s
back: broken since 2026-10-24 09:12:00 UTC, for 75s
```

A tick's line names what it did — lost, escalated, released, left, settled, ended, advanced, unlocked, reclaimed, dispatched, skipped (a frontier issue found blocked just before its claim, with what blocks it), closed meanwhile (a frontier issue its dispatch found closed since the frontier was read, which fails nothing: `#545 closed meanwhile`), nudged, waiting on its own background work (with since when: `#7 (since 2026-10-20 23:03 UTC, 12m ago)`), continued, restarted, asked for a turn again for (a landing claim whose last turn answered a wait, asked again with no harness started) — then, when admission held back a worker it would have started, which threshold did (`admission holds back new workers: the load 31.20 is past 28.00, 200% of its 14 cores`), then the claims held out of the concurrency, and the frontier left. The claims a fleet lock has it leave alone take no slot and are counted apart (`holding 1/3 · 2 left alone`), and so are those whose worker declared a verdict not acted on yet, or that are landing with their worker not changing the tree (`holding 3/3 · 1 taking no slot`), as are those past the bound — a start took them, or the checkout's concurrency was lowered under them (`holding 3/3 · 1 past the bound`). A step that failed is in the line as `FAILED <step> #<issue>: <why>`, and a tick that could not start as `tick N: FAILED: <why>`; the loop goes on either way. One cause that stopped several steps is said once: `FAILED 7 steps: <why>`.

`advanced #12 → review`: the tick read a verdict that left #12 at `review`. Where a role performs that stage the same tick starts its worker, and says so: `dispatched #12`. An issue advanced to no stage — its workflow ended, or it was parked — is named alone.

`escalated #15 (workup: consult let it through)`: the tick handed #15 to a human, whichever step did, with why in a few words — the stage first, where it was at one. It is named as nothing else. The comment on the issue says why in full and ends with the line that answers it; `afkconvoy doctor` lists it under `waiting_on_human`.

Between ticks a fleet instance sleeps {const:TICK_BUSY_SECONDS} seconds while it has work, or {const:TICK_IDLE_MINUTES} minutes once {const:TICK_IDLE_AFTER} ticks in a row were empty. Commands that change fleet state wake it sooner, as does `afkconvoy wake`. A fleet on this host that closes an issue or sees it closed wakes every other fleet on the same tracker whose open issues it blocked, across repos. The wake names the closed issue and the fleet instance that saw it, in the woken fleet's tick log. Even with another blocker still open the fleet is woken; that next tick reads GitHub and takes up only what is unblocked. A change made on GitHub, or by another host's fleet, is read at the next tick: no webhook reaches the host. Where the tick acts on such a change, its line says what changed since the tick before, and who changed it where that is known:

```
tick 8: lost #46 to instance beta-1a2b3c4d on host h2 (takeover) · holding 1/3 · frontier 0 · next in 90s
tick 9: dispatched #45 (labelled ready-for-agent since tick 8) · holding 2/3 · frontier 0 · next in 90s
tick 9: released #231 (the issue was deleted) · holding 1/3 · frontier 0 · next in 90s
tick 9: left #97 (closed by @alice) · holding 1/3 · frontier 0 · next now
tick 9: skipped #320 (blocked by #379, added since tick 8) · holding 1/3 · frontier 0 · next in 90s
```

`lost #46`: a claim this instance held is another's now — taken by a takeover, or `(stale: past the lease)`, as a stale claim, this instance's heartbeat being past the lease. On another host, the tick closes its local worker's terminal and adds `stopped its worker`, keeps the worktree and pushes nothing of its branch. A failed close is an error under `stop_taken`, retried next tick. `released #231`: its issue was deleted, `(the issue was transferred to acme/other#12)`, where GitHub says it moved, or `(there is no such issue)`, a 404 with no move. `left #97`: a claim with no PR whose issue was closed, by whoever GitHub says closed it (`closed on GitHub` where it says no one), is settled — nothing of the fleet's landed. `labelled …` and `added since …` say what the tick before saw otherwise. A change the tick does nothing about is in no line; nor is one made before the fleet instance's first tick, which has no tick before it, nor anything of the kind in what `afkconvoy tick` answers.

`next now` in place of `next in <n>s`: the tick settled an issue, or advanced one without starting what follows, and left a slot free, so what follows — a consult, the landing of a PR, an issue the landing unblocked — is the next tick's to start, and that tick starts with no sleep.

So does the tick after one that failed on a read GitHub could not be reached for (`GitHub could not be reached: …`): nothing says the next read fails too. At most {const:BACK_TO_BACK} ticks run back to back; then it sleeps, whatever the last one left.

The host runner serves the panel from its start until it ends, on the panel's port of the host settings — {const:PANEL_PORT} where they say none, or any free port where that one is taken: every fleet of the host as one web page — the host first, then what waits for a human across every repo, then each repo with its workflows as lines of their stages and its landing turns — brought up to date in the browser after every tick of any fleet instance, by itself. `afkconvoy panel --help` says what the page shows, and `afkconvoy panel --open` where it is served. Its address is a line of the host runner's log:

```
panel: http://127.0.0.1:7070/
```

`--panel <port>` sets that port, a host setting, which the host runner reads as it starts: one that runs already serves the panel where it does, and the next one — after `afkconvoy stop --all`, or an upgrade — on the port set.

Errors, before the first tick, as {"error": …} with a non-zero exit: the host settings cannot be read or written; a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run); the repo fails the startup probe; its tracker — the one `--tracker` names, or the one the checkout says — is public (the error names it, with the line that says a private one); the config cannot be read; the host kit cannot be read; the worker launch command of a harness the fleet could be asked for resolves to nothing, or does not take the flags the fleet adds for its role or worktree trust; another checkout of the repo is on the checkout list; the checkout has no remote `origin`; the login shell did not answer within 20 seconds; the host runner is of a newer release than this `run` (the error names both versions, and nothing was changed); the host runner did not say its version within 10 seconds; the host runner of an older release, told to stop, still held the host after 30 seconds; no host runner answered within 35 seconds; the host runner ended before it answered, or is stopping; the checkout's fleet instance is stopping, told by `afkconvoy stop`; no first tick within 30 minutes; the checkout list cannot be written. Each error a host runner gave says which, as `host_runner`.

## What trips you up

- Stop the host runner with `afkconvoy stop --all`, or SIGTERM to its pid, `host_runner.pid` in any `run`'s answer: each fleet instance finishes the step it is in, makes none of the steps left in its tick, and ends — within a second when it sleeps — and the host runner exits once every one has, keeping the checkout list: the next `run` starts every checkout on it. No claim is released on the way out and no worker is stopped: the workers work on, and the next start on this host takes the claims back. Holding no claim, a fleet instance removes its heartbeat from the remote; holding one, it leaves it, and that next start removes it. A second signal ends the host runner at once, as SIGKILL does, with no goodbye: what that leaves, a later start on this host removes once it is past the lease. A patrol's run it takes back is ended by its first tick once its worker is gone or has stopped.
- Ctrl-C while `run` waits ends that `run` alone: the host runner is in a process group of its own, and goes on with the checkout.
- Of a `run` whose checkout's fleet instance runs already, the flags that are that instance's own — `--instance`, `--host`, `--worker-no-proxy` — change nothing: `afkconvoy stop` it, and run again with those you want. `afkconvoy stop --help` says how to stop one checkout and how to stop them all. The host settings it names change all the same, for that instance and every other on the host, and so do its checkout's `--concurrency` and `--harness`, for that instance alone, from its next tick. The running instance is kept.
- A checkout's concurrency is that checkout's, on this host: another host works the repo at its own, and a checkout that says none holds {const:CONCURRENCY}. How many workers a repo gets at once is what the host can carry — its cores, its memory, its logins — and no fact of the repo, so the repo config says none: a `concurrency` it still names is read no more. Raising it for an afternoon, or lowering it while the host is needed for something else, is one `run`, and no commit.
- A checkout's harness is that checkout's, on this host, like its concurrency. `--harness kimi` names it; `--harness default` follows the host kit again. The next tick reads it and does not change a running instance. Workers start on the named harness with the host kit's command and that harness's model order; a `model:` label selects only a model of that harness. A role that holds no limits for it starts no worker, and no other harness is started in its place. Where that harness cannot start — `doctor` says `account fault`, or `model fault` once no placed model is left, or that its launch command cannot start — no worker starts on the next harness. A worker already at work on another harness is continued onto the named one, and no second writer starts until the old one has ended. A worker already on it is left. Clearing with `default` continues a worker that is not on the host kit's first eligible harness, by that same rule. `switch` does not change the name and does not continue that checkout's workers onto the kit's new first. The host's watch is not a worker of the checkout. A word that is not a harness, or a harness the host kit does not hold, is an error and writes nothing.
- Without `--instance` each start mints a new instance id, named for this host: the answer says it as `instance`, and `afkconvoy doctor` as `fleet.instance`, for `plan --instance` and the commands that make one transition by hand.
- A host setting is the host's, not a `run`'s or a repo's: `--hot-cores 4` named in one checkout holds for every fleet instance on the host until a `run` names it again, and `--hot-cores default` changes it back. The variables that stand for the flags are read from the environment of a `run` and of nothing else — not of the host runner, nor of `tick`, `plan` or `doctor` — so one exported in your shell's rc files is named again by every `run` started from that shell.
- The panel is served to this host only, and shows a tick, not this second: between two ticks it changes nothing but its countdown. It costs GitHub no read of its own.
- Every worker launch command comes from the host kit: run `afkconvoy kit` to write it again. It is $XDG_CONFIG_HOME/afkconvoy/kit.json, or $HOME/.config/afkconvoy/kit.json, kept on this host and never pushed. `--worker-command` is gone. On a host with no host kit, `run` writes one as `afkconvoy kit` does, at a terminal or not, asking nothing: it says on stderr where the file is, what it holds and what was left out, and goes on to its first tick. At each worker launch the latest kit is read; a known harness with no entry or command is discovered in the login shell and its checked stock command merged into the kit without model probes. Explicit commands are respected, including errors. A checkout that names none starts on the host kit's first eligible harness. One that names a harness starts on that harness; the host kit still orders that harness's models. A start checks the named harness alone, and does not refuse the checkout because some other harness cannot start.
- The fleet starts every worker launch command under `nice`, with a `TMPDIR` of the worktree's own, adding its harness's model flag where the worker runs a model — and, in a bound repo, what puts the worker in its role, under the guard: [`afkconvoy help worker-launch`](help:worker-launch) says, harness by harness, the line each worker is started with, what each part is for and what loads from where.
- In a bound repo whose checkout names no harness, before any worker is started, the command of every harness the fleet could be asked for is checked: the first harness in the host kit's order, each harness of each role's range (`afkconvoy help roster`). One that cannot be started refuses the start, named with its harness and the range that calls for it, and no worker at all is started: no harness is ever started in another's place. A range leaves out each harness the role holds no limits for while it holds limits for another, so no worker runs a harness it holds no limits for; `plan` and `check` name each harness left out.
- A role that holds no limits for the harness in force starts no worker of that role, and no other harness is started in its place. `plan` and `doctor` name the checkout, the harness and the missing limits. Where that harness cannot start — `doctor` says `account fault`, or `model fault` once no placed model is left, or that its launch command cannot start — no worker of this checkout starts on the next harness; `plan` and `doctor` say why. A role whose range on this host is empty still refuses no other role: no worker of it is started, its stages wait, and every other stage goes on.

## Worker proxy bypass

A start on this host reaches some hosts past the proxy with `--worker-no-proxy [HARNESS=]HOSTS`, never a key of the repo's config: [`afkconvoy help worker-proxy`](help:worker-proxy) says what each worker is given."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expired_move_names_the_ticks_and_running_command_that_held_it() {
        let installed = Installed {
            path: PathBuf::from("/bin/afkconvoy"),
            version: Some("999.0.0".into()),
            from: VERSION.into(),
            since: 1000,
            wait_expired: true,
            attempt_since: 1000,
            held_back: vec![
                "/checkout is in a tick".into(),
                "1 exclusive command(s) running".into(),
            ],
            failed: None,
        };
        let finding = serde_json::to_value(not_moved(&installed).unwrap()).unwrap();
        assert!(finding["what"].as_str().unwrap().contains("exceeded 600s"), "{finding}");
        assert_eq!(finding["evidence"], "/checkout is in a tick; 1 exclusive command(s) running");
        assert_eq!(finding["severity"], "warning");
    }

    #[test]
    fn a_move_waits_at_most_ten_minutes_then_resumes_the_old_runner() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared::default();
        *shared.installed() = Some(Installed {
            path: "/bin/afkconvoy".into(),
            version: Some("999.0.0".into()),
            from: VERSION.into(),
            since: 1000,
            wait_expired: false,
            attempt_since: 1000,
            held_back: vec![],
            failed: None,
        });
        let seen = Arc::new(Seen::default());
        seen.stage().instance = Some("alpha".into());
        let stop = Stop::never();
        let runner = Runner {
            list: List::at(dir.path().join("checkouts.json")),
            log: Arc::default(),
            stop: stop.clone(),
            moving: AtomicBool::new(false),
            panel: HostPanel::new(shared.clone()),
            shared: shared.clone(),
            held: Mutex::new(BTreeMap::from([(
                PathBuf::from("/checkout"),
                Held {
                    repo: "repo".into(),
                    host: "h1".into(),
                    seen,
                    stop: stop.clone(),
                    reach: Reach::default(),
                },
            )])),
        };
        let host = crate::standings::Standing::open("alpha", &shared.standings);
        host.begins();
        assert!(runner.stopped_for_move_at(1599).is_none());
        assert!(!shared.installed().as_ref().unwrap().wait_expired);
        assert!(runner.stopped_for_move_at(1600).is_none());
        let installed = shared.installed().clone().unwrap();
        assert!(installed.wait_expired);
        assert_eq!(installed.held_back, ["/checkout is in a tick"]);
        assert!(not_moved(&installed).is_some());
        assert!(!runner.moved() && !stop.told());
        host.ticked(true);
        assert!(host.begins_unless_stopped(&stop), "new ticks resume after expiry");
        let watched = Watched {
            path: None,
            seen: None,
            looked: Instant::now(),
            installed: Arc::clone(&shared.installed),
            published: Arc::default(),
        };
        assert!(!watched.moves_at(1600), "the old runner gets time to work");
        assert!(watched.moves_at(1601), "the same installed file is tried again");
        host.ticked(true);
        assert!(runner.stopped_for_move_at(1601).is_some());
        assert!(runner.moved() && stop.told(), "it moves once the long tick ended");
    }

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
    fn a_login_shell_that_gives_the_same_directories_is_given_no_xdg_variable() {
        let named = |env: Vec<(OsString, OsString)>| -> Vec<(String, String)> {
            env.into_iter()
                .map(|(name, value)| (name.into_string().unwrap(), value.into_string().unwrap()))
                .collect()
        };
        let cache = Path::new("/Users/me/.cache/afkconvoy");
        let config = Path::new("/Users/me/.config");
        // no XDG_CONFIG_HOME in the login shell: none in the host runner,
        // so none in a gate it runs (sunfmin/control#765)
        let login = vec![("PATH".into(), "/usr/bin".into()), ("HOME".into(), "/Users/me".into())];
        assert_eq!(named(pinned(login.clone(), cache, config)), named(login));
        // the login shell's own, where they are this run's, stay as it says them
        let login = vec![
            ("XDG_CONFIG_HOME".into(), "/conf".into()),
            ("HOME".into(), "/Users/me".into()),
            ("XDG_CACHE_HOME".into(), "/c".into()),
        ];
        assert_eq!(
            named(pinned(login.clone(), Path::new("/c/afkconvoy"), Path::new("/conf"))),
            named(login)
        );
        // a run whose config is elsewhere than the login shell's still pins it
        let login = vec![("HOME".into(), "/Users/me".into())];
        assert_eq!(
            named(pinned(login, cache, Path::new("/conf"))),
            [
                ("HOME".to_string(), "/Users/me".to_string()),
                ("XDG_CONFIG_HOME".to_string(), "/conf".to_string())
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
        assert_eq!(last_lines(&file, 0, 2), "");
        std::fs::write(&file, "a\nb\nc\n").unwrap();
        assert_eq!(last_lines(&file, 0, 2), "b\nc");
        assert_eq!(last_lines(&file, 0, 5), "a\nb\nc");
        // what was written past a point, and all of a log that rolled since
        assert_eq!(last_lines(&file, 2, 5), "b\nc");
        assert_eq!(last_lines(&file, 99, 5), "a\nb\nc");
    }
}

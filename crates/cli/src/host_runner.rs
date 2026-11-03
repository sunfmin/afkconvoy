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
    let env = launch::login_environment()
        .context("the host runner starts with the environment of a fresh login shell")?;
    let kit = kit::path()?;
    let config = kit.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
    let env = pinned(env, &dir, config);
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

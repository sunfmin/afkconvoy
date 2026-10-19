//! The wake's two ends: the socket `run` listens on, and
//! `afkconvoy wake`, which a worker runs to end that sleep early.
//!
//! The socket is a file in the repo's git dir — the one place every worktree
//! of a repo shares on a host — and one datagram is all that crosses it to
//! the `run`: what sent it, in words (`park #46`), or nothing. The `run`
//! answers a wake sent from a socket with a name (`Answer`), as it hears it.

use std::io::ErrorKind;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use afkconvoy_core::wake::{Answer, SIGNAL, SOCKET};
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::git::Repo;

/// The directory a repo's wake socket is in: where a `run` keeps every socket
/// it listens on.
pub(crate) fn home(repo: &Repo) -> Result<PathBuf> {
    Ok(repo.common_dir()?.to_path_buf())
}

/// Runs `with` on the socket's name with `dir` as the working directory, and
/// goes back: a socket's path may only be some hundred bytes long, a git
/// dir's any length, so the socket is only ever named from inside its
/// directory.
fn inside<T>(dir: &Path, with: impl FnOnce(&str) -> std::io::Result<T>) -> Result<T> {
    named_inside(dir, SOCKET, with)
}

/// The same, for a socket called `name`. It moves the whole process: only a
/// process with one thread at work calls it — a `run` before its first tick.
pub(crate) fn named_inside<T>(
    dir: &Path,
    name: &str,
    with: impl FnOnce(&str) -> std::io::Result<T>,
) -> Result<T> {
    let back = std::env::current_dir().context("this process has no working directory")?;
    std::env::set_current_dir(dir).with_context(|| format!("cannot enter {}", dir.display()))?;
    let made = with(name);
    std::env::set_current_dir(&back).with_context(|| format!("cannot go back to {back:?}"))?;
    Ok(made?)
}

/// How long the thread that hears wakes goes without seeing that its
/// listener was dropped.
const HEARS_WITHIN: Duration = Duration::from_millis(100);

/// What a `run` is told of a wake heard during tick `n`: the first of that
/// tick's, as it is heard, with what it named.
pub type During = Box<dyn Fn(u64, Option<String>) + Send>;

/// The most bytes of what a wake names that are heard.
const NAMED_UP_TO: usize = 256;

/// The listening end, held by `run` for as long as it runs: a thread of its
/// own hears every wake as it arrives, during a tick as between ticks. One
/// that could not be opened hears nothing: every wake is then lost, which
/// costs only the waits they would have saved.
pub struct Listener {
    heard: Option<Arc<Heard>>,
    thread: Option<JoinHandle<()>>,
}

/// What the hearing thread and the `run` share.
struct Heard {
    state: Mutex<Wakes>,
    rung: Condvar,
}

#[derive(Default)]
struct Wakes {
    /// A wake was heard since the last was forgotten, or ended a rest.
    waiting: bool,
    /// What the first of those that named something named.
    by: Option<String>,
    /// The tick running, if one is.
    ticking: Option<u64>,
    /// Whether a wake was heard during it, and told.
    told: bool,
    /// The last tick to have started: none before the first.
    last: u64,
    /// The listener was dropped: the thread ends.
    closed: bool,
}

impl Heard {
    fn wakes(&self) -> MutexGuard<'_, Wakes> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Listener {
    /// Listens for the wakes of the repo `repo` is a checkout of, telling
    /// `during` of the first heard during each tick. A `run` already
    /// listening there stops hearing them: the newest one is woken.
    pub fn open(repo: &Repo, during: During) -> Listener {
        match home(repo).and_then(|dir| Listener::at(&dir, during)) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!(
                    "warning: this run cannot be woken, and waits out every sleep: {error:#}"
                );
                Listener { heard: None, thread: None }
            }
        }
    }

    fn at(dir: &Path, during: During) -> Result<Listener> {
        let socket = inside(dir, |name| {
            // What a `run` that ended left: nobody listens on it any more.
            let _ = std::fs::remove_file(name);
            UnixDatagram::bind(name)
        })?;
        socket.set_read_timeout(Some(HEARS_WITHIN))?;
        let heard = Arc::new(Heard { state: Mutex::default(), rung: Condvar::new() });
        let hearing = Arc::clone(&heard);
        let thread = std::thread::spawn(move || hear(&socket, &hearing, &during));
        Ok(Listener { heard: Some(heard), thread: Some(thread) })
    }

    /// Forgets every wake heard so far, as tick `count` is about to start.
    /// They carry nothing, so there is nothing to keep: the tick reads GitHub
    /// itself. One heard from now until `ended` is told (`During`).
    pub fn forget(&self, count: u64) {
        let Some(heard) = &self.heard else { return };
        let mut wakes = heard.wakes();
        (wakes.waiting, wakes.by, wakes.ticking, wakes.told, wakes.last) =
            (false, None, Some(count), false, count);
    }

    /// The tick `forget` named ended: a wake heard from now is told by the
    /// rest it ends, not as one during a tick.
    pub fn ended(&self) {
        let Some(heard) = &self.heard else { return };
        heard.wakes().ticking = None;
    }

    /// Sleeps for `sleep`, or until a wake — one that arrived before the
    /// sleep began ends it at once → the wake that ended it, if one did,
    /// which is used up.
    pub fn rest(&self, sleep: Duration) -> Option<Woken> {
        let Some(heard) = &self.heard else {
            std::thread::sleep(sleep);
            return None;
        };
        let wakes = heard.wakes();
        let (mut wakes, _) = heard
            .rung
            .wait_timeout_while(wakes, sleep, |wakes| !wakes.waiting)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let by = wakes.by.take();
        std::mem::take(&mut wakes.waiting).then_some(Woken { by })
    }
}

/// A wake that ended a rest.
#[derive(Debug, PartialEq)]
pub struct Woken {
    /// What the first wake heard since the last was used up named, where one
    /// named something.
    pub by: Option<String>,
}

impl Drop for Listener {
    /// Stops hearing: the socket is closed as the thread ends.
    fn drop(&mut self) {
        if let Some(heard) = &self.heard {
            heard.wakes().closed = true;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The hearing thread: every wake on `socket` is kept in `heard`, and the
/// first of a tick told to `during` as it is heard — under the lock, so the
/// tick's end is said after it. A wake from a socket with a name is answered
/// there, at once: what the `run` will do of it.
fn hear(socket: &UnixDatagram, heard: &Heard, during: &During) {
    let mut named = [0; NAMED_UP_TO];
    loop {
        let got = socket.recv_from(&mut named);
        let mut wakes = heard.wakes();
        if wakes.closed {
            return;
        }
        match got {
            Ok((length, from)) => {
                let by = String::from_utf8_lossy(&named[..length]).trim().to_string();
                let by = Some(by).filter(|by| !by.is_empty());
                wakes.waiting = true;
                if wakes.by.is_none() {
                    wakes.by.clone_from(&by);
                }
                let answer = Answer::of(wakes.ticking, wakes.last);
                if let Some(count) = wakes.ticking.filter(|_| !wakes.told) {
                    wakes.told = true;
                    during(count, by);
                }
                heard.rung.notify_all();
                drop(wakes);
                // An answer that cannot be sent costs the waker its wait.
                if let Some(to) = from.as_pathname() {
                    let _ = socket.send_to(answer.said().as_bytes(), to);
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            // Nothing more can be heard: every wake from now on is lost.
            Err(_) => return,
        }
    }
}

/// The sending end of a `run`'s own listener, for a thread of that `run` to end
/// its sleep with: made once the listener is open, and before any tick.
pub struct Waker {
    socket: UnixDatagram,
}

impl Waker {
    /// `None` where this run cannot be woken (`Listener::open` said so).
    pub fn of(repo: &Repo) -> Option<Waker> {
        let connected = |name: &str| {
            let socket = UnixDatagram::unbound()?;
            socket.connect(name)?;
            socket.set_nonblocking(true)?;
            Ok(socket)
        };
        let socket = home(repo).and_then(|dir| inside(&dir, connected)).ok()?;
        Some(Waker { socket })
    }

    /// A wake that cannot be sent is lost, as any is.
    pub fn wake(&self) {
        let _ = self.socket.send(SIGNAL);
    }
}

/// Whether a `run` listens for the wakes of the repo `repo` is a checkout of:
/// the one sign on this host that one is alive. Nothing is sent, so it is not
/// woken.
pub fn listening(repo: &Repo) -> bool {
    let reached =
        home(repo).and_then(|dir| inside(&dir, |name| UnixDatagram::unbound()?.connect(name)));
    reached.is_ok()
}

/// How long a wake waits for the `run`'s answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(3);

/// The socket `afkconvoy wake` hears its answer on: a file of its own in the
/// temp dir, named short enough for a socket, and gone with it.
struct Reply {
    socket: UnixDatagram,
    path: PathBuf,
}

impl Reply {
    fn bind() -> std::io::Result<Reply> {
        let name = format!("convoy-wake-{}.sock", std::process::id());
        let mut path = std::env::temp_dir().join(&name);
        // A socket's path may only be some hundred bytes long.
        if path.as_os_str().len() > 100 {
            path = Path::new("/tmp").join(&name);
        }
        let _ = std::fs::remove_file(&path);
        let socket = UnixDatagram::bind(&path)?;
        Ok(Reply { socket, path })
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Tells the `run` of this host for this repo to open its next tick now,
/// naming `by` — what woke it, `park #46` — for it to log: what every command
/// that acts does as it ends (`crate::acting`), `afkconvoy wake` among them.
/// It waits a few seconds at most (`ANSWER_WITHIN`) for the `run`'s answer:
/// the tick the wake starts, or the tick in progress it follows. With none
/// running, or none answering, it does nothing, and whatever goes wrong is not
/// an error: the fleet finds the outcome at its next tick anyway.
pub fn wake(repo: &Repo, by: &str) -> Result<Value> {
    let reply = Reply::bind();
    let sent = inside(&home(repo)?, |name| {
        let socket = match &reply {
            Ok(reply) => reply.socket.try_clone()?,
            Err(_) => UnixDatagram::unbound()?,
        };
        socket.set_nonblocking(true)?;
        let by = by.as_bytes();
        socket.send_to(&by[..by.len().min(NAMED_UP_TO)], name)
    });
    if let Err(error) = sent {
        let nobody = error.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(error.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused)
        });
        let why = if nobody {
            "no afkconvoy run is listening for this repo on this host: start one with              `afkconvoy run` in a checkout of it"
                .to_string()
        } else {
            format!("{error:#}: `afkconvoy doctor` says what is wrong with the fleet")
        };
        return Ok(json!({"woke": false, "detail": why}));
    }
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => {
            let why = format!(
                "the wake was sent, but no answer can be heard here ({error}): the tick log, \
                 or `afkconvoy doctor`, says what the run did of it"
            );
            return Ok(json!({"woke": false, "detail": why}));
        }
    };
    let heard = || {
        reply.socket.set_nonblocking(false).ok()?;
        reply.socket.set_read_timeout(Some(ANSWER_WITHIN)).ok()?;
        let mut bytes = [0; 64];
        let n = reply.socket.recv(&mut bytes).ok()?;
        Answer::read(&bytes[..n])
    };
    Ok(match heard() {
        Some(Answer::Starts(tick)) => json!({"woke": true, "tick": tick}),
        Some(Answer::Follows(tick)) => json!({"woke": true, "after_tick": tick}),
        None => {
            let why = format!(
                "the afkconvoy run listening for this repo on this host did not answer within \
                 {}s: `afkconvoy doctor` says why",
                ANSWER_WITHIN.as_secs()
            );
            json!({"woke": false, "detail": why})
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Instant;

    use super::*;

    /// Tests share one working directory, and `inside` moves it.
    pub(crate) static CWD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn listen(dir: &Path) -> Listener {
        Listener::at(dir, Box::new(|_, _| {})).unwrap()
    }

    fn send(dir: &Path) -> bool {
        send_named(dir, "")
    }

    fn send_named(dir: &Path, by: &str) -> bool {
        let sent = inside(dir, |name| UnixDatagram::unbound()?.send_to(by.as_bytes(), name));
        sent.is_ok()
    }

    #[test]
    fn a_wake_ends_the_rest_and_one_before_it_ends_it_at_once() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = listen(dir.path());
        // one that arrived during the tick
        assert!(send(dir.path()));
        let started = Instant::now();
        assert!(listener.rest(Duration::from_secs(30)).is_some());
        assert!(started.elapsed() < Duration::from_secs(5));
        // …and with none, the whole sleep is waited out
        let started = Instant::now();
        assert!(listener.rest(Duration::from_millis(150)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(listener.rest(Duration::ZERO).is_none());
    }

    #[test]
    fn the_first_wake_heard_during_a_tick_is_told_as_it_is_heard() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (told, during) = std::sync::mpsc::channel();
        let listener =
            Listener::at(dir.path(), Box::new(move |n, _| told.send(n).unwrap())).unwrap();
        // between ticks: the rest it ends says so, not this
        assert!(send(dir.path()));
        assert!(listener.rest(Duration::from_secs(30)).is_some());
        assert!(during.try_recv().is_err());
        listener.forget(7);
        assert!(send(dir.path()));
        assert_eq!(during.recv_timeout(Duration::from_secs(1)), Ok(7), "within a second");
        assert!(send(dir.path()), "and a second, as well");
        listener.ended();
        // …which is kept, not told twice, and ends the rest at once
        assert!(listener.rest(Duration::from_secs(30)).is_some());
        assert!(during.try_recv().is_err(), "one line a tick");
        listener.forget(8);
        listener.ended();
        assert!(send(dir.path()));
        assert!(listener.rest(Duration::from_secs(30)).is_some());
        assert!(during.try_recv().is_err(), "heard after the tick ended");
    }

    #[test]
    fn a_wake_says_what_sent_it_and_the_first_that_named_one_is_kept() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (told, during) = std::sync::mpsc::channel();
        let listener =
            Listener::at(dir.path(), Box::new(move |_, by| told.send(by).unwrap())).unwrap();
        assert!(send(dir.path()));
        assert!(send_named(dir.path(), "park #46"));
        assert!(send_named(dir.path(), "land #7"));
        std::thread::sleep(Duration::from_millis(300));
        let woken = listener.rest(Duration::from_secs(5));
        assert_eq!(woken, Some(Woken { by: Some("park #46".into()) }));
        // used up with the wake it ended
        assert!(send(dir.path()));
        assert_eq!(listener.rest(Duration::from_secs(5)), Some(Woken { by: None }));
        // during a tick: told as it is heard, and kept for the tick after it
        listener.forget(3);
        assert!(send_named(dir.path(), "verdict #121"));
        let told = during.recv_timeout(Duration::from_secs(1));
        assert_eq!(told, Ok(Some("verdict #121".to_string())));
        listener.ended();
        let woken = listener.rest(Duration::from_secs(5));
        assert_eq!(woken, Some(Woken { by: Some("verdict #121".into()) }));
    }

    #[test]
    fn forgotten_wakes_end_no_rest_however_many_there_were() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = listen(dir.path());
        for _ in 0..3 {
            assert!(send(dir.path()));
        }
        std::thread::sleep(Duration::from_millis(300));
        listener.forget(1);
        assert!(listener.rest(Duration::from_millis(50)).is_none());
        // three at once are one early tick, then the next turn forgets the rest
        for _ in 0..3 {
            assert!(send(dir.path()));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(listener.rest(Duration::from_secs(5)).is_some());
        listener.forget(2);
        assert!(listener.rest(Duration::ZERO).is_none());
    }

    #[test]
    fn a_socket_nobody_listens_on_takes_no_wake() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listening =
            |dir: &Path| inside(dir, |name| UnixDatagram::unbound()?.connect(name)).is_ok();
        assert!(!send(dir.path()), "no run ever listened here");
        assert!(!listening(dir.path()));
        let listener = listen(dir.path());
        assert!(listening(dir.path()));
        assert!(listener.rest(Duration::ZERO).is_none(), "asking whether one listens wakes nobody");
        drop(listener);
        assert!(!send(dir.path()), "the run that listened here ended");
        assert!(!listening(dir.path()));
        // the next run listens over what the last one left
        let listener = listen(dir.path());
        assert!(send(dir.path()));
        assert!(listener.rest(Duration::from_secs(5)).is_some());
    }

    #[test]
    fn a_git_dir_of_any_depth_can_be_listened_in() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("d".repeat(120)).join("e".repeat(120));
        std::fs::create_dir_all(&deep).unwrap();
        let back = std::env::current_dir().unwrap();
        let listener = listen(&deep);
        assert_eq!(std::env::current_dir().unwrap(), back, "the working directory is left alone");
        assert!(send(&deep));
        assert!(listener.rest(Duration::from_secs(5)).is_some());
    }
}

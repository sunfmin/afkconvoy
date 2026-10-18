//! The wake's two ends: the socket `run` listens on, and
//! `afkconvoy wake`, which a worker runs to end that sleep early.
//!
//! The socket is a file in the repo's git dir — the one place every worktree
//! of a repo shares on a host — and an empty datagram is all that crosses it.

use std::io::ErrorKind;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use afkconvoy_core::wake::{SIGNAL, SOCKET};
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
/// tick's, as it is heard.
pub type During = Box<dyn Fn(u64) + Send>;

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
    /// The tick running, if one is.
    ticking: Option<u64>,
    /// Whether a wake was heard during it, and told.
    told: bool,
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
        (wakes.waiting, wakes.ticking, wakes.told) = (false, Some(count), false);
    }

    /// The tick `forget` named ended: a wake heard from now is told by the
    /// rest it ends, not as one during a tick.
    pub fn ended(&self) {
        let Some(heard) = &self.heard else { return };
        heard.wakes().ticking = None;
    }

    /// Sleeps for `sleep`, or until a wake — one that arrived before the
    /// sleep began ends it at once → whether a wake ended it. The wake that
    /// ended it is used up.
    pub fn rest(&self, sleep: Duration) -> bool {
        let Some(heard) = &self.heard else {
            std::thread::sleep(sleep);
            return false;
        };
        let wakes = heard.wakes();
        let (mut wakes, _) = heard
            .rung
            .wait_timeout_while(wakes, sleep, |wakes| !wakes.waiting)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut wakes.waiting)
    }
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
/// tick's end is said after it.
fn hear(socket: &UnixDatagram, heard: &Heard, during: &During) {
    loop {
        let got = socket.recv(&mut [0; 1]);
        let mut wakes = heard.wakes();
        if wakes.closed {
            return;
        }
        match got {
            Ok(_) => {
                wakes.waiting = true;
                if let Some(count) = wakes.ticking.filter(|_| !wakes.told) {
                    wakes.told = true;
                    during(count);
                }
                heard.rung.notify_all();
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

/// `afkconvoy wake`: tells the `run` of this host for this repo to open its
/// next tick now. With none running it does nothing, and whatever goes wrong
/// sending it is not an error: the fleet finds the outcome at its next tick
/// anyway.
pub fn wake(repo: &Repo) -> Result<Value> {
    let sent = inside(&home(repo)?, |name| {
        let socket = UnixDatagram::unbound()?;
        socket.set_nonblocking(true)?;
        socket.send_to(SIGNAL, name)
    });
    Ok(match sent {
        Ok(_) => json!({"woke": true}),
        Err(error) => {
            let nobody = error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                matches!(error.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused)
            });
            let why = if nobody {
                "no afkconvoy run is listening for this repo on this host".to_string()
            } else {
                format!("{error:#}")
            };
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
        Listener::at(dir, Box::new(|_| {})).unwrap()
    }

    fn send(dir: &Path) -> bool {
        let sent = inside(dir, |name| UnixDatagram::unbound()?.send_to(SIGNAL, name));
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
        assert!(listener.rest(Duration::from_secs(30)));
        assert!(started.elapsed() < Duration::from_secs(5));
        // …and with none, the whole sleep is waited out
        let started = Instant::now();
        assert!(!listener.rest(Duration::from_millis(150)));
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(!listener.rest(Duration::ZERO));
    }

    #[test]
    fn the_first_wake_heard_during_a_tick_is_told_as_it_is_heard() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (told, during) = std::sync::mpsc::channel();
        let listener = Listener::at(dir.path(), Box::new(move |n| told.send(n).unwrap())).unwrap();
        // between ticks: the rest it ends says so, not this
        assert!(send(dir.path()));
        assert!(listener.rest(Duration::from_secs(30)));
        assert!(during.try_recv().is_err());
        listener.forget(7);
        assert!(send(dir.path()));
        assert_eq!(during.recv_timeout(Duration::from_secs(1)), Ok(7), "within a second");
        assert!(send(dir.path()), "and a second, as well");
        listener.ended();
        // …which is kept, not told twice, and ends the rest at once
        assert!(listener.rest(Duration::from_secs(30)));
        assert!(during.try_recv().is_err(), "one line a tick");
        listener.forget(8);
        listener.ended();
        assert!(send(dir.path()));
        assert!(listener.rest(Duration::from_secs(30)));
        assert!(during.try_recv().is_err(), "heard after the tick ended");
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
        assert!(!listener.rest(Duration::from_millis(50)));
        // three at once are one early tick, then the next turn forgets the rest
        for _ in 0..3 {
            assert!(send(dir.path()));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(listener.rest(Duration::from_secs(5)));
        listener.forget(2);
        assert!(!listener.rest(Duration::ZERO));
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
        assert!(!listener.rest(Duration::ZERO), "asking whether one listens wakes nobody");
        drop(listener);
        assert!(!send(dir.path()), "the run that listened here ended");
        assert!(!listening(dir.path()));
        // the next run listens over what the last one left
        let listener = listen(dir.path());
        assert!(send(dir.path()));
        assert!(listener.rest(Duration::from_secs(5)));
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
        assert!(listener.rest(Duration::from_secs(5)));
    }
}

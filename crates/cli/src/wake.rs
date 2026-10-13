//! The wake's two ends: the socket `run` listens on between ticks, and
//! `afkconvoy wake`, which a worker runs to end that sleep early.
//!
//! The socket is a file in the repo's git dir — the one place every worktree
//! of a repo shares on a host — and an empty datagram is all that crosses it.

use std::io::ErrorKind;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
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

/// The listening end, held by `run` for as long as it runs. One that could
/// not be opened hears nothing: every wake is then lost, which costs only the
/// waits they would have saved.
pub struct Listener {
    socket: Option<UnixDatagram>,
}

impl Listener {
    /// Listens for the wakes of the repo `repo` is a checkout of. A `run`
    /// already listening there stops hearing them: the newest one is woken.
    pub fn open(repo: &Repo) -> Listener {
        match home(repo).and_then(|dir| Listener::at(&dir)) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!(
                    "warning: this run cannot be woken, and waits out every sleep: {error:#}"
                );
                Listener { socket: None }
            }
        }
    }

    fn at(dir: &Path) -> Result<Listener> {
        let socket = inside(dir, |name| {
            // What a `run` that ended left: nobody listens on it any more.
            let _ = std::fs::remove_file(name);
            UnixDatagram::bind(name)
        })?;
        Ok(Listener { socket: Some(socket) })
    }

    /// Forgets every wake heard so far. They carry nothing, so there is
    /// nothing to keep: the tick about to start reads GitHub itself.
    pub fn forget(&self) {
        let Some(socket) = &self.socket else { return };
        if socket.set_nonblocking(true).is_ok() {
            while socket.recv(&mut [0; 1]).is_ok() {}
        }
    }

    /// Sleeps for `sleep`, or until a wake — one that arrived before the
    /// sleep began ends it at once → whether a wake ended it.
    pub fn rest(&self, sleep: Duration) -> bool {
        let Some(socket) = &self.socket else {
            std::thread::sleep(sleep);
            return false;
        };
        // No wait at all is not a timeout a socket can be given.
        let waits = if sleep.is_zero() {
            socket.set_nonblocking(true)
        } else {
            socket.set_nonblocking(false).and_then(|()| socket.set_read_timeout(Some(sleep)))
        };
        if waits.is_err() {
            std::thread::sleep(sleep);
            return false;
        }
        socket.recv(&mut [0; 1]).is_ok()
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

    fn send(dir: &Path) -> bool {
        let sent = inside(dir, |name| UnixDatagram::unbound()?.send_to(SIGNAL, name));
        sent.is_ok()
    }

    #[test]
    fn a_wake_ends_the_rest_and_one_before_it_ends_it_at_once() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = Listener::at(dir.path()).unwrap();
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
    fn forgotten_wakes_end_no_rest_however_many_there_were() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = Listener::at(dir.path()).unwrap();
        for _ in 0..3 {
            assert!(send(dir.path()));
        }
        listener.forget();
        assert!(!listener.rest(Duration::from_millis(50)));
        // three at once are one early tick, then the next turn forgets the rest
        for _ in 0..3 {
            assert!(send(dir.path()));
        }
        assert!(listener.rest(Duration::ZERO));
        listener.forget();
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
        let listener = Listener::at(dir.path()).unwrap();
        assert!(listening(dir.path()));
        assert!(!listener.rest(Duration::ZERO), "asking whether one listens wakes nobody");
        drop(listener);
        assert!(!send(dir.path()), "the run that listened here ended");
        assert!(!listening(dir.path()));
        // the next run listens over what the last one left
        let listener = Listener::at(dir.path()).unwrap();
        assert!(send(dir.path()));
        assert!(listener.rest(Duration::ZERO));
    }

    #[test]
    fn a_git_dir_of_any_depth_can_be_listened_in() {
        let _cwd = CWD.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("d".repeat(120)).join("e".repeat(120));
        std::fs::create_dir_all(&deep).unwrap();
        let back = std::env::current_dir().unwrap();
        let listener = Listener::at(&deep).unwrap();
        assert_eq!(std::env::current_dir().unwrap(), back, "the working directory is left alone");
        assert!(send(&deep));
        assert!(listener.rest(Duration::ZERO));
    }
}

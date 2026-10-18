//! Host sleep's effect half (docs/adr/0016): the socket each `run` listens on
//! to say how it stands, the asking of every other instance on the host, the
//! reading of the host — when it last woke, how long its keyboard and mouse
//! have gone untouched — and `pmset sleepnow`. What keeps the host awake is
//! the core's to decide (`afkconvoy_core::host_sleep::awake`).
//!
//! The sockets are in one directory of the cache, shared by every instance on
//! the host whatever its repo, one named for each instance. One line crosses
//! a socket each way: `standing`, or `tick` to have the instance tick first,
//! and the instance's `Standing` as JSON.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use afkconvoy_core::host_sleep::{
    ANSWERS_WITHIN, Asked, Awake, Host, Standing, TICKS_WITHIN, awake, check_at,
};
use anyhow::{Context, Result, bail};

use crate::copies::cache;
use crate::stop::Stop;
use crate::wake::{Waker, named_inside};

/// How a socket is named: for its instance, with this after it.
const SUFFIX: &str = ".sock";

/// Runs `with` on the socket `name` in `dir`: by its whole path where that
/// fits in a socket's address — under the cache directory of a home it does —
/// and else named from inside `dir` (`named_inside`), which moves the whole
/// process while it is named.
fn at<T>(dir: &Path, name: &str, with: impl Fn(&Path) -> std::io::Result<T>) -> Result<T> {
    match with(&dir.join(name)) {
        Err(error) if error.kind() == ErrorKind::InvalidInput => {
            named_inside(dir, name, |name| with(Path::new(name)))
        }
        made => Ok(made?),
    }
}

/// The directory every instance on this host listens in.
fn home() -> Result<PathBuf> {
    Ok(cache()?.join("instances"))
}

/// What this instance answers, and the askers waiting for its next tick.
struct Shared {
    standing: Standing,
    waiting: Vec<Sender<Standing>>,
}

/// One check, as far as it got.
pub enum Checked {
    /// Nothing keeps the host awake so far: every instance is asked to tick
    /// first, this one too — the next tick is now, and the check ends after it.
    TickNow,
    /// The host stays awake, for these.
    Awake(Vec<Awake>),
    /// The host was put to sleep.
    Slept,
    /// Nothing kept the host awake, and `pmset sleepnow` failed.
    Failed(String),
    /// The `run` was told to stop while the check waited: it ended there,
    /// with the host left awake.
    Stopped,
}

/// A `run`'s part in host sleep: it listens from `open` until it is dropped,
/// which removes its socket.
pub struct HostSleep {
    dir: PathBuf,
    name: String,
    /// Whether it listens there: what it removes as it is dropped.
    listening: bool,
    shared: Arc<Mutex<Shared>>,
    /// Whether this instance puts the host to sleep: not told `--no-host-sleep`.
    on: bool,
    /// How long, in seconds, every instance must have been idle.
    wait: u64,
    /// When this instance last checked.
    checked: Option<u64>,
    /// The other instances asked to tick first, while a check waits for this
    /// one's own tick.
    asking: Option<Vec<Asking>>,
}

/// One instance asked, its answer to come.
struct Asking {
    instance: String,
    answer: JoinHandle<Option<Standing>>,
}

impl HostSleep {
    /// Listens as `standing` says this instance is, in the directory every
    /// instance of this host listens in, and puts the host to sleep unless
    /// `standing` was told `--no-host-sleep`. Called before the first tick.
    /// One that cannot listen says so, and is seen by no other instance.
    pub fn open(standing: Standing, waker: Option<Waker>, wait: u64) -> HostSleep {
        let name = format!("{}{SUFFIX}", standing.instance);
        let on = !standing.no_host_sleep;
        let shared = Arc::new(Mutex::new(Shared { standing, waiting: vec![] }));
        let dir = home().unwrap_or_default();
        let listener = home().and_then(|dir| {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("cannot make {}", dir.display()))?;
            at(&dir, &name, |path| {
                // What a `run` of the same instance that ended left.
                let _ = std::fs::remove_file(path);
                UnixListener::bind(path)
            })
        });
        let listening = match listener {
            Ok(listener) => {
                let shared = shared.clone();
                std::thread::spawn(move || serve(listener, shared, waker));
                true
            }
            Err(error) => {
                eprintln!(
                    "warning: the other fleets on this host cannot see this run, which they do \
                     not wait for to put the host to sleep: {error:#}"
                );
                false
            }
        };
        HostSleep { dir, name, listening, shared, on, wait, checked: None, asking: None }
    }

    /// Says how this instance stands after a tick at `now`: idle or not, and
    /// whether it worked — `None` of a tick that failed whole, which is not
    /// idle. Every asker waiting for a tick is answered.
    pub fn ticked(&self, now: u64, idle: Option<(bool, bool)>) {
        let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (idle, worked) = idle.unwrap_or((false, false));
        let standing = &mut shared.standing;
        if standing.idle != idle {
            standing.since = now;
        }
        standing.idle = idle;
        if worked {
            standing.worked = Some(now);
        }
        let standing = standing.clone();
        for asker in std::mem::take(&mut shared.waiting) {
            let _ = asker.send(standing.clone());
        }
    }

    fn standing(&self) -> Standing {
        self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).standing.clone()
    }

    /// When this instance checks next; `None`: it does not, not being idle or
    /// having been told `--no-host-sleep`. A check underway is due now.
    pub fn due(&self) -> Option<u64> {
        let standing = self.standing();
        match self.asking {
            Some(_) => Some(0),
            None if self.on => {
                check_at(standing.idle.then_some(standing.since), self.checked, self.wait)
            }
            None => None,
        }
    }

    /// Checks at `now`, if a check is due → how far it got. A check asks
    /// every instance how it stands; if nothing keeps the host awake, it has
    /// each — this one too — tick first (`Checked::TickNow`), and ends with the
    /// call after this instance's tick, by putting the host to sleep or not.
    /// A `run` told to stop (`stop`) while a check waits is not held up by it,
    /// and does not put the host to sleep.
    pub fn check(&mut self, now: u64, clock: impl Fn() -> u64, stop: &Stop) -> Option<Checked> {
        if let Some(asking) = self.asking.take() {
            return Some(self.end(asking, clock, stop));
        }
        if self.due().is_none_or(|due| due > now) {
            return None;
        }
        self.checked = Some(now);
        let standing = self.standing();
        let mut asked = vec![Asked::Answered(standing.clone())];
        let answers = Duration::from_secs(ANSWERS_WITHIN);
        asked.extend(wait_for(self.ask("standing"), answers, || {}, || stop.told()));
        if stop.told() {
            return Some(Checked::Stopped);
        }
        let awake = awake(standing.worked, &read_host(), &asked, now, self.wait);
        if !awake.is_empty() {
            return Some(Checked::Awake(awake));
        }
        self.asking = Some(self.ask("tick"));
        Some(Checked::TickNow)
    }

    /// Ends a check after this instance ticked: waits for the others it asked
    /// to tick, answering any that asks it meanwhile with how it stands now.
    fn end(&mut self, asking: Vec<Asking>, clock: impl Fn() -> u64, stop: &Stop) -> Checked {
        let answering = || self.ticked_again();
        let ticks = Duration::from_secs(TICKS_WITHIN);
        let others = wait_for(asking, ticks, answering, || stop.told());
        if stop.told() {
            return Checked::Stopped;
        }
        let now = clock();
        self.checked = Some(now);
        let standing = self.standing();
        let mut asked = vec![Asked::Answered(standing.clone())];
        asked.extend(others);
        let awake = awake(standing.worked, &read_host(), &asked, now, self.wait);
        if !awake.is_empty() {
            return Checked::Awake(awake);
        }
        match sleep_now() {
            Ok(()) => Checked::Slept,
            Err(error) => Checked::Failed(format!("{error:#}")),
        }
    }

    /// Answers every asker waiting for a tick with how this instance stands:
    /// it ticked within the check it is making.
    fn ticked_again(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let standing = shared.standing.clone();
        for asker in std::mem::take(&mut shared.waiting) {
            let _ = asker.send(standing.clone());
        }
    }

    /// Asks every other instance listening here `what`, each in a thread of
    /// its own. A socket nobody listens on is removed, and counts for nothing.
    fn ask(&self, what: &str) -> Vec<Asking> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return vec![] };
        let mut asking = vec![];
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(instance) = name.strip_suffix(SUFFIX) else { continue };
            if name == self.name {
                continue;
            }
            let instance = instance.to_string();
            let connected = at(&self.dir, &name, |path| match UnixStream::connect(path) {
                Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                    // What a `run` killed with no goodbye left.
                    let _ = std::fs::remove_file(path);
                    Err(error)
                }
                connected => connected,
            });
            let Ok(stream) = connected else { continue };
            let what = what.to_string();
            let answer = std::thread::spawn(move || asked(stream, &what).ok());
            asking.push(Asking { instance, answer });
        }
        asking
    }
}

impl Drop for HostSleep {
    fn drop(&mut self) {
        if self.listening {
            let _ = std::fs::remove_file(self.dir.join(&self.name));
        }
    }
}

/// Waits up to `within` for every one of `asking` to answer, calling `meanwhile`
/// as it waits, and no longer once `given_up` → each as asking it came out:
/// one that has not answered by then is silent.
fn wait_for(
    asking: Vec<Asking>,
    within: Duration,
    meanwhile: impl Fn(),
    given_up: impl Fn() -> bool,
) -> Vec<Asked> {
    let started = Instant::now();
    while started.elapsed() < within
        && !given_up()
        && asking.iter().any(|one| !one.answer.is_finished())
    {
        meanwhile();
        std::thread::sleep(Duration::from_millis(20));
    }
    meanwhile();
    let came_out = |one: Asking| match one.answer.is_finished() {
        true => match one.answer.join().ok().flatten() {
            Some(standing) => Asked::Answered(standing),
            None => Asked::Silent { instance: one.instance },
        },
        false => Asked::Silent { instance: one.instance },
    };
    asking.into_iter().map(came_out).collect()
}

/// Asks the instance at the other end of `stream` `what` → its answer.
fn asked(mut stream: UnixStream, what: &str) -> Result<Standing> {
    let within = match what {
        "tick" => TICKS_WITHIN,
        _ => ANSWERS_WITHIN,
    };
    stream.set_read_timeout(Some(Duration::from_secs(within)))?;
    stream.set_write_timeout(Some(Duration::from_secs(ANSWERS_WITHIN)))?;
    writeln!(stream, "{what}")?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

/// Answers whoever asks on `listener`, each in a thread of its own: one
/// waiting for a tick holds up nobody else.
fn serve(listener: UnixListener, shared: Arc<Mutex<Shared>>, waker: Option<Waker>) {
    let waker = waker.map(Arc::new);
    for stream in listener.incoming().flatten() {
        let (shared, waker) = (shared.clone(), waker.clone());
        std::thread::spawn(move || answer(stream, &shared, waker.as_deref()));
    }
}

/// Answers one asker. Whatever goes wrong with it is its own loss.
fn answer(mut stream: UnixStream, shared: &Mutex<Shared>, waker: Option<&Waker>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(ANSWERS_WITHIN)))?;
    stream.set_write_timeout(Some(Duration::from_secs(ANSWERS_WITHIN)))?;
    let mut asked = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut asked)?;
    let lock = || shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let standing = match asked.trim() {
        "tick" => {
            let (send, ticked): (Sender<Standing>, Receiver<Standing>) = channel();
            lock().waiting.push(send);
            if let Some(waker) = waker {
                waker.wake();
            }
            ticked.recv_timeout(Duration::from_secs(TICKS_WITHIN))?
        }
        _ => lock().standing.clone(),
    };
    writeln!(stream, "{}", serde_json::to_string(&standing)?)?;
    Ok(())
}

/// What a check reads of the host.
fn read_host() -> Host {
    Host { woke: woke().ok(), untouched: untouched().ok() }
}

/// The output of `program` with `args`, which must succeed.
fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("`{program}` could not be run"))?;
    if !out.status.success() {
        bail!("`{program} {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// When the host last woke, in epoch seconds — zero for one that never slept
/// since it booted — as `sysctl kern.waketime` says: `{ sec = 1760000000,
/// usec = 5 } Sat Oct 18 …`.
fn woke() -> Result<u64> {
    let said = output("sysctl", &["-n", "kern.waketime"])?;
    waketime(&said).with_context(|| format!("`sysctl kern.waketime` said {said:?}"))
}

fn waketime(said: &str) -> Option<u64> {
    let after = said.split_once("sec =")?.1;
    after.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// How long, in seconds, the keyboard and mouse have gone untouched, as
/// `ioreg` says HIDIdleTime, in nanoseconds: `"HIDIdleTime" = 1234567890`.
fn untouched() -> Result<u64> {
    let said = output("ioreg", &["-c", "IOHIDSystem", "-d", "4"])?;
    hid_idle(&said).context("`ioreg -c IOHIDSystem` says no HIDIdleTime")
}

fn hid_idle(said: &str) -> Option<u64> {
    let line = said.lines().find(|line| line.contains("\"HIDIdleTime\""))?;
    let nanos: u64 = line.split_once('=')?.1.trim().parse().ok()?;
    Some(nanos / 1_000_000_000)
}

/// Puts the host to sleep: it goes to sleep as `pmset` returns.
fn sleep_now() -> Result<()> {
    output("pmset", &["sleepnow"]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_is_reached_by_its_whole_path_and_from_inside_a_directory_too_deep_for_it() {
        let _cwd = crate::wake::tests::CWD.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let back = std::env::current_dir().unwrap();
        let deep = root.path().join("d".repeat(120)).join("e".repeat(120));
        for dir in [root.path().to_path_buf(), deep] {
            std::fs::create_dir_all(&dir).unwrap();
            let _listener = at(&dir, "alpha.sock", |path| UnixListener::bind(path)).unwrap();
            assert!(dir.join("alpha.sock").exists(), "{}", dir.display());
            assert!(at(&dir, "alpha.sock", |path| UnixStream::connect(path)).is_ok());
            assert_eq!(
                std::env::current_dir().unwrap(),
                back,
                "the working directory is left alone"
            );
        }
    }

    #[test]
    fn the_host_is_read_as_sysctl_and_ioreg_say_it() {
        let said = "{ sec = 1760000000, usec = 518000 } Sat Oct 18 10:00:00 2026\n";
        assert_eq!(waketime(said), Some(1_760_000_000));
        assert_eq!(waketime("{ sec = 0, usec = 0 } Thu Jan  1 09:00:00 1970\n"), Some(0));
        assert_eq!(waketime("nothing"), None);
        let said = "    | |   \"HIDIdleTime\" = 754000000000\n    | |   \"Other\" = 1\n";
        assert_eq!(hid_idle(said), Some(754));
        assert_eq!(hid_idle("no such key"), None);
    }
}

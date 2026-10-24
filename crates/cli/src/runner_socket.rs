//! The host runner's socket: how `run`, `stop` and `checkouts` reach the host
//! runner of their host (docs/adr/0023). It is a file in the cache directory,
//! so every afkconvoy that shares the cache directory reaches the same one.
//! One line of JSON crosses it each way, the question and its answer, and
//! then the connection closes.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::copies::cache;
use crate::instance::Start;
use crate::wake::reached;

/// The socket's name in the cache directory.
pub const SOCKET: &str = "host-runner.sock";

/// How long the host runner waits for a question once it was asked one: a
/// question is one line, written at once.
const ASKED_WITHIN: Duration = Duration::from_secs(10);

/// What `run`, `stop` and `checkouts` ask the host runner: one line of JSON,
/// tagged by its `op`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Its version: answered with nothing but `host_runner`, as every answer
    /// is. One of an afkconvoy that cannot read the question says the same,
    /// beside its error.
    Version,
    /// Put `checkout` on the checkout list and start its fleet instance as
    /// `start` asks: answered once its first tick ended, or once it was
    /// refused before that.
    Add { checkout: PathBuf, start: Box<Start> },
    /// Take `checkout` off the checkout list and stop its fleet instance:
    /// answered once it has stopped.
    Stop { checkout: PathBuf },
    /// Stop every fleet instance, and then the host runner, keeping the
    /// checkout list: answered once every fleet instance has stopped.
    StopAll,
    /// The checkouts it holds a fleet instance of, each with that instance's
    /// id once its start is logged: answered at once.
    Checkouts,
}

/// The questions the host runner took and has not answered yet.
#[derive(Clone, Default)]
pub struct Answering {
    taken: Arc<(Mutex<usize>, Condvar)>,
}

impl Answering {
    /// Waits, `within` at most, until every question taken is answered: the
    /// host runner ends only once whoever asked it was told.
    pub fn settled(&self, within: Duration) {
        let (taken, changed) = &*self.taken;
        let until = Instant::now() + within;
        let mut now = taken.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while *now > 0 {
            let Some(left) = until.checked_duration_since(Instant::now()) else { return };
            now = changed.wait_timeout(now, left).map_or_else(|p| p.into_inner().0, |w| w.0);
        }
    }

    /// Counts one question taken until the guard it answers is dropped.
    fn took(&self) -> Taken {
        *self.taken.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
        Taken(self.clone())
    }
}

/// One question taken, counted until it is dropped.
struct Taken(Answering);

impl Drop for Taken {
    fn drop(&mut self) {
        let (taken, changed) = &*self.0.taken;
        *taken.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) -= 1;
        changed.notify_all();
    }
}

/// Binds the socket, over whatever a host runner that ended left: a socket
/// nobody accepts on. Only the holder of the host runner's lock calls it.
pub fn listen() -> Result<UnixListener> {
    let dir = cache()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    reached(&dir, SOCKET, |path| {
        let _ = std::fs::remove_file(path);
        UnixListener::bind(path)
    })
    .context("the host runner cannot listen on its socket")
}

/// Answers each asker on `listener`, from now on, each on a thread of its
/// own: one waiting for a first tick holds up no other → the questions taken
/// and not answered yet. Every answer gets `about` as its `host_runner`, an
/// error and a question it cannot read included, so whoever asked can tell
/// which afkconvoy answered. A question not read within `ASKED_WITHIN` is
/// dropped.
pub fn serve(
    listener: UnixListener,
    about: Value,
    answer: impl Fn(Request) -> Value + Send + Sync + 'static,
) -> Answering {
    let answer = Arc::new(answer);
    let answering = Answering::default();
    let taking = answering.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (about, answer) = (about.clone(), Arc::clone(&answer));
            let taken = taking.took();
            std::thread::spawn(move || {
                // Whatever goes wrong with one asker is its own loss.
                let _ = answered(stream, &about, answer.as_ref());
                drop(taken);
            });
        }
    });
    answering
}

/// Reads one question from `stream` and writes its answer back.
fn answered(
    stream: UnixStream,
    about: &Value,
    answer: &dyn Fn(Request) -> Value,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(ASKED_WITHIN))?;
    let mut asked = String::new();
    BufReader::new(&stream).read_line(&mut asked)?;
    let mut answered = match serde_json::from_str::<Request>(asked.trim()) {
        Ok(request) => answer(request),
        Err(_) => json!({"error": format!(
            "this host runner (afkconvoy {}) cannot read {}: it is another version than the \
             afkconvoy that asked",
            crate::VERSION,
            asked.trim()
        )}),
    };
    if let Some(answered) = answered.as_object_mut() {
        answered.insert("host_runner".into(), about.clone());
    }
    let mut stream = stream;
    writeln!(stream, "{answered}")
}

/// How asking the host runner came out.
#[derive(Debug)]
pub enum Reached {
    /// What it answered.
    Answer(Value),
    /// No host runner listens: there is no socket, or nobody accepts on it.
    Nobody,
    /// One took the question and closed before it answered: it ended.
    Closed,
    /// One took the question and gave no answer within the time it was given.
    Unanswered,
}

/// Asks the host runner `request`, waiting `within` at most for its answer.
/// No socket, or one nobody accepts on, is `Nobody`, and the socket is left
/// where it is: a host runner between its bind and its first accept takes a
/// question too, and answers it once it accepts. Errors: the cache directory
/// is not known; the question cannot be written; the answer is no JSON.
pub fn ask(request: &Request, within: Duration) -> Result<Reached> {
    ask_in(&cache()?, request, within)
}

/// `ask`, of the socket in `dir`.
fn ask_in(dir: &Path, request: &Request, within: Duration) -> Result<Reached> {
    let stream = match reached(dir, SOCKET, |path| UnixStream::connect(path)) {
        Ok(stream) => stream,
        Err(error) => {
            let kind = error.downcast_ref::<std::io::Error>().map(std::io::Error::kind);
            if matches!(kind, Some(ErrorKind::NotFound | ErrorKind::ConnectionRefused)) {
                return Ok(Reached::Nobody);
            }
            return Err(error.context("the host runner could not be asked"));
        }
    };
    // One that closed on the question, or as it answered, ended.
    let closed = |error: &std::io::Error| {
        matches!(error.kind(), ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
    };
    let asked = serde_json::to_string(request)?;
    match writeln!(&stream, "{asked}") {
        Err(error) if closed(&error) => return Ok(Reached::Closed),
        written => written.context("the host runner could not be asked")?,
    }
    stream.set_read_timeout(Some(within))?;
    let mut line = String::new();
    match BufReader::new(&stream).read_line(&mut line) {
        Ok(0) => Ok(Reached::Closed),
        Ok(_) => {
            let answer = serde_json::from_str(line.trim())
                .with_context(|| format!("the host runner answered no JSON: {}", line.trim()))?;
            Ok(Reached::Answer(answer))
        }
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Ok(Reached::Unanswered)
        }
        Err(error) if closed(&error) => Ok(Reached::Closed),
        Err(error) => Err(error).context("the host runner's answer could not be read"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_crosses_as_one_line_tagged_by_its_op() {
        let start: Start = serde_json::from_value(json!({
            "named": {"host": "h1"}, "panel": null, "no_host_sleep": true,
            "hot": {"millicores": null, "resident": null, "written": null},
            "knobs": {"ticks": null, "second_ms": 100, "host_sleep_after": 600,
                "ready_timeout": 120, "now": null}
        }))
        .unwrap();
        let add = Request::Add { checkout: "/w".into(), start: Box::new(start) };
        let line = serde_json::to_string(&add).unwrap();
        assert!(line.starts_with(r#"{"op":"add","checkout":"/w","start":{"named":{"host":"h1"}"#));
        assert!(!line.contains('\n'));
        let Ok(Request::Add { checkout, .. }) = serde_json::from_str(&line) else {
            panic!("{line}")
        };
        assert_eq!(checkout, PathBuf::from("/w"));
        let stop = serde_json::to_string(&Request::Stop { checkout: "/w".into() }).unwrap();
        assert_eq!(stop, r#"{"op":"stop","checkout":"/w"}"#);
        assert_eq!(serde_json::to_string(&Request::StopAll).unwrap(), r#"{"op":"stop_all"}"#);
        assert_eq!(serde_json::to_string(&Request::Checkouts).unwrap(), r#"{"op":"checkouts"}"#);
        assert_eq!(serde_json::to_string(&Request::Version).unwrap(), r#"{"op":"version"}"#);
    }

    #[test]
    fn settled_waits_for_every_question_taken_and_no_longer_than_it_is_given() {
        let answering = Answering::default();
        let started = Instant::now();
        answering.settled(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1), "none taken: at once");

        let taken = answering.took();
        let started = Instant::now();
        answering.settled(Duration::from_millis(200));
        assert!(started.elapsed() >= Duration::from_millis(200), "one taken: until it is given");

        let answered = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(taken);
        });
        let started = Instant::now();
        answering.settled(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(4), "once it is answered");
        answered.join().unwrap();
    }

    #[test]
    fn every_answer_says_which_host_runner_gave_it_a_question_it_cannot_read_too() {
        let about = json!({"pid": 42, "version": "0.6.0"});
        let ask = |line: &str| {
            let (asker, runner) = UnixStream::pair().unwrap();
            writeln!(&asker, "{line}").unwrap();
            let checkout = |request: Request| match request {
                Request::Add { checkout, .. } | Request::Stop { checkout } => {
                    json!({"checkout": checkout})
                }
                Request::Version | Request::StopAll | Request::Checkouts => json!({}),
            };
            answered(runner, &about, &checkout).unwrap();
            let mut answer = String::new();
            BufReader::new(&asker).read_line(&mut answer).unwrap();
            serde_json::from_str::<Value>(&answer).unwrap()
        };
        let start = r#""start":{"named":{},"panel":null,"no_host_sleep":true,"hot":{"millicores":null,"resident":null,"written":null},"knobs":{"ticks":null,"second_ms":1000,"host_sleep_after":600,"ready_timeout":120,"now":null}}"#;
        let added = ask(&format!(r#"{{"op":"add","checkout":"/w",{start}}}"#));
        assert_eq!(added, json!({"checkout": "/w", "host_runner": about}));
        assert_eq!(ask(r#"{"op":"version"}"#), json!({"host_runner": about}));
        let unread = ask(r#"{"op":"wake","checkout":"/w"}"#);
        let error = unread["error"].as_str().unwrap();
        assert!(error.starts_with("this host runner (afkconvoy "), "{error}");
        assert!(error.ends_with("it is another version than the afkconvoy that asked"), "{error}");
        assert_eq!(unread["host_runner"], about);
    }

    #[test]
    fn asking_tells_nobody_from_one_that_ended_or_said_nothing_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let add: Request = serde_json::from_value(json!({"op": "add", "checkout": "/w",
            "start": {"named": {}, "panel": null, "no_host_sleep": true,
                "hot": {"millicores": null, "resident": null, "written": null},
                "knobs": {"ticks": null, "second_ms": 1000, "host_sleep_after": 600,
                    "ready_timeout": 120, "now": null}}}))
        .unwrap();
        let within = Duration::from_millis(200);
        let asked = |dir: &Path| ask_in(dir, &add, within).unwrap();
        assert!(matches!(asked(dir.path()), Reached::Nobody), "no socket");
        // a host runner that ended left its socket: nobody accepts on it
        drop(reached(dir.path(), SOCKET, |path| UnixListener::bind(path)).unwrap());
        assert!(matches!(asked(dir.path()), Reached::Nobody), "a socket nobody accepts on");
        assert!(dir.path().join(SOCKET).exists(), "and it is left where it is");

        let _ = std::fs::remove_file(dir.path().join(SOCKET));
        let listener = reached(dir.path(), SOCKET, |path| UnixListener::bind(path)).unwrap();
        let held = std::thread::spawn(move || {
            // the first asker is answered never, the second is closed on
            let (first, _) = listener.accept().unwrap();
            let (second, _) = listener.accept().unwrap();
            drop(second);
            std::thread::sleep(Duration::from_millis(500));
            drop(first);
        });
        assert!(matches!(asked(dir.path()), Reached::Unanswered));
        assert!(matches!(asked(dir.path()), Reached::Closed));
        held.join().unwrap();
    }
}

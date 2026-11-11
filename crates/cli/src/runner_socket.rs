//! The host runner's socket: the one socket of a host, on which every command
//! that speaks to a running fleet reaches the host runner of its host —
//! `run` (its version check among it), `stop`, `checkouts`, `wake`, `panel
//! --open` (its version, which says where it serves the panel), `exclusive`,
//! and `doctor` asking whether a fleet instance runs and what the host's
//! fleets share — each question about one repo naming its checkout
//! (docs/adr/0023). It is a file in the cache directory, so every afkconvoy
//! that shares the cache directory reaches the same one. One line of JSON
//! crosses it each way, the question and its answer, and then the connection
//! closes: but for `exclusive`, which keeps the line open for as long as its
//! command waits and runs, and is told on it when its turn comes
//! (`Reply::Kept`, `Conversation`).

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use afkconvoy_core::exclusive::Ticket;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::copies::cache;
use crate::instance::Start;

/// The socket's name in the cache directory.
pub const SOCKET: &str = "host-runner.sock";

/// How long the host runner waits for a question once it was asked one: a
/// question is one line, written at once.
const ASKED_WITHIN: Duration = Duration::from_secs(10);

/// What a command asks the host runner: one line of JSON, tagged by its
/// `op`. Each question about one repo names a checkout of it — its root, or
/// any worktree of it, a worker's among them — and is answered for the fleet
/// instance of that repo the host runner holds.
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
    /// End the sleep of the fleet instance of `checkout`'s repo, naming `by`
    /// — what woke it, `park #46` — where it names anything: answered at once
    /// with what it will do of it (`wake::Wakes::hear`).
    Wake {
        checkout: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    /// Whether it holds a fleet instance of `checkout`'s repo: answered as
    /// `holds`.
    Holds { checkout: PathBuf },
    /// Put `ticket`'s command in the host's exclusive queue — its `dir`
    /// naming its checkout — and keep the asker on the line: answered at
    /// once with how many are `ahead` of it, and then with `runs`, when, once
    /// its turn comes. It leaves the queue once the asker closes its end: its
    /// command ended, or its process did. A command that waits as its host
    /// runner is replaced asks the next one the same, whichever afkconvoy
    /// that is: this line changes by fields with a default alone.
    Exclusive { ticket: Ticket },
    /// A display copy of a turn's authoritative remote candidate, no grant
    /// or fleet state changed. Older runners may ignore this question.
    Candidate { pid: u32, validation: afkconvoy_core::candidate::Validation },
    /// What the host's fleets share: answered at once with the `exclusive`
    /// queue, in the order it runs, and the `watch` running, or null.
    Shared,
}

/// The longest path a socket's address holds, with room to spare: 104 bytes
/// on macOS and 108 on Linux, the last of them a NUL.
pub(crate) const ADDRESS_HOLDS: usize = 100;

/// Runs `with` on a path that reaches the socket `name` in `dir` and fits in
/// a socket's address (some hundred bytes; a cache directory may be any
/// length): the whole path where it fits, else one through a link to `dir`
/// made for the call — `convoy-<unique()>` in the temp dir, or /tmp where
/// even that is too long — and removed after it. Nothing of the process
/// moves, so any thread may call it while others work.
pub(crate) fn reached<T>(
    dir: &Path,
    name: &str,
    with: impl FnOnce(&Path) -> std::io::Result<T>,
) -> Result<T> {
    let whole = dir.join(name);
    if whole.as_os_str().len() <= ADDRESS_HOLDS {
        return Ok(with(&whole)?);
    }
    // A link is read from where it is: it names the directory whole.
    let dir =
        std::path::absolute(dir).with_context(|| format!("cannot reach {}", dir.display()))?;
    let linked = format!("convoy-{}", crate::unique());
    let mut link = std::env::temp_dir().join(&linked);
    if link.join(name).as_os_str().len() > ADDRESS_HOLDS {
        link = Path::new("/tmp").join(&linked);
    }
    std::os::unix::fs::symlink(&dir, &link)
        .with_context(|| format!("cannot link {} to {}", link.display(), dir.display()))?;
    // A bind or a connect follows the link once: a socket bound through it is
    // in `dir`, and outlives it.
    let made = with(&link.join(name));
    let _ = std::fs::remove_file(&link);
    Ok(made?)
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

/// How the host runner answers one question.
pub enum Reply {
    /// With one line, and the connection closes.
    Line(Value),
    /// With one line, and then as `keep` says, keeping the asker on the line
    /// (`Kept`): an `exclusive` command, as it waits and runs.
    Kept(Value, Box<dyn FnOnce(Kept) + Send>),
}

/// An asker the host runner keeps on the line past its first answer.
pub struct Kept {
    stream: UnixStream,
    about: Value,
}

impl Kept {
    /// Ends this conversation, including the copy watching its peer close.
    pub fn close(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Says `said` to the asker, with which host runner says it → whether it
    /// was still there to be told.
    pub fn say(&mut self, mut said: Value) -> bool {
        if let Some(said) = said.as_object_mut() {
            said.insert("host_runner".into(), self.about.clone());
        }
        writeln!(self.stream, "{said}").is_ok()
    }

    /// Calls `then`, on a thread of its own, once the asker has closed its
    /// end of the line — at once where that cannot be watched.
    pub fn on_close(&self, then: impl FnOnce() + Send + 'static) {
        let Ok(stream) = self.stream.try_clone() else { return then() };
        std::thread::spawn(move || {
            // An asker says nothing more: whatever comes before its end is dropped.
            let _ = stream.set_read_timeout(None);
            let mut said = [0; 64];
            while matches!((&stream).read(&mut said), Ok(read) if read > 0) {}
            then();
        });
    }
}

/// Answers each asker on `listener`, from now on, each on a thread of its
/// own: one waiting for a first tick holds up no other → the questions taken
/// and not answered yet, one kept on the line being answered once its first
/// line is. Every answer gets `about` as its `host_runner`, an error and a
/// question it cannot read included, so whoever asked can tell which
/// afkconvoy answered. A question not read within `ASKED_WITHIN` is dropped.
pub fn serve(
    listener: UnixListener,
    about: Value,
    answer: impl Fn(Request) -> Reply + Send + Sync + 'static,
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
                let _ = answered(stream, &about, answer.as_ref(), taken);
            });
        }
    });
    answering
}

/// Reads one question from `stream` and answers it, counted as `taken` until
/// its first line is written.
fn answered(
    stream: UnixStream,
    about: &Value,
    answer: &dyn Fn(Request) -> Reply,
    taken: Taken,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(ASKED_WITHIN))?;
    let mut asked = String::new();
    BufReader::new(&stream).read_line(&mut asked)?;
    let reply = match serde_json::from_str::<Request>(asked.trim()) {
        Ok(request) => answer(request),
        Err(_) => Reply::Line(json!({"error": format!(
            "this host runner (afkconvoy {}) cannot read {}: it is another version than the \
             afkconvoy that asked",
            crate::VERSION,
            asked.trim()
        )})),
    };
    let mut kept = Kept { stream, about: about.clone() };
    match reply {
        Reply::Line(answered) => {
            kept.say(answered);
        }
        Reply::Kept(first, keep) => {
            kept.say(first);
            drop(taken);
            keep(kept);
        }
    }
    Ok(())
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
/// No socket, or one nobody listens on, is `Nobody`; one nobody listens on
/// is removed, unless a host runner holds the host — one between its bind
/// and its listen refuses an asker too, and is let be. A host runner between
/// its listen and its first accept takes a question, and answers it once it
/// accepts. Errors: the cache directory is not known; the question cannot be
/// written; the answer is no JSON.
pub fn ask(request: &Request, within: Duration) -> Result<Reached> {
    ask_in(&cache()?, request, within)
}

/// A bounded observation: never removes a stale socket or asks the runner
/// to change anything. Temporary path links, where required, are removed.
pub fn observe(request: &Request, within: Duration) -> Result<Reached> {
    Ok(conversation_in(&cache()?, request, within, false)?.0)
}

/// `ask`, the asker staying on the line once answered: what the host runner
/// says after its answer, as a `Conversation`, there with an answer alone.
pub fn keep_asking(request: &Request, within: Duration) -> Result<(Reached, Option<Conversation>)> {
    keep_asking_in(&cache()?, request, within)
}

/// What the host runner says to an asker it keeps on the line, after its
/// answer. Dropped, it closes the line: the host runner is told the asker is
/// gone.
pub struct Conversation {
    lines: BufReader<UnixStream>,
}

impl Conversation {
    /// A handle that ends this conversation, including a read on another
    /// thread. The caller uses it once its command ends, then joins the reader.
    pub fn ender(&self) -> Result<impl FnOnce() + use<>> {
        let stream = self.lines.get_ref().try_clone()?;
        Ok(move || {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        })
    }

    /// Waits, as long as it takes, for what the host runner says next →
    /// `None`: it closed the line, as it ended. Errors: what it said is no
    /// JSON; the line could not be read.
    pub fn heard(&mut self) -> Result<Option<Value>> {
        // A line the host runner closed refuses it (EINVAL on macOS), and reads
        // its end at once.
        let _ = self.lines.get_ref().set_read_timeout(None);
        let mut line = String::new();
        match self.lines.read_line(&mut line) {
            // One that ended as it said something said nothing.
            Ok(_) if !line.ends_with('\n') => Ok(None),
            Ok(_) => serde_json::from_str(line.trim())
                .map(Some)
                .with_context(|| format!("the host runner said no JSON: {}", line.trim())),
            Err(error) if closed(&error) => Ok(None),
            Err(error) => Err(error).context("what the host runner said could not be read"),
        }
    }
}

/// Whether `error` is the host runner's end closing on the line: it ended.
fn closed(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionReset
            | ErrorKind::InvalidInput
            | ErrorKind::NotConnected
    )
}

/// `ask`, of the socket in `dir`.
fn ask_in(dir: &Path, request: &Request, within: Duration) -> Result<Reached> {
    Ok(keep_asking_in(dir, request, within)?.0)
}

/// `keep_asking`, of the socket in `dir`.
fn keep_asking_in(
    dir: &Path,
    request: &Request,
    within: Duration,
) -> Result<(Reached, Option<Conversation>)> {
    conversation_in(dir, request, within, true)
}

fn conversation_in(
    dir: &Path,
    request: &Request,
    within: Duration,
    clean_stale: bool,
) -> Result<(Reached, Option<Conversation>)> {
    let stream = match reached(dir, SOCKET, |path| UnixStream::connect(path)) {
        Ok(stream) => stream,
        Err(error) => {
            let kind = error.downcast_ref::<std::io::Error>().map(std::io::Error::kind);
            if clean_stale
                && kind == Some(ErrorKind::ConnectionRefused)
                && !crate::host_runner::held_in(dir)
            {
                // What a host runner that ended with no goodbye left.
                let _ = std::fs::remove_file(dir.join(SOCKET));
            }
            if matches!(kind, Some(ErrorKind::NotFound | ErrorKind::ConnectionRefused)) {
                return Ok((Reached::Nobody, None));
            }
            return Err(error.context("the host runner could not be asked"));
        }
    };
    // Before the question: a host runner that answers and closes the line
    // before the timeout is set leaves a line that refuses it (EINVAL on
    // macOS). One it closed already reads its end at once.
    let _ = stream.set_read_timeout(Some(within));
    let _ = stream.set_write_timeout(Some(within));
    let asked = serde_json::to_string(request)?;
    match writeln!(&stream, "{asked}") {
        Err(error) if closed(&error) => return Ok((Reached::Closed, None)),
        written => written.context("the host runner could not be asked")?,
    }
    let mut lines = BufReader::new(stream);
    let mut line = String::new();
    match read_line_within(&mut lines, &mut line, within) {
        Ok(0) => Ok((Reached::Closed, None)),
        Ok(_) => {
            let answer = serde_json::from_str(line.trim())
                .with_context(|| format!("the host runner answered no JSON: {}", line.trim()))?;
            Ok((Reached::Answer(answer), Some(Conversation { lines })))
        }
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Ok((Reached::Unanswered, None))
        }
        Err(error) if closed(&error) => Ok((Reached::Closed, None)),
        Err(error) => Err(error).context("the host runner's answer could not be read"),
    }
}

/// A total deadline, including a peer that dribbles part of a line forever.
/// Retain bytes after the newline for exclusive's continuing conversation.
fn read_line_within(
    lines: &mut BufReader<UnixStream>,
    line: &mut String,
    within: Duration,
) -> std::io::Result<usize> {
    let began = Instant::now();
    let mut bytes = vec![];
    loop {
        let left = within.saturating_sub(began.elapsed());
        if left.is_zero() {
            return Err(ErrorKind::TimedOut.into());
        }
        let _ = lines.get_ref().set_read_timeout(Some(left));
        let buffer = lines.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        let length = buffer.iter().position(|byte| *byte == b'\n').map(|at| at + 1);
        let take = length.unwrap_or(buffer.len());
        bytes.extend_from_slice(&buffer[..take]);
        lines.consume(take);
        if length.is_some() {
            break;
        }
    }
    *line = String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
    Ok(line.len())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn a_socket_is_reached_by_its_whole_path_or_through_a_link_and_is_made_in_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let deep = root.path().join("d".repeat(120)).join("e".repeat(120));
        for dir in [root.path().to_path_buf(), deep] {
            std::fs::create_dir_all(&dir).unwrap();
            let _listener = reached(&dir, "alpha.sock", |path| UnixListener::bind(path)).unwrap();
            assert!(dir.join("alpha.sock").exists(), "{}", dir.display());
            assert!(reached(&dir, "alpha.sock", |path| UnixStream::connect(path)).is_ok());
        }
    }

    #[test]
    fn threads_reach_sockets_too_deep_for_an_address_at_once_and_nothing_moves() {
        let root = tempfile::tempdir().unwrap();
        let back = std::env::current_dir().unwrap();
        let done = Arc::new(AtomicBool::new(false));
        // The working directory, read the whole time the threads work.
        let watched = {
            let (done, back) = (Arc::clone(&done), back.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    assert_eq!(std::env::current_dir().unwrap(), back);
                }
            })
        };
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let dir = root.path().join(format!("{n}-{}", "d".repeat(120)));
                std::fs::create_dir_all(&dir).unwrap();
                std::thread::spawn(move || {
                    let listener =
                        reached(&dir, "alpha.sock", |path| UnixListener::bind(path)).unwrap();
                    assert!(dir.join("alpha.sock").exists(), "{}", dir.display());
                    reached(&dir, "alpha.sock", |path| UnixStream::connect(path)).unwrap();
                    listener.accept().unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        done.store(true, Ordering::Relaxed);
        watched.join().expect("the working directory never moved");
        assert_eq!(std::env::current_dir().unwrap(), back);
        // Every link made for a call is gone after it.
        let ours = format!("convoy-{}-", std::process::id());
        for dir in [std::env::temp_dir(), PathBuf::from("/tmp")] {
            let entries = std::fs::read_dir(&dir).unwrap().flatten();
            let left: Vec<_> = entries
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(&ours))
                .collect();
            assert!(left.is_empty(), "{left:?} in {}", dir.display());
        }
    }

    #[test]
    fn a_request_crosses_as_one_line_tagged_by_its_op() {
        let start: Start = serde_json::from_value(json!({
            "named": {"host": "h1"}, "panel": null,
            "hot": {"millicores": null, "resident": null, "written": null},
            "knobs": {"ticks": null, "second_ms": 100,
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
        let woke = Request::Wake { checkout: "/w".into(), by: Some("park #46".into()) };
        let woke = serde_json::to_string(&woke).unwrap();
        assert_eq!(woke, r#"{"op":"wake","checkout":"/w","by":"park #46"}"#);
        let unnamed = serde_json::to_string(&Request::Wake { checkout: "/w".into(), by: None });
        assert_eq!(unnamed.unwrap(), r#"{"op":"wake","checkout":"/w"}"#, "naming nothing");
        let holds = serde_json::to_string(&Request::Holds { checkout: "/w".into() });
        assert_eq!(holds.unwrap(), r#"{"op":"holds","checkout":"/w"}"#);
        assert_eq!(serde_json::to_string(&Request::Shared).unwrap(), r#"{"op":"shared"}"#);
        // What a command waiting as its host runner is replaced asks the
        // next, whichever afkconvoy that is.
        let ticket = Ticket {
            number: 0,
            pid: 7,
            dir: "/w".into(),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            note: afkconvoy_core::exclusive::Note { issue: Some(46), validation: None },
            gate: None,
            command: "cargo test".into(),
            asked: 1000,
            ran: None,
        };
        let exclusive = serde_json::to_string(&Request::Exclusive { ticket }).unwrap();
        assert_eq!(
            exclusive,
            r#"{"op":"exclusive","ticket":{"number":0,"pid":7,"command":"cargo test","dir":"/w","issue":46,"asked":1000,"ran":null}}"#
        );
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
            let checkout = |request: Request| {
                Reply::Line(match request {
                    Request::Add { checkout, .. }
                    | Request::Stop { checkout }
                    | Request::Wake { checkout, .. }
                    | Request::Holds { checkout } => json!({"checkout": checkout}),
                    Request::Exclusive { ticket } => json!({"checkout": ticket.dir}),
                    Request::Version
                    | Request::StopAll
                    | Request::Checkouts
                    | Request::Shared
                    | Request::Candidate { .. } => {
                        json!({})
                    }
                })
            };
            answered(runner, &about, &checkout, Answering::default().took()).unwrap();
            let mut answer = String::new();
            BufReader::new(&asker).read_line(&mut answer).unwrap();
            serde_json::from_str::<Value>(&answer).unwrap()
        };
        // an older afkconvoy's start, with keys this one has none of, is read
        let start = r#""start":{"named":{},"panel":null,"no_host_sleep":true,"hot":{"millicores":null,"resident":null,"written":null},"knobs":{"ticks":null,"second_ms":1000,"host_sleep_after":600,"ready_timeout":120,"now":null}}"#;
        let added = ask(&format!(r#"{{"op":"add","checkout":"/w",{start}}}"#));
        assert_eq!(added, json!({"checkout": "/w", "host_runner": about}));
        assert_eq!(ask(r#"{"op":"version"}"#), json!({"host_runner": about}));
        let unread = ask(r#"{"op":"dance","checkout":"/w"}"#);
        let error = unread["error"].as_str().unwrap();
        assert!(error.starts_with("this host runner (afkconvoy "), "{error}");
        assert!(error.ends_with("it is another version than the afkconvoy that asked"), "{error}");
        assert_eq!(unread["host_runner"], about);
    }

    #[test]
    fn an_asker_kept_on_the_line_hears_what_comes_after_and_its_leaving_is_heard() {
        let dir = tempfile::tempdir().unwrap();
        let listener = reached(dir.path(), SOCKET, |path| UnixListener::bind(path)).unwrap();
        let (left, gone) = std::sync::mpsc::channel();
        let answering = serve(listener, json!({"pid": 42}), move |_| {
            let left = left.clone();
            Reply::Kept(
                json!({"ahead": 1}),
                Box::new(move |mut kept: Kept| {
                    kept.on_close(move || left.send(()).unwrap());
                    assert!(kept.say(json!({"runs": 1000})));
                }),
            )
        });
        let (reached, conversation) =
            keep_asking_in(dir.path(), &Request::Shared, Duration::from_secs(5)).unwrap();
        let Reached::Answer(answer) = reached else { panic!("{reached:?}") };
        assert_eq!(answer, json!({"ahead": 1, "host_runner": {"pid": 42}}));
        let mut conversation = conversation.expect("kept on the line");
        let said = conversation.heard().unwrap();
        assert_eq!(said, Some(json!({"runs": 1000, "host_runner": {"pid": 42}})));
        answering.settled(Duration::from_secs(5));
        assert!(gone.try_recv().is_err(), "still on the line");
        drop(conversation);
        gone.recv_timeout(Duration::from_secs(5)).expect("its leaving is heard");

        // one the host runner closes on hears nothing more
        let (asker, runner) = UnixStream::pair().unwrap();
        let mut conversation = Conversation { lines: BufReader::new(asker) };
        drop(runner);
        assert_eq!(conversation.heard().unwrap(), None);
    }

    #[test]
    fn asking_tells_nobody_from_one_that_ended_or_said_nothing_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let add: Request = serde_json::from_value(json!({"op": "add", "checkout": "/w",
            "start": {"named": {}, "panel": null,
                "hot": {"millicores": null, "resident": null, "written": null},
                "knobs": {"ticks": null, "second_ms": 1000,
                    "ready_timeout": 120, "now": null}}}))
        .unwrap();
        let within = Duration::from_millis(200);
        let asked = |dir: &Path| ask_in(dir, &add, within).unwrap();
        assert!(matches!(asked(dir.path()), Reached::Nobody), "no socket");
        // a host runner that ended left its socket: nobody listens on it
        drop(reached(dir.path(), SOCKET, |path| UnixListener::bind(path)).unwrap());
        assert!(matches!(asked(dir.path()), Reached::Nobody), "a socket nobody listens on");
        assert!(!dir.path().join(SOCKET).exists(), "is removed by whoever finds it");

        // …unless a host runner holds the host: it may be about to listen
        let lock = std::fs::File::create(dir.path().join(crate::host_runner::LOCK)).unwrap();
        lock.lock().unwrap();
        drop(reached(dir.path(), SOCKET, |path| UnixListener::bind(path)).unwrap());
        assert!(matches!(asked(dir.path()), Reached::Nobody));
        assert!(dir.path().join(SOCKET).exists(), "it is let be");
        drop(lock);

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

    #[test]
    fn macos_s_invalid_read_of_a_closed_socket_is_a_close_not_an_error() {
        let invalid = std::io::Error::from(ErrorKind::InvalidInput);
        assert!(closed(&invalid));
        // a write to a line the host runner already closed: ENOTCONN on macOS
        assert!(closed(&std::io::Error::from(ErrorKind::NotConnected)));
        assert!(!closed(&std::io::Error::from(ErrorKind::InvalidData)));
    }
}

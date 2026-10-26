//! `afkconvoy exclusive`'s effect half (docs/adr/0019): a heavy command run
//! once no other `exclusive` command runs on the host, across all its fleets.
//!
//! The queue is the host runner's (docs/adr/0023), held in its process for
//! every fleet on the host whatever its repo (`Queue`). Each command waiting
//! or running asks it on its socket, naming its checkout, and stays on the
//! line: it is told there once its turn comes, and leaves the queue as its
//! line closes — its command ended, or its process did. Which ticket runs is
//! the core's to say (`afkconvoy_core::exclusive`).

use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use afkconvoy_core::exclusive::{
    Ticket, ahead, at_once_line, ended_line, in_order, queued_since, rejoins_line, runs_line,
    waits_line,
};
use afkconvoy_core::role::ISSUE_ENV;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::acting::Acting;
use crate::git::Repo;
use crate::runner_socket::{self, Conversation, Kept, Reached, Reply, Request};

/// How long a command whose host runner ended as it waited — or one that
/// finds a host runner starting or stopping — asks again for one to answer,
/// before it runs in no queue: a host runner replaced is back within it.
const REACHED_WITHIN: Duration = Duration::from_secs(60);
/// How often it asks again meanwhile.
const ASKS_EVERY: Duration = Duration::from_millis(200);
/// How long a host runner is given to say where a command stands in its
/// queue: it says it at once.
const PLACED_WITHIN: Duration = Duration::from_secs(10);

/// The default bound on time holding other commands up (docs/adr/0031).
pub const SECONDS: u64 = 1200;
/// Reserved for a command ended at the exclusive bound. A child's own use
/// of it is reported as exit 1, so this outcome is unambiguous.
pub const BOUND_CODE: u8 = 124;
const CHECK_EVERY: Duration = Duration::from_millis(100);

/// What the host runner tells a command that reached the bound. It keeps
/// its place until the command has ended and reaped its descendants.
#[derive(Debug, Deserialize, Serialize)]
struct Bound {
    bound: u64,
    waiting: usize,
}

/// The host's exclusive queue, as its host runner holds it: one command runs
/// at a time, the one that asked first, and each leaves as its asker's line
/// closes. A clone is the same.
#[derive(Clone, Default)]
pub struct Queue {
    held: Arc<(Mutex<Joined>, Condvar)>,
}

/// Every ticket in the queue, waiting or running, and the number the next
/// to join takes.
struct Joined {
    tickets: Vec<Ticket>,
    next: u64,
    waited: Duration,
    counted_at: Instant,
}

impl Default for Joined {
    fn default() -> Self {
        Self { tickets: vec![], next: 0, waited: Duration::ZERO, counted_at: Instant::now() }
    }
}

impl Joined {
    /// Account for the interval before any queue change, so short waits,
    /// gaps without waiters and a changing set of waiters count correctly.
    fn count(&mut self, now: Instant) {
        if self.tickets.len() > 1 && self.tickets.iter().any(|ticket| ticket.ran.is_some()) {
            self.waited = self.waited.saturating_add(now.duration_since(self.counted_at));
        }
        self.counted_at = now;
    }
}

impl Queue {
    /// The queue in the order it runs: the first runs, or is about to, and
    /// the rest wait.
    pub fn standing(&self) -> Vec<Ticket> {
        in_order(self.joined().tickets.clone())
    }

    /// Answers `exclusive` asking for `ticket`: it joins the queue, and its
    /// asker is told at once how many are `ahead` of it, and later that it
    /// `runs` — on the line it is kept on, as long as it stays there.
    pub fn answer(&self, ticket: Ticket) -> Reply {
        let (number, ahead) = {
            let mut joined = self.joined();
            joined.count(Instant::now());
            joined.next = joined.next.saturating_add(1);
            let number = joined.next;
            joined.tickets.push(Ticket { number, ran: None, ..ticket });
            (number, ahead(&joined.tickets, number).unwrap_or(0))
        };
        self.held.1.notify_all();
        let queue = self.clone();
        Reply::Kept(json!({"ahead": ahead}), Box::new(move |kept| queue.hold(number, kept)))
    }

    /// Holds the ticket `number` for its asker on `kept`, telling it once
    /// none is ahead of it: it leaves the queue once the asker's line closes,
    /// or where it cannot be told.
    fn hold(&self, number: u64, mut kept: Kept) {
        let queue = self.clone();
        kept.on_close(move || queue.leave(number));
        let (_, turned) = &*self.held;
        let mut joined = self.joined();
        loop {
            match ahead(&joined.tickets, number) {
                // Its asker left as it waited.
                None => return,
                Some(0) => break,
                Some(_) => joined = turned.wait(joined).unwrap_or_else(PoisonError::into_inner),
            }
        }
        let ran = crate::clock();
        joined.waited = Duration::ZERO;
        joined.counted_at = Instant::now();
        for ticket in joined.tickets.iter_mut().filter(|ticket| ticket.number == number) {
            ticket.ran = Some(ran);
        }
        drop(joined);
        if !kept.say(json!({"runs": ran})) {
            self.leave(number);
            return;
        }
        let mut joined = self.joined();
        loop {
            if !joined.tickets.iter().any(|ticket| ticket.number == number) {
                return;
            }
            joined.count(Instant::now());
            // A changed host setting applies to the command already running.
            // An unreadable file leaves the default in force.
            let bound = crate::host_settings::of_host().unwrap_or_default().exclusive_seconds();
            let waiting = joined.tickets.len().saturating_sub(1);
            if waiting > 0 && joined.waited >= Duration::from_secs(bound) {
                drop(joined);
                kept.say(json!(Bound { bound, waiting }));
                // Do not give the next command its turn before cleanup.
                return;
            }
            joined =
                turned.wait_timeout(joined, CHECK_EVERY).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Takes the ticket `number` out of the queue: the next runs, if it was
    /// the one that ran.
    fn leave(&self, number: u64) {
        let mut joined = self.joined();
        joined.count(Instant::now());
        joined.tickets.retain(|ticket| ticket.number != number);
        drop(joined);
        self.held.1.notify_all();
    }

    fn joined(&self) -> MutexGuard<'_, Joined> {
        self.held.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Listen while the command runs. Closing the conversation on drop both
/// releases its ticket and unblocks the reader; no thread or socket remains.
struct Watch {
    read: mpsc::Receiver<Bound>,
    end: Option<Box<dyn FnOnce()>>,
    reader: Option<std::thread::JoinHandle<Conversation>>,
}

impl Watch {
    fn new(mut place: Conversation) -> Result<Self> {
        let end = Box::new(place.ender()?);
        let (send, read) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            while let Ok(Some(said)) = place.heard() {
                if let Ok(bound) = serde_json::from_value::<Bound>(said) {
                    let _ = send.send(bound);
                    break;
                }
            }
            // Keep the ticket even after the host runner's line closes or
            // it asks for the command to end: the owning thread cleans up.
            place
        });
        Ok(Self { read, end: Some(end), reader: Some(reader) })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(end) = self.end.take() {
            end();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// The host's exclusive queue, as its host runner says it, in the order it
/// runs; empty where no host runner runs. Errors: the host runner could not
/// be asked, or said no queue it holds.
pub fn standing() -> Result<Vec<Ticket>> {
    Ok(crate::host_runner::shared()?.map(|shared| shared.exclusive).unwrap_or_default())
}

/// Since when the worker in the worktree `dir` has waited in this host's
/// queue, or run what it waited for; `None`: it has nothing there. Soft: a
/// queue that cannot be read holds nothing.
pub fn queued_in(dir: &Path) -> Option<u64> {
    queued_since(&standing().ok()?, &canonical(dir))
}

/// `dir` as a ticket names it: whole, with no link in it.
fn canonical(dir: &Path) -> String {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()).to_string_lossy().into_owned()
}

/// How a command's turn came.
enum Turn {
    /// The host runner says it runs: the line, held open for as long as it
    /// runs, holds its place.
    Queued(Conversation),
    /// It runs in no queue, as this says why.
    AtOnce(String),
}

/// Waits in the host runner's queue for `ticket`'s turn, saying to `acting`
/// and stderr how it waits. One whose host runner ends as it waits asks the
/// next, as it first asked, for `REACHED_WITHIN`; one that finds no host
/// runner, or one that cannot hold it, runs at once. Errors: the host runner
/// could not be asked, or what it said could not be read.
fn turn(ticket: &Ticket, acting: &Acting, now: &dyn Fn() -> u64) -> Result<Turn> {
    let asked = Request::Exclusive { ticket: ticket.clone() };
    // Since when it has asked with no host runner placing it.
    let mut lost: Option<Instant> = None;
    loop {
        let (reached, conversation) = runner_socket::keep_asking(&asked, PLACED_WITHIN)?;
        match (reached, conversation) {
            (Reached::Answer(answer), Some(mut conversation)) => {
                if let Some(error) = answer.get("error") {
                    let why = format!(
                        "the host runner cannot hold it: {}",
                        error.as_str().unwrap_or_default()
                    );
                    return Ok(Turn::AtOnce(why));
                }
                let waiting = answer["ahead"].as_u64().unwrap_or(0);
                if waiting > 0 {
                    let line = waits_line(ticket, usize::try_from(waiting).unwrap_or(usize::MAX));
                    eprintln!("[afkconvoy] exclusive: {line}");
                    acting.say(now(), line);
                }
                // What it cannot read, a newer host runner's, is passed over.
                while let Some(said) = conversation.heard()? {
                    if said.get("runs").is_some() {
                        return Ok(Turn::Queued(conversation));
                    }
                }
                let line = rejoins_line(ticket);
                eprintln!("[afkconvoy] exclusive: {line}");
                acting.say(now(), line);
                lost = Some(Instant::now());
            }
            (Reached::Nobody, _) if lost.is_none() && !crate::host_runner::runs() => {
                return Ok(Turn::AtOnce("no host runner runs on this host".to_string()));
            }
            _ => {
                let since = *lost.get_or_insert_with(Instant::now);
                if since.elapsed() >= REACHED_WITHIN {
                    let why = format!(
                        "no host runner placed it in its queue within {}s",
                        REACHED_WITHIN.as_secs()
                    );
                    return Ok(Turn::AtOnce(why));
                }
                std::thread::sleep(ASKS_EVERY);
            }
        }
    }
}

/// `afkconvoy exclusive -- <command>`: runs `command` in the directory `dir`
/// once no other `exclusive` command runs on this host, waiting its turn in
/// the host runner's queue until then → its exit status. `checkout` is the
/// checkout `dir` is in, where it is in one: its ticket names it, and its tick
/// log is where the queue's own words go, as well as to stderr. What it prints
/// is the command's. With no host runner, or one that cannot hold it, it runs
/// the command at once, saying so on stderr (`turn`). It fails before the
/// command runs with one JSON object and exit 125, and with 127 for a command
/// that cannot be started.
pub fn exclusive(
    command: &[OsString],
    dir: &Path,
    checkout: Option<Repo>,
    now: impl Fn() -> u64,
) -> ExitCode {
    let shown: Vec<String> =
        command.iter().map(|word| word.to_string_lossy().into_owned()).collect();
    let ticket = Ticket {
        number: 0,
        pid: std::process::id(),
        command: shown.join(" "),
        dir: canonical(checkout.as_ref().map_or(dir, |repo| &repo.dir)),
        issue: std::env::var(ISSUE_ENV).ok().and_then(|issue| issue.parse().ok()),
        asked: now(),
        ran: None,
    };
    let acting = Acting::new("exclusive").on_own_issue().in_checkout(checkout);
    let place = match turn(&ticket, &acting, &now) {
        Ok(Turn::Queued(place)) => {
            let started = now();
            acting.say(started, runs_line(&ticket, started));
            Some(place)
        }
        Ok(Turn::AtOnce(why)) => {
            let line = at_once_line(&ticket, &why);
            eprintln!("[afkconvoy] exclusive: {line}");
            acting.say(now(), line);
            None
        }
        Err(error) => return failed(&format!("{error:#}")),
    };
    let ticket = Ticket { ran: Some(now()), ..ticket };
    let watch = match place.map(Watch::new).transpose() {
        Ok(watch) => watch,
        Err(error) => return failed(&format!("{error:#}")),
    };
    let status = run_command(command, dir, watch.as_ref(), &ticket, &acting, &now);
    // Its place is free the moment the command ends.
    drop(watch);
    let (code, ended) = match status {
        Ok(None) => return ExitCode::from(BOUND_CODE),
        Ok(Some(status)) => ended(status),
        Err(error) => {
            eprintln!("[afkconvoy] exclusive: `{}` could not be started: {error}", shown[0]);
            (127, "could not be started".to_string())
        }
    };
    acting.say(now(), ended_line(&ticket, &ended, now()));
    ExitCode::from(code)
}

/// Wait for the child or the bound, preserving its streams. On the bound,
/// end its descendants and reap it before releasing its ticket.
/// `None` is the bound's outcome; it has its own single log line.
fn run_command(
    command: &[OsString],
    dir: &Path,
    watch: Option<&Watch>,
    ticket: &Ticket,
    acting: &Acting,
    now: &dyn Fn() -> u64,
) -> std::io::Result<Option<ExitStatus>> {
    let mut child =
        crate::process::spawn(Command::new(&command[0]).args(&command[1..]).current_dir(dir))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if let Some(bound) = watch.and_then(|watch| watch.read.try_recv().ok()) {
            end_descendants(child.id());
            child.wait()?;
            let line = format!(
                "ended at the exclusive bound of {}s after {}s with {} commands waiting: `{}`; \
                 run each step of the work as its own `afkconvoy exclusive`",
                bound.bound,
                started.elapsed().as_secs(),
                bound.waiting,
                ticket.command
            );
            eprintln!("[afkconvoy] exclusive: {line}");
            acting.say(now(), line);
            return Ok(None);
        }
        std::thread::sleep(CHECK_EVERY);
    }
}

/// Stop each process before discovering its children, repeating until none
/// can fork another. Keep the inherited process group: a gate that reaches
/// its own limit must still be able to end exclusive and its command together.
fn end_descendants(root: u32) {
    let mut stopped = vec![root];
    // SAFETY: root is our unreaped child's pid, never the caller's group.
    unsafe {
        libc::kill(root as i32, libc::SIGSTOP);
    }
    loop {
        let procs = crate::hot::listed();
        let children: Vec<_> = procs
            .iter()
            .filter(|proc| stopped.contains(&proc.ppid) && !stopped.contains(&proc.pid))
            .map(|proc| proc.pid)
            .collect();
        if children.is_empty() {
            break;
        }
        for pid in children {
            // SAFETY: these are descendants of the stopped child.
            unsafe {
                libc::kill(pid as i32, libc::SIGSTOP);
            }
            stopped.push(pid);
        }
    }
    for pid in stopped.into_iter().rev() {
        // SAFETY: each pid was found under the stopped child above.
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

/// The exit code a command that ended with `status` is passed on with — a
/// shell's for one a signal ended, 128 and the signal — and how it ended, in
/// words.
fn ended(status: ExitStatus) -> (u8, String) {
    match (status.code(), status.signal()) {
        (Some(code), _) if code == i32::from(BOUND_CODE) => (
            1,
            format!("exit {code}, reported as 1: {BOUND_CODE} is reserved for the exclusive bound"),
        ),
        (Some(code), _) => ((code & 0xff) as u8, format!("exit {code}")),
        (None, Some(signal)) => {
            ((128 + signal).clamp(0, 255) as u8, format!("killed by signal {signal}"))
        }
        (None, None) => (1, "ended unexplained".to_string()),
    }
}

/// A failure before the command ran: of the queue's own, or of where it
/// would run.
pub fn failed(message: &str) -> ExitCode {
    println!("{}", json!({ "error": format!("exclusive: {message}; the command did not run") }));
    ExitCode::from(125)
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    use serde_json::Value;

    use super::*;

    fn ticket(command: &str, asked: u64) -> Ticket {
        Ticket {
            number: 0,
            pid: std::process::id(),
            command: command.into(),
            dir: "/w/issue-7".into(),
            issue: Some(7),
            asked,
            ran: None,
        }
    }

    #[test]
    fn contention_pauses_without_waiters_and_changing_waiters_does_not_reset_it() {
        let start = Instant::now();
        let mut running = ticket("running", 0);
        running.ran = Some(0);
        let mut joined = Joined { tickets: vec![running], counted_at: start, ..Joined::default() };
        joined.count(start + Duration::from_secs(20));
        assert_eq!(joined.waited, Duration::ZERO, "time alone does not count");
        joined.tickets.push(ticket("waiting", 20));
        joined.count(start + Duration::from_secs(23));
        assert_eq!(joined.waited, Duration::from_secs(3));
        joined.tickets.pop();
        joined.count(start + Duration::from_secs(50));
        assert_eq!(joined.waited, Duration::from_secs(3), "nobody waits in the gap");
        joined.tickets.push(ticket("another waiter", 50));
        joined.count(start + Duration::from_secs(54));
        assert_eq!(joined.waited, Duration::from_secs(7), "another waiter resumes the count");
    }

    /// An asker on the queue's line: what it was told first, and the line.
    struct Asker {
        lines: BufReader<UnixStream>,
    }

    impl Asker {
        /// Joins the queue served in `dir` (`served`) with `ticket`.
        fn joins(dir: &Path, ticket: Ticket) -> (Asker, Value) {
            let stream = runner_socket::reached(dir, SOCKET, |path| UnixStream::connect(path));
            let stream = stream.unwrap();
            let asked = serde_json::to_string(&Request::Exclusive { ticket }).unwrap();
            writeln!(&stream, "{asked}").unwrap();
            let mut asker = Asker { lines: BufReader::new(stream) };
            let first = asker.next().unwrap();
            (asker, first)
        }

        /// What it is told next, waiting a second at most; `None`: nothing.
        fn next(&mut self) -> Option<Value> {
            self.lines.get_ref().set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut line = String::new();
            self.lines.read_line(&mut line).ok()?;
            serde_json::from_str(&line).ok()
        }
    }

    fn commands(queue: &Queue) -> Vec<String> {
        queue.standing().into_iter().map(|ticket| ticket.command).collect()
    }

    /// The socket a test's queue is served on.
    const SOCKET: &str = "q.sock";

    /// Serves `queue` on a socket in `dir` as a host runner does.
    fn served(dir: &Path, queue: &Queue) {
        let listener = runner_socket::reached(dir, SOCKET, |path| UnixListener::bind(path));
        let queue = queue.clone();
        runner_socket::serve(listener.unwrap(), json!({}), move |request| match request {
            Request::Exclusive { ticket } => queue.answer(ticket),
            _ => Reply::Line(json!({})),
        });
    }

    /// Waits a while for `queue` to hold `commands`, in the order it runs.
    fn holds(queue: &Queue, commands_now: &[&str]) {
        let until = Instant::now() + Duration::from_secs(5);
        while commands(queue) != commands_now && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(commands(queue), commands_now);
    }

    #[test]
    fn one_runs_at_a_time_in_the_order_asked_and_each_leaves_as_its_line_closes() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::default();
        served(dir.path(), &queue);
        let (mut first, said) = Asker::joins(dir.path(), ticket("a", 100));
        assert_eq!(said["ahead"], 0);
        assert!(first.next().unwrap()["runs"].is_u64(), "it runs at once");
        let (mut second, said) = Asker::joins(dir.path(), ticket("b", 101));
        assert_eq!(said["ahead"], 1);
        let (mut third, said) = Asker::joins(dir.path(), ticket("c", 102));
        assert_eq!(said["ahead"], 2);
        assert_eq!(commands(&queue), ["a", "b", "c"]);
        assert!(queue.standing()[0].ran.is_some() && queue.standing()[1].ran.is_none());
        assert_eq!(second.next(), None, "waits while the first runs");

        drop(first);
        assert!(second.next().unwrap()["runs"].is_u64(), "its turn, once the first left");
        holds(&queue, &["b", "c"]);
        assert_eq!(third.next(), None);
        drop(second);
        assert!(third.next().unwrap()["runs"].is_u64());
        drop(third);
        holds(&queue, &[]);
    }

    #[test]
    fn one_whose_process_dies_as_it_waits_leaves_and_one_that_asked_again_keeps_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::default();
        served(dir.path(), &queue);
        let (mut running, _) = Asker::joins(dir.path(), ticket("a", 300));
        assert!(running.next().unwrap()["runs"].is_u64());
        let (waiting, _) = Asker::joins(dir.path(), ticket("b", 301));
        // One that asked of the host runner before this one: before b.
        let (mut rejoined, said) = Asker::joins(dir.path(), ticket("c", 200));
        assert_eq!(said["ahead"], 1, "behind the one running alone");
        assert_eq!(commands(&queue), ["a", "c", "b"]);
        drop(waiting);
        holds(&queue, &["a", "c"]);
        drop(running);
        assert!(rejoined.next().unwrap()["runs"].is_u64());
    }

    #[test]
    fn an_ended_command_is_passed_on_as_a_shell_would() {
        let run =
            |line: &str| crate::process::status(Command::new("sh").args(["-c", line])).unwrap();
        assert_eq!(ended(run("exit 3")), (3, "exit 3".to_string()));
        assert_eq!(ended(run("kill -9 $$")), (137, "killed by signal 9".to_string()));
    }
}

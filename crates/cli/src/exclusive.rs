//! `afkconvoy exclusive`'s effect half (docs/adr/0019): a heavy command run
//! once no other `exclusive` command runs on the host, across all its fleets.
//!
//! The queue is one directory of the cache, shared by every fleet on the host
//! whatever its repo, as host sleep's sockets are (docs/adr/0016). Each
//! command waiting or running has a ticket there, a file its process holds
//! locked for as long as it lives: a ticket whose lock can be taken is what a
//! process that died left, and is removed. The directory is changed, and read,
//! only by one holding its own lock. Which ticket runs is the core's to say
//! (`afkconvoy_core::exclusive`).

use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};
use std::time::Duration;

use afkconvoy_core::exclusive::{
    Ticket, ahead, ended_line, in_order, next_number, queued_since, runs_line, waits_line,
};
use afkconvoy_core::role::ISSUE_ENV;
use anyhow::{Context, Result};
use serde_json::json;

use crate::acting::Acting;
use crate::copies::cache;
use crate::git::Repo;

/// The queue's own lock, beside the tickets.
const LOCK: &str = "queue.lock";
/// How a ticket's file is named: its number, with this after it.
const SUFFIX: &str = ".json";
/// How often a waiting ticket looks whether its turn came.
const POLL: Duration = Duration::from_millis(200);

/// The queue of `exclusive` commands of this host.
pub struct Queue {
    dir: PathBuf,
}

impl Queue {
    /// The queue every fleet of this host shares.
    pub fn of_host() -> Result<Queue> {
        Ok(Queue::at(cache()?.join("exclusive")))
    }

    pub fn at(dir: impl Into<PathBuf>) -> Queue {
        Queue { dir: dir.into() }
    }

    /// Takes the queue's lock: held until what it answers is dropped.
    fn lock(&self) -> Result<File> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot make {}", self.dir.display()))?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(LOCK))
            .with_context(|| format!("cannot open {}", self.dir.join(LOCK).display()))?;
        lock.lock().with_context(|| format!("cannot lock {}", self.dir.join(LOCK).display()))?;
        Ok(lock)
    }

    /// The tickets of live processes, in the order they run; what a dead one
    /// left is removed. Called holding the queue's lock.
    fn live(&self) -> Result<Vec<Ticket>> {
        let entries = std::fs::read_dir(&self.dir)
            .with_context(|| format!("cannot read {}", self.dir.display()))?;
        let mut live = vec![];
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(number) = name.strip_suffix(SUFFIX).and_then(|n| n.parse::<u64>().ok()) else {
                continue;
            };
            let Ok(mut file) = File::open(entry.path()) else { continue };
            match file.try_lock() {
                // Nobody holds it: its process is gone.
                Ok(()) => {
                    let _ = std::fs::remove_file(entry.path());
                }
                Err(TryLockError::WouldBlock) => {
                    let mut text = String::new();
                    let _ = file.read_to_string(&mut text);
                    // One a newer afkconvoy wrote holds its place all the same.
                    let ticket = serde_json::from_str(&text).unwrap_or_else(|_| Ticket {
                        number,
                        pid: 0,
                        command: "(unreadable)".to_string(),
                        dir: String::new(),
                        issue: None,
                        asked: 0,
                        ran: None,
                    });
                    live.push(Ticket { number, ..ticket });
                }
                Err(TryLockError::Error(_)) => {}
            }
        }
        Ok(in_order(live))
    }

    /// The tickets of live processes, in the order they run: the first runs,
    /// or is about to, and the rest wait. A queue nobody used is empty.
    pub fn standing(&self) -> Result<Vec<Ticket>> {
        if !self.dir.exists() {
            return Ok(vec![]);
        }
        let _held = self.lock()?;
        self.live()
    }

    /// Puts `ticket` at the end of the queue → it, held for as long as what
    /// this answers lives, with how many are ahead of it.
    pub fn join(&self, ticket: Ticket) -> Result<(Held<'_>, usize)> {
        let _held = self.lock()?;
        let live = self.live()?;
        let numbers: Vec<u64> = live.iter().map(|ticket| ticket.number).collect();
        let ticket = Ticket { number: next_number(&numbers), ..ticket };
        let path = self.dir.join(format!("{:020}{SUFFIX}", ticket.number));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        file.lock().with_context(|| format!("cannot lock {}", path.display()))?;
        let held = Held { queue: self, file, path, ticket };
        held.write()?;
        let ahead = ahead(&live, held.ticket.number);
        Ok((held, ahead))
    }
}

/// A ticket in the queue, held by this process: dropped, it leaves the queue.
pub struct Held<'a> {
    queue: &'a Queue,
    file: File,
    path: PathBuf,
    pub ticket: Ticket,
}

impl Held<'_> {
    fn write(&self) -> Result<()> {
        let text = serde_json::to_string(&self.ticket)?;
        self.file.set_len(0)?;
        self.file
            .write_all_at(text.as_bytes(), 0)
            .with_context(|| format!("cannot write {}", self.path.display()))
    }

    /// How many live tickets are ahead of this one.
    pub fn ahead(&self) -> Result<usize> {
        let _held = self.queue.lock()?;
        Ok(ahead(&self.queue.live()?, self.ticket.number))
    }

    /// Waits until no live ticket is ahead of this one.
    pub fn wait(&self) -> Result<()> {
        while self.ahead()? > 0 {
            std::thread::sleep(POLL);
        }
        Ok(())
    }

    /// Says in the ticket that its command runs from `now`.
    pub fn begin(&mut self, now: u64) -> Result<()> {
        let _held = self.queue.lock()?;
        self.ticket.ran = Some(now);
        self.write()
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        // Removed while still locked, and so never taken for a dead one's:
        // with no need of the queue's lock, which a failed `join` still holds.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Since when the worker in the worktree `dir` has waited in this host's
/// queue, or run what it waited for; `None`: it has nothing there. Soft: a
/// queue that cannot be read holds nothing.
pub fn queued_in(dir: &Path) -> Option<u64> {
    let live = Queue::of_host().and_then(|queue| queue.standing()).ok()?;
    queued_since(&live, &canonical(dir))
}

/// `dir` as a ticket names it: whole, with no link in it.
fn canonical(dir: &Path) -> String {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()).to_string_lossy().into_owned()
}

/// `afkconvoy exclusive -- <command>`: runs `command` once no other
/// `exclusive` command runs on this host, waiting its turn until then → its
/// exit status. What it prints is the command's; the queue's own words go to
/// stderr and to the tick log of this repo. It fails before the command runs
/// with one JSON object and exit 125, and with 127 for a command that cannot be
/// started.
pub fn exclusive(command: &[OsString], now: impl Fn() -> u64) -> ExitCode {
    let shown: Vec<String> =
        command.iter().map(|word| word.to_string_lossy().into_owned()).collect();
    let dir = match Repo::here() {
        Ok(repo) => repo.dir,
        Err(_) => std::env::current_dir().unwrap_or_default(),
    };
    let ticket = Ticket {
        number: 0,
        pid: std::process::id(),
        command: shown.join(" "),
        dir: canonical(&dir),
        issue: std::env::var(ISSUE_ENV).ok().and_then(|issue| issue.parse().ok()),
        asked: now(),
        ran: None,
    };
    let acting = Acting::new("exclusive").on_own_issue();
    let queue = match Queue::of_host() {
        Ok(queue) => queue,
        Err(error) => return failed(&format!("{error:#}")),
    };
    let (mut held, waiting) = match queue.join(ticket) {
        Ok(joined) => joined,
        Err(error) => return failed(&format!("{error:#}")),
    };
    if waiting > 0 {
        let line = waits_line(&held.ticket, waiting);
        eprintln!("[afkconvoy] exclusive: {line}");
        acting.say(now(), line);
        if let Err(error) = held.wait() {
            return failed(&format!("{error:#}"));
        }
    }
    let started = now();
    if let Err(error) = held.begin(started) {
        return failed(&format!("{error:#}"));
    }
    acting.say(started, runs_line(&held.ticket, started));
    let status = Command::new(&command[0]).args(&command[1..]).status();
    let ticket = held.ticket.clone();
    // Its place is free the moment the command ends.
    drop(held);
    let (code, ended) = match status {
        Ok(status) => ended(status),
        Err(error) => {
            eprintln!("[afkconvoy] exclusive: `{}` could not be started: {error}", shown[0]);
            (127, "could not be started".to_string())
        }
    };
    acting.say(now(), ended_line(&ticket, &ended, now()));
    ExitCode::from(code)
}

/// The exit code a command that ended with `status` is passed on with — a
/// shell's for one a signal ended, 128 and the signal — and how it ended, in
/// words.
fn ended(status: ExitStatus) -> (u8, String) {
    match (status.code(), status.signal()) {
        (Some(code), _) => ((code & 0xff) as u8, format!("exit {code}")),
        (None, Some(signal)) => {
            ((128 + signal).clamp(0, 255) as u8, format!("killed by signal {signal}"))
        }
        (None, None) => (1, "ended unexplained".to_string()),
    }
}

/// A failure of the queue's own, before the command ran.
fn failed(message: &str) -> ExitCode {
    println!("{}", json!({ "error": format!("exclusive: {message}; the command did not run") }));
    ExitCode::from(125)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(command: &str) -> Ticket {
        Ticket {
            number: 0,
            pid: std::process::id(),
            command: command.into(),
            dir: "/w/issue-7".into(),
            issue: Some(7),
            asked: 100,
            ran: None,
        }
    }

    fn commands(queue: &Queue) -> Vec<String> {
        queue.standing().unwrap().into_iter().map(|ticket| ticket.command).collect()
    }

    #[test]
    fn tickets_run_in_the_order_they_asked_and_each_leaves_as_it_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::at(dir.path());
        assert_eq!(commands(&queue), Vec::<String>::new());
        let (first, ahead_first) = queue.join(ticket("a")).unwrap();
        let (second, ahead_second) = queue.join(ticket("b")).unwrap();
        let (third, _) = queue.join(ticket("c")).unwrap();
        assert_eq!((ahead_first, ahead_second), (0, 1));
        assert_eq!(commands(&queue), ["a", "b", "c"]);
        assert_eq!((second.ahead().unwrap(), third.ahead().unwrap()), (1, 2));
        drop(first);
        assert_eq!((second.ahead().unwrap(), third.ahead().unwrap()), (0, 1));
        assert_eq!(commands(&queue), ["b", "c"]);
        drop(second);
        third.wait().unwrap();
        drop(third);
        assert_eq!(commands(&queue), Vec::<String>::new());
    }

    #[test]
    fn what_a_dead_process_left_holds_no_place_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::at(dir.path());
        // A ticket nobody holds locked: its process died without removing it.
        let dead = dir.path().join(format!("{:020}{SUFFIX}", 1));
        std::fs::write(&dead, serde_json::to_string(&Ticket { number: 1, ..ticket("a") }).unwrap())
            .unwrap();
        let (waiter, ahead) = queue.join(ticket("b")).unwrap();
        assert_eq!(ahead, 0, "it runs at once");
        assert_eq!(waiter.ticket.number, 1, "the dead one's place is free");
        assert!(dir.path().join(format!("{:020}{SUFFIX}", 1)).exists(), "the waiter's own file");
        assert_eq!(commands(&queue), ["b"]);
    }

    #[test]
    fn a_running_ticket_says_since_when() {
        let dir = tempfile::tempdir().unwrap();
        let queue = Queue::at(dir.path());
        let (mut held, _) = queue.join(ticket("a")).unwrap();
        held.begin(130).unwrap();
        assert_eq!(queue.standing().unwrap()[0].ran, Some(130));
        assert_eq!(queued_since(&queue.standing().unwrap(), "/w/issue-7"), Some(100));
    }

    #[test]
    fn an_ended_command_is_passed_on_as_a_shell_would() {
        let run = |line: &str| Command::new("sh").args(["-c", line]).status().unwrap();
        assert_eq!(ended(run("exit 3")), (3, "exit 3".to_string()));
        assert_eq!(ended(run("kill -9 $$")), (137, "killed by signal 9".to_string()));
    }
}

//! What a landing worktree last heard from its turn (`land --issue`), kept in
//! its own git dir: the process asking for one now, or the outcome the last
//! one answered. The tick reads it to tell a landing worker changing the tree
//! from one whose turn only waits, and asks for that turn again itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use afkconvoy_core::candidate::Process;
use afkconvoy_core::ending::Ending;
use afkconvoy_core::landing_worker::Answered;
use afkconvoy_core::stopped::Progress;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::Repo;
use crate::orca::{TURN_FILE, TURN_LOG_FILE, Worktree};

#[derive(Serialize, Deserialize)]
struct Note {
    /// The process asking for a turn now: its answer is not written yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asking: Option<Process>,
    /// What the last turn answered: its `outcome`, or `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    /// Only a fleet-asked turn needs its answer handed to the worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handoff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer: Option<Value>,
    #[serde(default)]
    told: Option<u64>,
    at: u64,
}

const HANDOFF: &str = "AFKCONVOY_TURN_HANDOFF";

fn note_in(repo: &Repo) -> Result<PathBuf> {
    Ok(repo.git_dir()?.join(TURN_FILE))
}

fn write(file: &Path, note: &Note) -> Result<()> {
    std::fs::write(file, serde_json::to_vec(note)?)
        .with_context(|| format!("cannot write {}", file.display()))
}

/// Runs the turn `ask` for the checkout `repo`, noting first that this
/// process asks for it, then what it answered — `error` when it failed. A
/// note that cannot be written leaves the turn as it was.
pub fn noted(repo: &Repo, now: u64, ask: impl FnOnce() -> Result<Value>) -> Result<Value> {
    let file = note_in(repo).ok();
    let asking = crate::candidates::identity(std::process::id()).ok().flatten();
    let handoff = std::env::var(HANDOFF).ok();
    if let Some(file) = &file {
        let _ = write(
            file,
            &Note {
                asking,
                outcome: None,
                handoff: handoff.clone(),
                answer: None,
                told: None,
                at: now,
            },
        );
    }
    let answer = ask();
    if let Some(file) = &file {
        let outcome = match &answer {
            Ok(answer) => answer["outcome"].as_str().unwrap_or("error").to_string(),
            Err(_) => "error".to_string(),
        };
        let _ = write(
            file,
            &Note {
                asking: None,
                outcome: Some(outcome),
                handoff,
                answer: answer.as_ref().ok().cloned(),
                told: None,
                at: crate::clock(),
            },
        );
    }
    answer
}

fn note(worktree: &Worktree) -> Result<Option<Note>> {
    let Ok(bytes) = std::fs::read(worktree.note(TURN_FILE)?) else { return Ok(None) };
    Ok(serde_json::from_slice(&bytes).ok())
}

/// A fleet-asked answer that requires changes, kept until a harness takes it.
pub(crate) struct Handoff {
    note: Note,
    pub line: String,
    pub mark: String,
}

/// Reads a pending handoff without considering silence or the tree the fleet
/// itself changed. The ending's instructions have their one home in `Ending`.
pub(crate) fn pending(worktree: &Worktree) -> Result<Option<Handoff>> {
    let Some(note) = note(worktree)? else { return Ok(None) };
    if note.told.is_some() || note.asking.is_some() {
        return Ok(None);
    }
    let Some(id) = &note.handoff else { return Ok(None) };
    let ending =
        Ending::ALL.into_iter().find(|ending| Some(ending.word()) == note.outcome.as_deref());
    let Some(ending @ (Ending::Conflict | Ending::SpeculativeConflict | Ending::GateRed)) = ending
    else {
        return Ok(None);
    };
    let mark = format!("turn-answer:{id}");
    let log = worktree.note(TURN_LOG_FILE)?;
    let line = format!(
        "The turn the fleet asked for answered `{}` ({mark}). {}. Full answer: {}. Answer: {}. After fixing it, run `afkconvoy land --issue` with your issue number again.",
        ending.word(),
        ending.told(),
        log.display(),
        note.answer.as_ref().unwrap_or(&Value::Null)
    );
    Ok(Some(Handoff { note, line, mark }))
}

impl Handoff {
    /// A new turn may have started as soon as the worker took the line. Never
    /// replace that newer note with this turn's delivery receipt.
    pub fn taken(mut self, worktree: &Worktree, now: u64) -> Result<()> {
        let current = note(worktree)?;
        if current.as_ref().is_none_or(|current| current.handoff == self.note.handoff) {
            self.note.told = Some(now);
            write(&worktree.note(TURN_FILE)?, &self.note)?;
        }
        Ok(())
    }
}

/// Once handed work, this claim takes a slot until the worker asks again,
/// even if its harness still reports the idle state before delivery.
pub(crate) fn handed(worktree: &Worktree) -> Result<Option<u64>> {
    Ok(note(worktree)?.and_then(|note| note.told))
}

/// Ordinary starts and nudges forget a wait; a handoff remains until the
/// worker asks for its next turn (or the operator gives different work).
pub(crate) fn forget_wait(worktree: &Worktree) -> Result<()> {
    if pending(worktree)?.is_none() && handed(worktree)?.is_none() {
        worktree.forget_turn()?;
    }
    Ok(())
}

/// A fleet-asked turn's merge and file writes are no worker sign of life.
/// Only later commits or touches count while that answer is current.
pub(crate) fn worker_progress(worktree: &Worktree, progress: &mut Progress) -> Result<()> {
    if let Some(note) = note(worktree)?.filter(|note| note.handoff.is_some()) {
        progress.last_commit = progress.last_commit.filter(|at| *at > note.at);
        progress.touched = progress.touched.filter(|at| *at > note.at);
    }
    Ok(())
}

/// What the worktree of a landing worker last heard from its turn: one whose
/// asking process is gone answered nothing.
pub fn read(worktree: &Worktree) -> Result<Answered> {
    let Ok(bytes) = std::fs::read(worktree.note(TURN_FILE)?) else {
        return Ok(Answered::Never);
    };
    let Ok(note) = serde_json::from_slice::<Note>(&bytes) else {
        return Ok(Answered::Never);
    };
    if let Some(asking) = note.asking {
        let alive = asking.host == crate::this_host()
            && crate::candidates::identity(asking.pid)?.as_ref() == Some(&asking);
        return Ok(if alive { Answered::Asking } else { Answered::Said("error".to_string()) });
    }
    Ok(note.outcome.map_or(Answered::Never, Answered::Said))
}

/// A landing worker obeying its turn waits on no person: a live turn or a
/// last answer that waits needs neither worker health nor a silence finding.
/// An unreadable note grants no exemption; a new worker or delivered
/// instruction forgets it.
pub fn waiting(worktree: &Worktree) -> bool {
    read(worktree).is_ok_and(|answered| matches!(answered, Answered::Asking) || answered.waits())
}

/// Asks for the turn of `issue` again in `worktree`, starting no harness: an
/// `afkconvoy land --issue` of its own, detached from the tick, whose answer
/// the note takes and the next tick reads. Its output is appended to the
/// worktree's turn log.
pub fn ask_again(worktree: &Worktree, issue: u64) -> Result<()> {
    let log = worktree.note(TURN_LOG_FILE)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("cannot open {}", log.display()))?;
    let program = std::env::current_exe().context("cannot find this afkconvoy")?;
    let mut child = crate::process::spawn(
        Command::new(program)
            .args(["land", "--issue", &issue.to_string()])
            .env(HANDOFF, uuid::Uuid::new_v4().to_string())
            .current_dir(&worktree.path)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log),
    )
    .context("cannot ask for the turn again")?;
    // Reaped once it ends; nothing waits on its answer here.
    std::thread::spawn(move || child.wait());
    Ok(())
}

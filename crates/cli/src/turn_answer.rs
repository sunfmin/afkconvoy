//! What a landing worktree last heard from its turn (`land --issue`), kept in
//! its own git dir: the process asking for one now, or the outcome the last
//! one answered. The tick reads it to tell a landing worker changing the tree
//! from one whose turn only waits, and asks for that turn again when its
//! ending calls for one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use afkconvoy_core::candidate::Process;
use afkconvoy_core::ending::Ending;
use afkconvoy_core::landing_worker::Answered;
use afkconvoy_core::stopped::Progress;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::gh::BlockedBy;
use crate::git::Repo;
use crate::orca::{TURN_FILE, TURN_LOG_FILE, Worktree};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Dependency {
    number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
}

fn dependency_list(edges: &[BlockedBy]) -> Vec<Dependency> {
    let mut dependencies: Vec<_> = edges
        .iter()
        .map(|edge| Dependency { number: edge.number, repo: edge.repo.clone() })
        .collect();
    dependencies.sort();
    dependencies.dedup();
    dependencies
}

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
    /// Every dependency after a blocked answer, open or closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dependencies: Option<Vec<Dependency>>,
    at: u64,
}

const HANDOFF: &str = "AFKCONVOY_TURN_HANDOFF";
const RECEIPT: &str = "convoy-turn-told.json";

#[derive(Serialize, Deserialize)]
struct Receipt {
    handoff: String,
    at: u64,
}

fn note_in(repo: &Repo) -> Result<PathBuf> {
    Ok(repo.git_dir()?.join(TURN_FILE))
}

fn write(file: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = file.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec(value)?)
        .and_then(|()| std::fs::rename(&temporary, file))
        .with_context(|| format!("cannot write {}", file.display()))
}

/// Runs the turn `ask` for the checkout `repo`, noting first that this
/// process asks for it, then what it answered — `error` when it failed. A
/// blocked answer also keeps every dependency of `issue`, open or closed. A
/// note that cannot be written leaves the turn as it was.
pub fn noted(
    repo: &Repo,
    issue: Option<u64>,
    now: u64,
    ask: impl FnOnce() -> Result<Value>,
) -> Result<Value> {
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
                dependencies: None,
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
        let dependencies = issue.filter(|_| outcome == Ending::Blocked.word()).and_then(|issue| {
            let gh = crate::gh::GitHub::of(repo).ok()?;
            let edges = gh.blocked_by(issue).ok()?;
            Some(dependency_list(&edges))
        });
        let _ = write(
            file,
            &Note {
                asking: None,
                outcome: Some(outcome),
                handoff,
                answer: answer.as_ref().ok().cloned(),
                dependencies,
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
    if note.asking.is_some() || handed(worktree)?.is_some() {
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
    /// A new turn may start as soon as the worker takes the line. Keep the
    /// receipt separately, keyed to this answer, so it cannot overwrite the
    /// newer turn's note or give that turn a stale delivery receipt.
    pub fn taken(self, worktree: &Worktree, now: u64) -> Result<()> {
        let handoff = self.note.handoff.context("a handoff has an identity")?;
        write(&worktree.note(RECEIPT)?, &Receipt { handoff, at: now })
    }
}

/// Once handed work, this claim takes a slot until the worker asks again,
/// even if its harness still reports the idle state before delivery.
pub(crate) fn handed(worktree: &Worktree) -> Result<Option<u64>> {
    let Some(handoff) = note(worktree)?.and_then(|note| note.handoff) else { return Ok(None) };
    let receipt = std::fs::read(worktree.note(RECEIPT)?)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).ok());
    Ok(receipt.filter(|receipt| receipt.handoff == handoff).map(|receipt| receipt.at))
}

/// Ordinary starts and nudges forget a wait; a handoff remains until the
/// worker asks for its next turn (or the operator gives different work).
pub(crate) fn forget_wait(worktree: &Worktree) -> Result<()> {
    if pending(worktree)?.is_none() && handed(worktree)?.is_none() {
        worktree.clear_turn()?;
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
    let Some(note) = note(worktree)? else { return Ok(Answered::Never) };
    if let Some(asking) = note.asking {
        let alive = asking.host == crate::this_host()
            && crate::candidates::identity(asking.pid)?.as_ref() == Some(&asking);
        return Ok(if alive { Answered::Asking } else { Answered::Said("error".to_string()) });
    }
    Ok(note.outcome.map_or(Answered::Never, Answered::Said))
}

fn unchanged(note: &Note, current: &[BlockedBy]) -> bool {
    if note.outcome.as_deref() != Some(Ending::Blocked.word()) {
        return false;
    }
    let Some(answer) = &note.answer else { return false };
    let Ok(mut answered) = serde_json::from_value::<Vec<u64>>(answer["blocked_by"].clone()) else {
        return false;
    };
    answered.sort_unstable();
    let mut open: Vec<_> = current
        .iter()
        .filter(|edge| edge.state.eq_ignore_ascii_case("open"))
        .map(|edge| edge.number)
        .collect();
    open.sort_unstable();
    if answered != open {
        return false;
    }
    let Some(recorded) = &note.dependencies else { return true };
    let mut recorded = recorded.clone();
    recorded.sort();
    recorded.dedup();
    recorded == dependency_list(current)
}

/// Whether a blocked answer's open blockers and full dependency list are
/// exactly as that answer left them. Older notes have no full-list snapshot;
/// their recorded open blockers still hold the turn.
pub(crate) fn blocked_unchanged(worktree: &Worktree, current: &[BlockedBy]) -> Result<bool> {
    Ok(note(worktree)?.is_some_and(|note| unchanged(&note, current)))
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

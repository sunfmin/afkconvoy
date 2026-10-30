//! What a landing worktree last heard from its turn (`land --issue`), kept in
//! its own git dir: the process asking for one now, or the outcome the last
//! one answered. The tick reads it to tell a landing worker changing the tree
//! from one whose turn only waits, and asks for that turn again itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use afkconvoy_core::candidate::Process;
use afkconvoy_core::landing_worker::Answered;
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
    at: u64,
}

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
    if let Some(file) = &file {
        let _ = write(file, &Note { asking, outcome: None, at: now });
    }
    let answer = ask();
    if let Some(file) = &file {
        let outcome = match &answer {
            Ok(answer) => answer["outcome"].as_str().unwrap_or("error").to_string(),
            Err(_) => "error".to_string(),
        };
        let _ = write(file, &Note { asking: None, outcome: Some(outcome), at: crate::clock() });
    }
    answer
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

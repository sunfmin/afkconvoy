//! Writing an issue's status board: one comment, found by its marker and
//! edited in place, rewritten only when its text changed.

use std::path::PathBuf;

use afkconvoy_core::blocked::Parkable;
use afkconvoy_core::board::{Board, Stage, board_comment, render};
use afkconvoy_core::claims::Claim;
use afkconvoy_core::left::Kept;
use afkconvoy_core::tick::Outcome;
use anyhow::{Context, Result};

use crate::VERSION;
use crate::claims::Me;
use crate::fleet::Fleet;
use crate::judge::mine;

/// Where this checkout remembers the board it last wrote for an issue: the
/// claim it was written under, then the text. It only spares a tick the read
/// of an issue whose board has not changed — what a board says is always
/// rendered, never taken from here or from GitHub.
fn remembered(fleet: &Fleet, issue: u64) -> Result<PathBuf> {
    Ok(fleet.repo.common_dir()?.join("convoy-boards").join(issue.to_string()))
}

/// Brings the status board of `claim`'s issue to what `board` renders → whether
/// anything was written. No two claims of an issue share a commit, and a
/// takeover moves a claim to a new one, so a board remembered under another
/// claim of the issue, or another holder of this one, is never taken for it.
pub fn post(fleet: &Fleet, claim: &Claim, board: &Board) -> Result<bool> {
    let issue = claim.number;
    let text = render(board);
    let memo = format!("{}\n{text}", claim.sha);
    let file = remembered(fleet, issue)?;
    if std::fs::read_to_string(&file).is_ok_and(|last| last == memo) {
        return Ok(false);
    }
    let comments = fleet.gh.comments(issue)?;
    let found = board_comment(comments.iter().map(|c| c.body.as_str()));
    let written = match found.and_then(|at| comments.get(at)) {
        Some(comment) if comment.body.trim() == text.trim() => false,
        Some(comment) => {
            fleet.gh.edit_comment(comment.id, &text)?;
            true
        }
        None => {
            fleet.gh.comment(issue, &text)?;
            true
        }
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    std::fs::write(&file, memo).with_context(|| format!("cannot write {}", file.display()))?;
    Ok(written)
}

/// The last word of a claim's board, written before the claim is released:
/// how it ended. The instance named is the claim's own.
pub fn last_word(
    fleet: &Fleet,
    claim: &Claim,
    stage: Stage,
    pr: Option<u64>,
    blocked_by: &[u64],
) -> Result<()> {
    let instance = instance_of(claim);
    post(
        fleet,
        claim,
        &Board {
            instance,
            version: VERSION,
            stage,
            pr,
            blocked_by,
            cut: false,
            waits_at: None,
            answer: None,
            kept: None,
        },
    )?;
    Ok(())
}

/// The last word of a claim that was parked as `parkable` says: `kept` is
/// its worktree, when that is kept for what only it holds.
pub fn parked(fleet: &Fleet, claim: &Claim, parkable: &Parkable, kept: Option<Kept>) -> Result<()> {
    let blocked_by = &parkable.blockers();
    let (instance, stage) = (instance_of(claim), Stage::Parked);
    let answer = None;
    post(
        fleet,
        claim,
        &Board {
            instance,
            version: VERSION,
            stage,
            pr: None,
            blocked_by,
            cut: parkable.cut(),
            waits_at: None,
            answer,
            kept,
        },
    )?;
    Ok(())
}

/// The last word of a claim whose stage declared. `waits_at` is the stage
/// the verdict left the issue at, when that is a human's: the board says who
/// the issue waits for.
pub fn declared(
    fleet: &Fleet,
    claim: &Claim,
    pr: Option<u64>,
    waits_at: Option<&str>,
) -> Result<()> {
    let (instance, stage) = (instance_of(claim), Stage::Declared);
    post(
        fleet,
        claim,
        &Board {
            instance,
            version: VERSION,
            stage,
            pr,
            blocked_by: &[],
            cut: false,
            waits_at,
            answer: None,
            kept: None,
        },
    )?;
    Ok(())
}

/// The last word of a claim that was escalated: `answer` is the lines that
/// answer the escalation (`waiting::Waits::answer_lines`), which the board
/// ends with as the comment that says why does. `kept` is its worktree, when
/// that is kept for what only it holds.
pub fn escalated(
    fleet: &Fleet,
    claim: &Claim,
    pr: Option<u64>,
    answer: &str,
    kept: Option<Kept>,
) -> Result<()> {
    let (instance, stage) = (instance_of(claim), Stage::Escalated);
    let answer = Some(answer);
    post(
        fleet,
        claim,
        &Board {
            instance,
            version: VERSION,
            stage,
            pr,
            blocked_by: &[],
            cut: false,
            waits_at: None,
            answer,
            kept,
        },
    )?;
    Ok(())
}

fn instance_of(claim: &Claim) -> &str {
    claim.owner.as_ref().map(|owner| owner.instance.as_str()).unwrap_or("unknown")
}

/// The tick's step: show a claim of mine in `stage`. A claim that is no
/// longer mine — the tick ended it, a peer took it — is left alone: its board
/// is whoever's holds it now.
pub fn show(fleet: &Fleet, me: &Me, issue: u64, stage: Stage, pr: Option<u64>) -> Result<Outcome> {
    let Some(claim) = mine(fleet, me, issue).ok().flatten() else {
        return Ok(Outcome::Nothing);
    };
    let instance = me.instance.as_str();
    let board = Board {
        instance,
        version: VERSION,
        stage,
        pr,
        blocked_by: &[],
        cut: false,
        waits_at: None,
        answer: None,
        kept: None,
    };
    Ok(if post(fleet, &claim, &board)? { Outcome::Done } else { Outcome::Nothing })
}

//! Writing an issue's status board: one comment, found by its marker and
//! edited in place, rewritten only when its text changed.

use std::path::PathBuf;

use afkconvoy_core::board::{Board, Stage, board_comment, render};
use afkconvoy_core::claims::Claim;
use afkconvoy_core::tick::Outcome;
use anyhow::{Context, Result};

use crate::claims::Me;
use crate::fleet::Fleet;
use crate::judge::mine;

/// Where this checkout remembers the board it last wrote for an issue: the
/// claim it was written under, then the text. It only spares a tick the read
/// of an issue whose board has not changed — what a board says is always
/// rendered, never taken from here or from GitHub.
fn remembered(fleet: &Fleet, issue: u64) -> Result<PathBuf> {
    let common = fleet.repo.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Ok(PathBuf::from(common).join("convoy-boards").join(issue.to_string()))
}

/// Brings the status board of `claim`'s issue to what `board` renders → whether
/// anything was written. A claim ref is written once, so a board remembered
/// under another claim of the issue is never taken for this one's.
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
    let instance = claim.owner.as_ref().map(|owner| owner.instance.as_str()).unwrap_or("unknown");
    post(fleet, claim, &Board { instance, stage, pr, blocked_by })?;
    Ok(())
}

/// The tick's step: show a claim of mine in `stage`. A claim that is no
/// longer mine — the tick ended it, a peer took it — is left alone: its board
/// is whoever's holds it now.
pub fn show(fleet: &Fleet, me: &Me, issue: u64, stage: Stage, pr: Option<u64>) -> Result<Outcome> {
    let Some(claim) = mine(fleet, me, issue).ok().flatten() else {
        return Ok(Outcome::Nothing);
    };
    let board = Board { instance: me.instance.as_str(), stage, pr, blocked_by: &[] };
    Ok(if post(fleet, &claim, &board)? { Outcome::Done } else { Outcome::Nothing })
}

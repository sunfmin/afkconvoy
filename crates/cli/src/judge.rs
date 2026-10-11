//! A claim with no PR: asking after its worker, and the transitions that end
//! it when the worker stopped — `close` and `escalate`.

use std::time::UNIX_EPOCH;

use afkconvoy_core::claims::{Claim, releasable};
use afkconvoy_core::stopped::{Progress, Route, Workless, route};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition;
use afkconvoy_core::transition::{
    CloseStep, EscalateStep, Escalation, Released, escalation_begun, escalation_comment, relabelled,
};
use afkconvoy_core::verdict::latest_verdict;
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::claims::{Me, delete};
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::orca::{Source, Worktree};

/// A worktree's git progress against the base branch's tip on the remote —
/// never the local branch of that name, against which a worktree freshly cut
/// from the remote would read as ahead with no work in it.
pub fn progress(worktree: &Worktree, base_tip: &str) -> Result<Progress> {
    let repo = Repo { dir: worktree.path.clone() };
    let ahead = repo.try_git(&["rev-list", "--count", &format!("{base_tip}..HEAD")])?;
    let committed = repo.try_git(&["log", "-1", "--format=%ct"])?;
    let dirty = !repo.git_raw(&["status", "--porcelain"])?.trim().is_empty();
    // What git would stage, as the worker left it: an ignored build directory says nothing.
    let listed = repo.git_raw(&["ls-files", "-z", "--cached", "--others", "--exclude-standard"])?;
    let touched = listed
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::fs::symlink_metadata(repo.dir.join(path)).ok()?.modified().ok())
        .filter_map(|at| Some(at.duration_since(UNIX_EPOCH).ok()?.as_secs()))
        .max();
    Ok(Progress {
        commits_ahead: ahead.stdout.parse().ok().filter(|_| ahead.ok),
        dirty,
        last_commit: committed.stdout.parse().ok().filter(|_| committed.ok),
        touched,
    })
}

/// My claim on `issue`: `None` when the issue has none — already gone counts
/// as released. Every transition that ends a claim refuses another's.
fn mine(fleet: &Fleet, me: &Me, issue: u64) -> Result<Option<Claim>> {
    Ok(releasable(fleet.scan.claim_of(issue), &me.instance)?.cloned())
}

fn nothing_held(issue: u64) -> Value {
    json!({"issue": issue, "released": true, "detail": "no claim is held on it: nothing was done"})
}

/// The worktree of `issue` on this machine, and the proof that its branch
/// holds no work when it does not.
fn worktree_and_work(fleet: &Fleet, issue: u64) -> Result<(Option<Worktree>, Option<Workless>)> {
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    let workless = match &worktree {
        Some(worktree) => Workless::of(&progress(worktree, &fleet.base_tip()?)?),
        None => None,
    };
    Ok((worktree, workless))
}

/// `afkconvoy escalate`: hand one of my claims to a human. The steps already
/// taken are read from GitHub and the rest are done in order — comment the
/// reason, swap `ready_label` for `escalate_label`, release the claim last —
/// so it can be run again from wherever it was cut, and posts one comment.
/// The PR, the branch and the worktree are kept, except a worktree whose
/// branch holds no work.
pub fn escalate(fleet: &mut Fleet, me: &Me, issue: u64, reason: &str) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    escalate_claim(fleet, &claim, reason, worktree.as_ref(), workless.as_ref())?;
    Ok(json!({"issue": issue, "action": "escalated", "released": true,
        "worktree": if workless.is_some() { "removed" } else { "kept" }}))
}

fn escalate_claim(
    fleet: &mut Fleet,
    claim: &Claim,
    reason: &str,
    worktree: Option<&Worktree>,
    workless: Option<&Workless>,
) -> Result<Released> {
    let issue = claim.number;
    let labels = fleet.gh.issue(issue)?.labels;
    let comments = fleet.gh.comments(issue)?;
    let said = escalation_begun(comments.iter().map(|c| c.body.as_str()), &claim.sha);
    let begun = Escalation::read(said, relabelled(&labels, &fleet.config));
    let Fleet { repo, gh, scan, config, .. } = fleet;
    transition::escalate(issue, begun, workless, |step| {
        match step {
            EscalateStep::Comment => gh.comment(issue, &escalation_comment(&claim.sha, reason))?,
            EscalateStep::Relabel => {
                if !labels.contains(&config.escalate_label) {
                    gh.add_label(issue, &config.escalate_label)?;
                }
                if labels.contains(&config.ready_label) {
                    gh.remove_label(issue, &config.ready_label)?;
                }
            }
            EscalateStep::Release => {
                delete(repo, scan, issue, &claim.sha)?;
            }
            EscalateStep::RemoveWorktree => {
                if let Some(worktree) = worktree {
                    worktree.remove()?;
                }
            }
        }
        Ok(())
    })
}

/// `afkconvoy close`: close one of my claims whose issue needed no change —
/// close the issue → release → remove the worktree. Refused unless the
/// worktree here shows a branch with no commit past the base and nothing
/// uncommitted.
pub fn close(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    let Some(workless) = workless else {
        bail!(
            "issue #{issue} is not closed as needing no change: {}; nothing was changed",
            match worktree {
                Some(_) => "its branch holds work (commits past the base, or uncommitted files)",
                None => "it has no worktree on this host to show its branch holds no work",
            }
        );
    };
    close_claim(fleet, &claim, worktree.as_ref(), &workless)?;
    Ok(json!({"issue": issue, "action": "closed", "released": true}))
}

fn close_claim(
    fleet: &mut Fleet,
    claim: &Claim,
    worktree: Option<&Worktree>,
    workless: &Workless,
) -> Result<Released> {
    let issue = claim.number;
    let Fleet { repo, gh, scan, .. } = fleet;
    transition::close(issue, workless, |step| {
        match step {
            CloseStep::CloseIssue => gh.close_issue(issue)?,
            CloseStep::Release => {
                delete(repo, scan, issue, &claim.sha)?;
            }
            CloseStep::RemoveWorktree => {
                if let Some(worktree) = worktree {
                    worktree.remove()?;
                }
            }
        }
        Ok(())
    })
}

/// The tick's look at one PR-less claim of mine. A busy worker is left with no
/// GitHub read; for one that stopped, its worktree's progress and its latest
/// verdict are gathered and routed (`stopped::route`) → `Done` when that ended
/// the claim.
pub fn judge(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Outcome> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(Outcome::Nothing) };
    let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)? else {
        return Ok(Outcome::Nothing);
    };
    let state = worktree.state(fleet.now)?;
    if !matches!(state, WorkerState::Idle { .. }) {
        return Ok(Outcome::Nothing);
    }
    let progress = progress(&worktree, &fleet.base_tip()?)?;
    let comments = fleet.gh.comments(issue)?;
    let verdict = latest_verdict(comments.iter().map(|c| c.body.as_str()));
    match route(state, &progress, verdict.as_ref(), fleet.now) {
        Route::Leave => Ok(Outcome::Nothing),
        Route::Close(workless) => {
            close_claim(fleet, &claim, Some(&worktree), &workless)?;
            Ok(Outcome::Done)
        }
        Route::Escalate(why) => {
            let said = verdict.and_then(|verdict| comments.get(verdict.comment));
            let reason = why.reason(said.map_or("(its comment)", |comment| comment.url.as_str()));
            let workless = Workless::of(&progress);
            escalate_claim(fleet, &claim, &reason, Some(&worktree), workless.as_ref())?;
            Ok(Outcome::Done)
        }
    }
}

/// A label as one path segment of a GitHub API URL.
pub fn label_segment(label: &str) -> String {
    let mut out = String::new();
    for byte in label.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

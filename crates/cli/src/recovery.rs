//! What survived a worker that died: the worktree on this host, and the
//! issue's own branches on the remote.

use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::recovery::{Pushed, Recovery, furthest_ahead, own_branches, select_recovery};
use afkconvoy_core::stopped::Progress;
use afkconvoy_core::worker::recorded_branches;
use anyhow::Result;
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::judge::progress;
use crate::orca::{Source, Worktree};
use crate::remote::Fresh;

/// What one issue's dead worker left, and the start it selects.
pub struct Survey {
    pub worktree: Option<Worktree>,
    pub progress: Option<Progress>,
    /// The issue's own branches on the remote, each against the base branch.
    pub candidates: Vec<Pushed>,
    /// The one a worktree would be recreated at.
    pub pushed: Option<Pushed>,
    pub plan: Recovery,
}

/// The issue's own branches the remote has: the ones the fleet recorded on
/// the issue, and the branch of `worktree`. An error when the remote cannot be
/// read — which is never "nothing survived".
pub fn own_heads(fleet: &Fleet, number: u64, worktree: Option<&Worktree>) -> Result<Vec<String>> {
    let comments = fleet.gh.comments(number)?;
    let recorded = recorded_branches(comments.iter().map(|c| c.body.as_str()));
    let heads = fleet.remote.heads(Fresh::Process)?;
    Ok(own_branches(&heads, &recorded, worktree.map(|worktree| worktree.branch.as_str())))
}

/// Reads both signals — always both: a pristine worktree over a branch that
/// carries pushed commits still has something to continue. Reads only.
pub fn survey(fleet: &Fleet, number: u64) -> Result<Survey> {
    let base_tip = fleet.task_tip(number)?;
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, number)?;
    let progress = match &worktree {
        Some(worktree) => Some(progress(worktree, &base_tip)?),
        None => None,
    };
    let mut candidates = vec![];
    for name in own_heads(fleet, number, worktree.as_ref())? {
        let tip = fleet.remote.fetched(&format!("refs/heads/{name}"), Fresh::Process)?;
        let commits_ahead = match tip {
            Some(tip) => {
                let counted =
                    fleet.repo.try_git(&["rev-list", "--count", &format!("{base_tip}..{tip}")])?;
                counted.stdout.parse().ok().filter(|_| counted.ok)
            }
            // gone between the two reads: nothing of it to continue
            None => None,
        };
        candidates.push(Pushed { name, commits_ahead });
    }
    let pushed = furthest_ahead(&candidates).cloned();
    // The issue's open PR, when it has one: what survived is then only joined.
    let joining = closing_pr(&fleet.gh.open_prs()?, number).map(|pr| pr.number);
    let plan = select_recovery(progress.as_ref(), pushed.as_ref(), joining);
    Ok(Survey { worktree, progress, candidates, pushed, plan })
}

/// `afkconvoy recovery`: the tier `dispatch` would continue issue `number`
/// from, without acting.
pub fn recovery(fleet: &Fleet, number: u64) -> Result<Value> {
    let mut found = survey(fleet, number)?;
    if fleet.scan.claim_of(number).is_none() {
        found.plan = found.plan.taken_up();
    }
    let names: Vec<&str> = found.candidates.iter().map(|pushed| pushed.name.as_str()).collect();
    Ok(json!({
        "issue": number,
        "base": fleet.task_branch(number)?,
        "worktree": {
            "present": found.worktree.is_some(),
            "path": found.worktree.as_ref().map(|worktree| worktree.path.clone()),
            "commits_ahead": found.progress.and_then(|progress| progress.commits_ahead),
            "dirty": found.progress.map(|progress| progress.dirty),
        },
        "branch": {
            "name": found.pushed.as_ref().map(|pushed| pushed.name.clone()),
            "candidates": names,
            "commits_ahead": found.pushed.as_ref().and_then(|pushed| pushed.commits_ahead),
        },
        "tier": found.plan.tier.number(),
        "action": found.plan.tier.word(),
        "prompt": found.plan.variant.word(),
        "reason": found.plan.reason,
    }))
}

/// What `afkconvoy recovery --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Before `dispatch` on an issue that was worked before: it shows what `dispatch` would continue from, and so what `--start fresh` would throw away.

## Who runs it

The operator, in a checkout of the repo, on the host the worker ran on — a worktree counts only where it is.

## What it reads and writes

It reads the issue's comments and the open PRs on GitHub, the issue's branches on the remote, and its worktree through orca. It changes nothing.

## What it answers

{"issue", "base", "worktree", "branch", "tier", "action", "prompt", "reason"}

- `worktree`: {present, path, commits_ahead, dirty} — what is on this host
- `branch`: {name, candidates, commits_ahead} — the issue's own branches on the remote, and the one a worktree would be recreated at
- `tier`, `action`, `prompt`, `reason`: what `dispatch` would do, as `dispatch` answers it

Errors: the config, GitHub or the remote cannot be read; the remote has no base branch; there is no such issue (`no such issue: #7 (HTTP 404)`), or GitHub says it was deleted or transferred.

## What trips you up

- It takes no `--instance` and looks at no claim: it answers for an issue nobody holds, and for one another instance holds."#;

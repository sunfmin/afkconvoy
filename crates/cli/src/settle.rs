//! Settling a claim whose work has landed, and finding the landings that were
//! cut short of closing their issue.

use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::landing::{LandedPr, closes_grep};
use afkconvoy_core::plan::{PullRequest, closing_pr};
use afkconvoy_core::settle::{
    Follow, SettleStep, cut_landing, follow, landed_comment, settle_steps,
};
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, bail};

use crate::board::last_word;
use crate::claims::{Me, release};
use crate::fleet::Fleet;
use crate::gh::Tracker;
use crate::git::{REMOTE, Repo};
use crate::landing::joins_in;
use crate::orca::{Ended, Source, Worktree};
use crate::recovery::own_heads;
use crate::remote::{Fresh, Push};

/// The joining on the base branch that landed `issue` of `tracker` — the
/// newest merge commit there a PR joined a train with that closes it — and
/// when it was made. A later commit that only says `Closes #n` is not one.
fn landing_of(
    repo: &Repo,
    tracker: &Tracker,
    base_tip: &str,
    issue: u64,
) -> Result<Option<(LandedPr, u64)>> {
    let grep = format!("--grep={}", closes_grep(tracker, issue));
    let joins = joins_in(repo, tracker, &["-E", "-i", &grep, base_tip])?;
    let Some(join) = joins.into_iter().rfind(|join| join.issue() == Some(issue)) else {
        return Ok(None);
    };
    let at = repo.git(&["log", "-1", "--format=%ct", &join.commit])?.parse()?;
    Ok(Some((join, at)))
}

/// The landing that put the still-open issue of `claim`, of `tracker`, on
/// the base branch, when there is one and it is this claim's
/// (`cut_landing`).
pub fn cut_landing_of(
    repo: &Repo,
    tracker: &Tracker,
    base_tip: &str,
    claim: &Claim,
    open: &[PullRequest],
) -> Result<Option<LandedPr>> {
    let Some((join, at)) = landing_of(repo, tracker, base_tip, claim.number)? else {
        return Ok(None);
    };
    let claimed_at = claim.since;
    let cut = cut_landing(&join, at, claimed_at, closing_pr(open, claim.number));
    Ok(cut.then_some(join))
}

/// Fast-forwards the base branch's LOCAL branch, in the checkout worktrees are
/// cut from — the one a human reads — to the remote's tip, so what the fleet
/// landed is on this machine without anyone pulling. Only ever a fast-forward:
/// a branch that diverged, changes in the way, no such local branch — each
/// leaves everything as it was.
fn follow_base(fleet: &Fleet) -> Result<()> {
    let main = Repo::new(fleet.repo.main_checkout()?);
    let base = &fleet.config.base_branch;
    let local = format!("refs/heads/{base}");
    let before = main.try_git(&["rev-parse", "-q", "--verify", &local])?;
    let Some(tip) = main.remote().fetched(&local, Fresh::Process)?.filter(|_| before.ok) else {
        return Ok(());
    };
    let here = main.try_git(&["symbolic-ref", "-q", "--short", "HEAD"])?.stdout;
    let moved = match follow(&before.stdout, &tip, here == *base) {
        Follow::UpToDate => true,
        Follow::Merge => main.try_git(&["merge", "--ff-only", "--quiet", &tip])?.ok,
        Follow::Fetch => {
            let update = format!("{local}:{local}");
            let fetch = ["fetch", "--quiet", "--no-write-fetch-head", "--no-tags", REMOTE, &update];
            main.try_fetch(&fetch)?.ok
        }
    };
    if moved {
        main.try_git(&["update-ref", &format!("refs/remotes/{REMOTE}/{base}"), &tip])?;
    }
    Ok(())
}

/// Removes the worktree settling `issue` leaves, unless a worker is still in
/// it or it holds a commit the remote does not. The worker the fleet started
/// here, stopped at its prompt, is its own session: it is ended first. A
/// worker still working, one whose end is not yet confirmed, or any other
/// process still in it, makes the settle wait, so the claim stays and a
/// later tick tries again. A commit the remote does not have stays, and the
/// settle goes on: the claim is released and nothing records a failed
/// removal, so a later tick does not take it.
fn remove_settled(fleet: &Fleet, issue: u64, worktree: &Worktree) -> Result<Option<u64>> {
    let wait = || -> Result<Option<u64>> {
        let file = worktree.note("convoy-settle-wait")?;
        let since =
            std::fs::read_to_string(&file).ok().and_then(|s| s.parse().ok()).unwrap_or(fleet.now);
        if !file.exists() {
            std::fs::write(file, since.to_string())?;
        }
        Ok(Some(since))
    };
    if worktree.holds_a_worker() {
        match worktree.state(fleet.now)? {
            WorkerState::Busy => return wait(),
            // One still ending is waited for like one still working: the
            // grace, not this tick, says when that is a finding.
            WorkerState::Idle { .. } => {
                if let Ended::Unconfirmed(_) = worktree.end_settled()? {
                    return wait();
                }
            }
            WorkerState::Gone => {}
        }
    }
    let busy = crate::hot::working_directories()?;
    if crate::hot::occupies(&busy, &worktree.path) {
        return wait();
    }
    if settled_keeps(fleet, issue, worktree)? {
        return Ok(None);
    }
    crate::account::refresh_stopped(&fleet.repo, &fleet.gh, issue)?;
    worktree.remove()?;
    Ok(None)
}

/// Whether settling keeps `worktree`. A worktree git can still read is kept
/// by `settle_keeps`. One git can no longer read — its registration already
/// gone — is kept unless the head orca lists is on the remote.
fn settled_keeps(fleet: &Fleet, issue: u64, worktree: &Worktree) -> Result<bool> {
    let head = Repo::new(worktree.path.clone()).try_git(&["rev-parse", "HEAD"])?;
    if head.ok {
        let leaving = crate::judge::leaving(fleet, issue, worktree)?;
        return Ok(afkconvoy_core::left::settle_keeps(&leaving.left));
    }
    let listed = crate::orca::listed_head(&worktree.path)?;
    Ok(!crate::orca::commit_is_on_remote(
        &fleet.repo,
        &fleet.config.base_branch,
        &worktree.branch,
        &listed,
    ))
}

/// Settles my claim on `issue`, whose work is on the base branch: what only
/// the tick can do for it — `land` runs inside the worktree and holds no
/// instance id. The claim is deleted last, so a settling that failed on the
/// way is the next tick's to finish.
pub fn settle(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Option<u64>> {
    let closed = fleet.gh.issue(issue)?.is_closed();
    let base_tip = fleet.task_tip(issue)?;
    let landed = landing_of(&fleet.repo, &fleet.gh.tracker, &base_tip, issue)?.is_some();
    let comments = fleet.gh.comments(issue)?;
    let accounted = afkconvoy_core::account::left(comments.iter().map(|c| c.body.as_str()));
    if let Some(end) = afkconvoy_core::account::settled_end(landed, accounted) {
        crate::account::leave(&fleet.repo, &fleet.gh, issue, end, fleet.now)?;
    }
    for step in settle_steps(closed) {
        match step {
            SettleStep::CloseIssue => {
                let open = fleet.gh.open_prs()?;
                let claim = fleet.scan.claim_of(issue);
                let landed = match claim {
                    Some(claim) => {
                        cut_landing_of(&fleet.repo, &fleet.gh.tracker, &base_tip, claim, &open)?
                    }
                    None => None,
                };
                if landed.is_none() {
                    bail!("issue #{issue} is open and nothing landed it under this claim");
                }
                fleet.gh.close_issue(issue)?;
            }
            SettleStep::ClosePr => {
                // The base branch itself is asked: a PR whose join commit is not on it is left alone.
                let open = fleet.gh.open_prs()?;
                let landing = landing_of(&fleet.repo, &fleet.gh.tracker, &base_tip, issue)?;
                if let (Some(pr), Some((join, _))) = (closing_pr(&open, issue), landing)
                    && pr.number == join.pr
                {
                    let comment = landed_comment(&join.commit, &fleet.task_branch(issue)?);
                    fleet.gh.close_pr(pr.number, &comment)?;
                }
            }
            // Only what is on the base branch, and only as it was read: a
            // branch its worker pushed to since holds work nothing landed.
            SettleStep::DropBranches => {
                let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
                for branch in own_heads(fleet, issue, worktree.as_ref())? {
                    let head = format!("refs/heads/{branch}");
                    let Some(tip) = fleet.remote.fetched(&head, Fresh::Process)? else { continue };
                    if fleet.repo.try_git(&["merge-base", "--is-ancestor", &tip, &base_tip])?.ok {
                        fleet.remote.push(&Push::delete(&head).leased(&tip))?;
                    }
                }
            }
            SettleStep::RemoveWorktree => {
                if let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?
                    && let Some(since) = remove_settled(fleet, issue, &worktree)?
                {
                    return Ok(Some(since));
                }
            }
            // Soft: a checkout that cannot follow never fails the release it rides on.
            SettleStep::FollowBase => {
                let _ = follow_base(fleet);
            }
            SettleStep::Board => {
                // The base branch itself says whether the work landed.
                let landed = landing_of(&fleet.repo, &fleet.gh.tracker, &base_tip, issue)?;
                let (stage, pr) = match landed {
                    Some((join, _)) => (Stage::Merged, Some(join.pr)),
                    None => (Stage::Closed, None),
                };
                if let Some(claim) = fleet.scan.claim_of(issue) {
                    last_word(fleet, claim, stage, pr, &[])?;
                }
            }
            SettleStep::Release => {
                release(&fleet.repo, &mut fleet.scan, &me.instance, issue)?;
            }
        }
    }
    Ok(None)
}

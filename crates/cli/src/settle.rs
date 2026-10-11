//! Settling a claim whose work has landed, and finding the landings that were
//! cut short of closing their issue.

use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::plan::{PullRequest, closing_pr};
use afkconvoy_core::settle::{
    Follow, SettleStep, cut_landing, follow, landed_comment, settle_steps,
};
use afkconvoy_core::train::Joined;
use anyhow::{Result, bail};

use crate::board::last_word;
use crate::claims::{Me, release};
use crate::fleet::Fleet;
use crate::git::{REMOTE, Repo};
use crate::orca::{Source, Worktree};
use crate::recovery::own_heads;
use crate::train::joins_in;

/// The joining on the base branch that landed `issue` — the newest merge
/// commit there a PR joined a train with that closes it — and when it was
/// made. A later commit that only says `Closes #n` is not one.
fn landing_of(repo: &Repo, base_tip: &str, issue: u64) -> Result<Option<(Joined, u64)>> {
    let grep = format!("--grep=^Closes #{issue}$");
    let joins = joins_in(repo, &["-E", &grep, base_tip])?;
    let Some(join) = joins.into_iter().rfind(|join| join.issue == issue) else {
        return Ok(None);
    };
    let at = repo.git(&["log", "-1", "--format=%ct", &join.commit])?.parse()?;
    Ok(Some((join, at)))
}

/// The landing that put the still-open issue of `claim` on the base branch,
/// when there is one and it is this claim's (`cut_landing`).
pub fn cut_landing_of(
    repo: &Repo,
    base_tip: &str,
    claim: &Claim,
    open: &[PullRequest],
) -> Result<Option<Joined>> {
    let Some((join, at)) = landing_of(repo, base_tip, claim.number)? else {
        return Ok(None);
    };
    let claimed_at = claim.owner.as_ref().map(|owner| owner.ts);
    let cut = cut_landing(&join, at, claimed_at, closing_pr(open, claim.number));
    Ok(cut.then_some(join))
}

/// Fast-forwards the base branch's LOCAL branch, in the checkout worktrees are
/// cut from — the one a human reads — to the remote's tip, so what the fleet
/// landed is on this machine without anyone pulling. Only ever a fast-forward:
/// a branch that diverged, changes in the way, no such local branch — each
/// leaves everything as it was.
fn follow_base(fleet: &Fleet) -> Result<()> {
    let main = Repo { dir: fleet.repo.main_checkout()? };
    let base = &fleet.config.base_branch;
    let local = format!("refs/heads/{base}");
    let before = main.try_git(&["rev-parse", "-q", "--verify", &local])?;
    let Some(tip) = main.fetched(&local)?.filter(|_| before.ok) else {
        return Ok(());
    };
    let here = main.try_git(&["symbolic-ref", "-q", "--short", "HEAD"])?.stdout;
    let moved = match follow(&before.stdout, &tip, here == *base) {
        Follow::UpToDate => true,
        Follow::Merge => main.try_git(&["merge", "--ff-only", "--quiet", &tip])?.ok,
        Follow::Fetch => {
            let update = format!("{local}:{local}");
            let fetch = ["fetch", "--quiet", "--no-write-fetch-head", "--no-tags", REMOTE, &update];
            main.try_git(&fetch)?.ok
        }
    };
    if moved {
        main.try_git(&["update-ref", &format!("refs/remotes/{REMOTE}/{base}"), &tip])?;
    }
    Ok(())
}

/// Settles my claim on `issue`, whose work is on the base branch: what only
/// the tick can do for it — `land` runs inside the worktree and holds no
/// instance id. The claim is deleted last, so a settling that failed on the
/// way is the next tick's to finish.
pub fn settle(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<()> {
    let closed = fleet.gh.issue(issue)?.is_closed();
    let base_tip = fleet.base_tip()?;
    for step in settle_steps(closed) {
        match step {
            SettleStep::CloseIssue => {
                let open = fleet.gh.open_prs()?;
                let claim = fleet.scan.claim_of(issue);
                let landed = match claim {
                    Some(claim) => cut_landing_of(&fleet.repo, &base_tip, claim, &open)?,
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
                let landing = landing_of(&fleet.repo, &base_tip, issue)?;
                if let (Some(pr), Some((join, _))) = (closing_pr(&open, issue), landing)
                    && pr.number == join.pr
                {
                    let comment = landed_comment(&join.commit, &fleet.config.base_branch);
                    fleet.gh.close_pr(pr.number, &comment)?;
                }
            }
            // Only what is on the base branch, and only as it was read: a
            // branch its worker pushed to since holds work nothing landed.
            SettleStep::DropBranches => {
                let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
                for branch in own_heads(fleet, issue, worktree.as_ref())? {
                    let head = format!("refs/heads/{branch}");
                    let Some(tip) = fleet.repo.fetched(&head)? else { continue };
                    if fleet.repo.try_git(&["merge-base", "--is-ancestor", &tip, &base_tip])?.ok {
                        let lease = format!("--force-with-lease={head}:{tip}");
                        fleet.repo.git(&[
                            "push",
                            "--quiet",
                            &lease,
                            REMOTE,
                            &format!(":{head}"),
                        ])?;
                    }
                }
            }
            SettleStep::RemoveWorktree => {
                if let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)? {
                    worktree.remove()?;
                }
            }
            // Soft: a checkout that cannot follow never fails the release it rides on.
            SettleStep::FollowBase => {
                let _ = follow_base(fleet);
            }
            SettleStep::Board => {
                // The base branch itself says whether the work landed.
                let landed = landing_of(&fleet.repo, &base_tip, issue)?;
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
    Ok(())
}

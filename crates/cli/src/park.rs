//! A `blocked` verdict: reading where the issues it names stand, and parking
//! the claim that waits on them.

use afkconvoy_core::blocked::{
    Blocked, BlockerRead, Named, Parkable, Standing, WalkError, blocked_route, standing, walk,
    walk_from,
};
use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::stopped::Workless;
use afkconvoy_core::transition::{self, ParkStep, Released};
use afkconvoy_core::verdict::{Phase, latest_verdict};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::board::last_word;
use crate::claims::{Me, delete};
use crate::fleet::Fleet;
use crate::judge::{mine, nothing_held, worktree_and_work};
use crate::orca::Worktree;

/// Where each issue `named` by the `blocked` verdict on `issue` stands, in
/// the order named: the blocker itself, the claims, the open PRs, and — behind
/// every blocker still open — the chain of open blockers, for the cycle
/// check. The tick and `park` both read through here, so the transition parks
/// exactly what the tick saw as parkable.
pub fn standings(fleet: &Fleet, issue: u64, named: &[u64]) -> Result<Vec<(u64, Standing)>> {
    if named.is_empty() {
        return Ok(vec![]);
    }
    let prs = fleet.gh.open_prs()?;
    let mut blockers = vec![];
    for &number in named {
        // One that cannot be read is never taken for closed.
        let read = fleet.gh.issue(number).ok().map(|read| BlockerRead {
            closed: read.is_closed(),
            state_reason: read.state_reason,
            labels: read.labels,
            pull_request: read.pull_request,
        });
        blockers.push(Named {
            number,
            read,
            claimed: fleet.scan.claim_of(number).is_some(),
            open_pr: closing_pr(&prs, number).is_some(),
        });
    }
    let edges = walk(issue, &walk_from(&blockers), |blocker| fleet.gh.open_blockers(blocker))
        .map_err(|stopped| match stopped {
            WalkError::Read(error) => error,
            WalkError::TooLong => {
                anyhow!(
                    "issue #{issue}: the dependency chain behind its blockers is too long to read"
                )
            }
        })?;
    let standings = blockers
        .iter()
        .map(|blocker| (blocker.number, standing(issue, blocker, &fleet.config, &edges)));
    Ok(standings.collect())
}

/// Parks `claim`: a native blocked-by edge to every blocker in `parkable`
/// GitHub does not record yet → release → remove a worktree with no work.
pub fn park_claim(
    fleet: &mut Fleet,
    claim: &Claim,
    parkable: &Parkable,
    worktree: Option<&Worktree>,
    workless: Option<&Workless>,
) -> Result<Released> {
    let issue = claim.number;
    let recorded = fleet.gh.blocked_by(issue)?;
    transition::park(issue, parkable, workless, |step| {
        match step {
            ParkStep::Edge { blocker } => {
                if !recorded.iter().any(|edge| edge.number == blocker) {
                    fleet.gh.add_blocker(issue, fleet.gh.issue(blocker)?.id)?;
                }
            }
            ParkStep::Board => last_word(fleet, claim, Stage::Parked, None, &parkable.blockers())?,
            ParkStep::Release => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            ParkStep::RemoveWorktree => {
                if let Some(worktree) = worktree {
                    worktree.remove()?;
                }
            }
        }
        Ok(())
    })
}

/// `afkconvoy park`: leave one of my claims waiting on the issues its worker
/// declared it blocked by. The verdict and the standings are read again here,
/// and a claim that is not parkable now is refused untouched.
pub fn park(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let comments = fleet.gh.comments(issue)?;
    let verdict = latest_verdict(comments.iter().map(|c| c.body.as_str()));
    let refused = |why: &str| anyhow!("issue #{issue} is not parkable: {why}; nothing was changed");
    let Some(verdict) = verdict.filter(|verdict| verdict.phase == Some(Phase::Blocked)) else {
        return Err(refused("its worker's latest verdict is not `blocked`"));
    };
    let parkable = match blocked_route(&standings(fleet, issue, &verdict.blocked_by)?) {
        Blocked::Park(parkable) => parkable,
        Blocked::Redispatch => {
            return Err(refused("every blocker it named is closed — dispatch it again instead"));
        }
        Blocked::Escalate(reason) => {
            bail!("{}", refused(&format!("{reason} — escalate it instead")))
        }
    };
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    park_claim(fleet, &claim, &parkable, worktree.as_ref(), workless.as_ref())?;
    Ok(json!({"issue": issue, "action": "parked", "blocked_by": parkable.blockers(),
        "released": true, "worktree": if workless.is_some() { "removed" } else { "kept" }}))
}

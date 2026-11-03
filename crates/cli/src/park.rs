//! A `blocked` verdict: reading where the issues it names stand, and parking
//! the claim that waits on them.

use afkconvoy_core::blocked::{
    Blocked, BlockedOn, BlockerRead, CutWait, Named, Parkable, Standing, WalkError, blocked_route,
    standing, walk, walk_from,
};
use afkconvoy_core::claims::Claim;
use afkconvoy_core::cut::WAITING_LABEL;
use afkconvoy_core::left::Left;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::transition::{self, ParkStep};
use afkconvoy_core::verdict::{Phase, latest_verdict};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::board::parked;
use crate::claims::{Me, delete};
use crate::fleet::Fleet;
use crate::judge::{leaving, mine, nothing_held, with_left, worktree_of};
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
    let flows = crate::team::flow_labels(&fleet.gh, &fleet.config);
    let standings = blockers
        .iter()
        .map(|blocker| (blocker.number, standing(issue, blocker, &fleet.config, &flows, &edges)));
    Ok(standings.collect())
}

/// What the `blocked` verdict on `issue`, waiting on `named`, comes to: from
/// where each issue it names stands (`standings`), and — where it names the
/// next cut — whether one will come (`cut::wait`).
pub fn routed(fleet: &Fleet, issue: u64, named: &BlockedOn) -> Result<Blocked> {
    let cut = match named.cut {
        true => crate::cut::wait(fleet)?,
        false => CutWait::No,
    };
    Ok(blocked_route(&standings(fleet, issue, &named.issues)?, &cut))
}

/// The `blocked` verdict a successful continuation will answer, read before
/// starting its successor. Keeping its comment, rather than reading again
/// after startup, never answers something the new worker just declared.
pub(crate) fn cleared_verdict(fleet: &Fleet, issue: u64) -> Result<Option<String>> {
    let comments = fleet.gh.comments(issue)?;
    let verdict = latest_verdict(comments.iter().map(|comment| comment.body.as_str()));
    let Some(verdict) = verdict.filter(|verdict| verdict.phase == Some(Phase::Blocked)) else {
        return Ok(None);
    };
    match routed(fleet, issue, &verdict.blocked_by)? {
        Blocked::Redispatch => Ok(comments.get(verdict.comment).map(|comment| comment.url.clone())),
        _ => Ok(None),
    }
}

/// Parks `claim`: the label that waits on the next cut where `parkable` does,
/// and a native blocked-by edge to every blocker in it GitHub does not record yet → release → remove the worktree, `worktree`
/// here, when it holds nothing the remote does not, or close the terminal of
/// its worker in one kept for what only it holds, which the board names →
/// what became of it.
pub fn park_claim(
    fleet: &mut Fleet,
    claim: &Claim,
    parkable: &Parkable,
    worktree: Option<&Worktree>,
) -> Result<Option<Left>> {
    let issue = claim.number;
    let left =
        worktree.map(|worktree| leaving(fleet, issue, worktree)).transpose()?.map(|l| l.left);
    let recorded = fleet.gh.blocked_by(issue)?;
    let spare = left.as_ref().and_then(Left::spare);
    let kept = left.as_ref().and_then(Left::kept).copied();
    transition::park(issue, parkable, spare, |step| {
        match step {
            ParkStep::Edge { blocker } => {
                if !recorded.iter().any(|edge| edge.number == blocker) {
                    fleet.gh.add_blocker(issue, fleet.gh.issue(blocker)?.id)?;
                }
            }
            ParkStep::Cut => {
                let labels = fleet.gh.issue(issue)?.labels;
                if !labels.iter().any(|label| label == WAITING_LABEL) {
                    fleet.gh.add_label(issue, WAITING_LABEL)?;
                }
            }
            ParkStep::Board => parked(fleet, claim, parkable, kept)?,
            ParkStep::Release => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            ParkStep::RemoveWorktree => {
                if let Some(worktree) = worktree {
                    worktree.remove()?;
                }
            }
            ParkStep::CloseTerminal => {
                if let Some(worktree) = worktree {
                    worktree.close_terminals()?;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(left)
}

/// `afkconvoy park`: leave one of my claims waiting on the issues its worker
/// declared it blocked by. The verdict and the standings are read again here,
/// and a claim that is not parkable now is refused untouched.
pub fn park(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let comments = fleet.gh.comments(issue)?;
    let refused = |why: &str| anyhow!("issue #{issue} is not parkable: {why}; nothing was changed");
    let not_blocked = || refused("its worker's latest verdict is not `blocked`");
    // Repairs in a bound repo run outside its workflows, as they do in a tick.
    let labels = match &fleet.config.bound {
        Some(_) => Some(fleet.gh.issue(issue)?.labels),
        None => None,
    };
    let named =
        if let Some(labels) = labels.filter(|labels| !afkconvoy_core::plan::is_repair(labels)) {
            use afkconvoy_core::stage_record::{Advance, advance, history, stage_records};
            let team = crate::team::load(crate::team::From::Config, Some(&fleet.repo))?.team;
            let workflow =
                afkconvoy_core::role::workflow_of(&team, &labels).map_err(|why| refused(&why))?;
            let records = stage_records(comments.iter().map(|c| c.body.as_str()))
                .into_iter()
                .map(|(_, record)| record)
                .collect();
            let history = history(workflow, records);
            match advance(&history, &claim.id) {
                Advance::Blocked(named) => named,
                _ => return Err(not_blocked()),
            }
        } else {
            latest_verdict(comments.iter().map(|c| c.body.as_str()))
                .filter(|verdict| verdict.phase == Some(Phase::Blocked))
                .ok_or_else(not_blocked)?
                .blocked_by
        };
    let parkable = match routed(fleet, issue, &named)? {
        Blocked::Park(parkable) => parkable,
        Blocked::Redispatch => {
            return Err(refused("every blocker it named is closed — dispatch it again instead"));
        }
        Blocked::Escalate(reason) => {
            bail!("{}", refused(&format!("{reason} — escalate it instead")))
        }
    };
    let worktree = worktree_of(fleet, issue)?;
    let left = park_claim(fleet, &claim, &parkable, worktree.as_ref())?;
    let mut answer = json!({"issue": issue, "action": "parked", "blocked_by": parkable.blockers(),
        "released": true});
    if parkable.cut() {
        answer["cut"] = json!(true);
    }
    Ok(with_left(answer, left.as_ref()))
}

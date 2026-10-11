//! Starting a worker on an issue.

use afkconvoy_core::prompt::{WorkerFields, worker_prompt};
use afkconvoy_core::recovery::{Recovery, Tier, superseded};
use afkconvoy_core::worker::{
    DispatchStep, branch_comment, dispatch_steps, recorded_branches, worktree_name,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::{Me, Take, take};
use crate::fleet::Fleet;
use crate::gh::IssueRead;
use crate::git::REMOTE;
use crate::orca::{Source, Worktree};
use crate::recovery::{Survey, own_heads, survey};

/// How a worker is started on this machine.
pub struct Agent {
    /// The worker launch command: opaque, handed to orca as it was given.
    pub command: String,
    /// How long an agent is given to be ready for its prompt, in seconds.
    pub ready_timeout: u64,
}

/// Where a dispatch starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StartFrom {
    /// Whatever progress survives: the worktree still here, else the pushed
    /// branch, else the base branch.
    Auto,
    /// The base branch, with the previous attempt discarded first.
    Fresh,
}

/// `afkconvoy dispatch`: start a worker on one issue, start to finish — on a
/// frontier issue, or continuing a claim already held from what survived.
///
///   {"started": true, "claim": "won"|"held", tier, action, prompt, reason,
///    worktree, branch, terminal}
///   {"started": false, "claim": "lost", "owner": {…}}   a peer holds the issue
///
/// A step that fails is an error with the claim still held: the next tick
/// finds a claim with no worker and continues it. Nothing is torn down unless
/// `from` is `Fresh`.
pub fn dispatch(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    number: u64,
    from: StartFrom,
) -> Result<Value> {
    let issue = fleet.gh.issue(number)?;
    if issue.is_closed() {
        // before the claim: never lock a closed issue
        bail!("issue #{number} is closed, not open — nothing to dispatch");
    }
    let held = fleet.scan.claim_of(number).is_some_and(|claim| claim.is_held_by(&me.instance));
    let by_hand = from == StartFrom::Fresh;
    let mut found: Option<Survey> = None;
    let mut plan = Recovery::discarded();
    let mut worktree: Option<Worktree> = None;
    let mut brief = None;
    let mut terminal: Option<String> = None;
    let mut discarded = None;
    let undecided = || anyhow!("a dispatch step ran before the one that feeds it");
    for step in dispatch_steps(held, by_hand) {
        match step {
            DispatchStep::Claim => {
                let claim = take(&fleet.repo, &mut fleet.scan, me, number, Take::Fresh)?;
                if claim["won"] != true {
                    return Ok(json!({"issue": number, "started": false, "claim": "lost",
                        "owner": claim["owner"]}));
                }
            }
            DispatchStep::Survey => {
                let surveyed = survey(fleet, number)?;
                plan = surveyed.plan.clone();
                found = Some(surveyed);
            }
            DispatchStep::Discard => discarded = Some(discard_attempt(fleet, number)?),
            DispatchStep::Place => {
                let survived = found.as_ref();
                worktree = Some(match plan.tier {
                    Tier::Reuse => {
                        survived.and_then(|found| found.worktree.clone()).ok_or_else(undecided)?
                    }
                    // What the REMOTE has now, never a local branch of that name.
                    Tier::Recreate => {
                        let pushed = survived.and_then(|found| found.pushed.as_ref());
                        cut_at(fleet, &issue, number, &pushed.ok_or_else(undecided)?.name)?
                    }
                    Tier::Fresh => cut_at(fleet, &issue, number, &fleet.config.base_branch)?,
                });
            }
            DispatchStep::RecordBranch => {
                let cut = worktree.as_ref().ok_or_else(undecided)?;
                let comments = fleet.gh.comments(number)?;
                let recorded = recorded_branches(comments.iter().map(|c| c.body.as_str()));
                if !recorded.contains(&cut.branch) {
                    fleet.gh.comment(number, &branch_comment(&cut.branch))?;
                }
            }
            DispatchStep::Brief => {
                let cut = worktree.as_ref().ok_or_else(undecided)?;
                let path = cut.path.display().to_string();
                let fields = WorkerFields {
                    number,
                    title: &issue.title,
                    branch: &cut.branch,
                    worktree: &path,
                    base_branch: &fleet.config.base_branch,
                    gate_command: &crate::own_command("gate --train"),
                    land_command: &crate::own_command(&format!("land --issue {number}")),
                };
                let prompt = worker_prompt(&fields, plan.variant).map_err(|e| anyhow!(e))?;
                brief = Some(cut.write_brief(&prompt)?);
            }
            DispatchStep::Start => {
                let cut = worktree.as_ref().ok_or_else(undecided)?;
                terminal = Some(cut.start(&agent.command)?);
            }
            DispatchStep::AwaitReady => {
                let (cut, handle) =
                    worktree.as_ref().zip(terminal.as_ref()).ok_or_else(undecided)?;
                cut.await_ready(handle, agent.ready_timeout)?;
            }
            DispatchStep::Deliver => {
                let (cut, handle) =
                    worktree.as_ref().zip(terminal.as_ref()).ok_or_else(undecided)?;
                cut.deliver(handle, brief.as_ref().ok_or_else(undecided)?)?;
            }
        }
    }
    let cut = worktree.ok_or_else(undecided)?;
    let mut started = json!({"issue": number, "started": true,
        "claim": if held { "held" } else { "won" },
        "tier": plan.tier.number(), "action": plan.tier.word(),
        "prompt": plan.variant.word(), "reason": plan.reason,
        "worktree": cut.path, "branch": cut.branch, "terminal": terminal});
    if let Some(discarded) = discarded {
        started["discarded"] = discarded;
    }
    Ok(started)
}

/// Has orca cut a worktree for issue `number` at the tip the remote has for
/// `branch`.
fn cut_at(fleet: &Fleet, issue: &IssueRead, number: u64, branch: &str) -> Result<Worktree> {
    let sha = fleet
        .repo
        .fetched(&format!("refs/heads/{branch}"))?
        .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
    let source = Source::of(&fleet.repo)?;
    Worktree::cut(&source, &worktree_name(number, &issue.title), &sha, Some(number))
}

/// Throws the previous attempt at issue `number` away, for a fresh start by
/// hand: close the open PRs on its own branches, delete those branches on the
/// remote — never one that only has the name of one — and remove its worktree.
fn discard_attempt(fleet: &Fleet, number: u64) -> Result<Value> {
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, number)?;
    let own = own_heads(fleet, number, worktree.as_ref())?;
    let closed = superseded(&fleet.gh.open_prs()?, number, &own);
    for pr in &closed {
        let why = "afkconvoy: superseded — this attempt was discarded and the issue is being \
                   started again from a clean base.";
        fleet.gh.close_pr(*pr, why)?;
    }
    for branch in &own {
        fleet.repo.git(&["push", "--quiet", REMOTE, &format!(":refs/heads/{branch}")])?;
    }
    if let Some(worktree) = &worktree {
        worktree.remove()?;
    }
    Ok(json!({"closed_prs": closed, "deleted_branches": own,
        "removed_worktree": worktree.map(|worktree| worktree.path)}))
}

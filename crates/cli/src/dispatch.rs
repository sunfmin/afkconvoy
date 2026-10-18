//! Starting a worker on an issue.

use std::path::PathBuf;

use afkconvoy_core::harness::Harness;
use afkconvoy_core::launch::LaunchCommand;
use afkconvoy_core::prompt::{WorkerFields, worker_prompt};
use afkconvoy_core::recovery::{Recovery, Tier, Variant, superseded};
use afkconvoy_core::worker::{
    DispatchStep, branch_comment, dispatch_steps, recorded_branches, worktree_name,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::{Me, Take, take};
use crate::fleet::Fleet;
use crate::gh::IssueRead;
use crate::git::REMOTE;
use crate::launch::Launches;
use crate::orca::{Source, Worktree};
use crate::recovery::{Survey, own_heads, survey};
use crate::role::Staged;

/// How a worker is started on this machine.
pub struct Agent {
    /// The worker launch command of each harness. In an unbound repo the
    /// default's is the one opaque command, handed to orca as it was given;
    /// in a bound repo what starts the worker in its role is added to its
    /// harness's (`role::Staged::command`).
    pub launches: Launches,
    /// How long an agent is given to be ready for its prompt, in seconds.
    pub ready_timeout: u64,
}

impl Agent {
    /// The worker launch command a worker of `harness` is started with. An
    /// error is a harness that cannot be started on this host: no other is
    /// ever started in its place.
    pub fn command(&self, harness: Harness) -> Result<LaunchCommand> {
        self.launches.command(harness)
    }
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
///    worktree, branch, terminal}   and, in a bound repo, the `stage` it performs
///                                  — `land` for the join that follows the last
///   {"started": false, "claim": "lost", "owner": {…}}   a peer holds the issue
///   {"started": false, "skipped": "blocked", "blocked_by": [n…]}
///                                  an open blocker, read just before the claim
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
    // In a bound repo the worker performs a stage, or the join that follows
    // the last, in its role. Read before the claim: an issue no worker can
    // be started on is never locked.
    let staged = match &fleet.config.bound {
        Some(bound) => Some(Staged::enter(fleet, bound, number, &issue.labels)?),
        None => None,
    };
    // Before the claim too: an issue whose harness cannot be started here is
    // never locked.
    let launch = agent.command(match &staged {
        Some(staged) => staged.harness(),
        None => fleet.config.harness(),
    })?;
    let held = fleet.scan.claim_of(number).is_some_and(|claim| claim.is_held_by(&me.instance));
    if let Some(staged) = staged.as_ref().filter(|_| held) {
        staged.refuse_spent(fleet.scan.claim_of(number))?;
    }
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
                // A tick's rebuild can be minutes older than this: a blocker
                // added since holds the issue back all the same.
                let mut blockers = fleet.gh.open_blockers(number)?;
                if !blockers.is_empty() {
                    blockers.sort_unstable();
                    return Ok(json!({"issue": number, "started": false, "skipped": "blocked",
                        "blocked_by": blockers}));
                }
                let claim = take(&fleet.repo, &mut fleet.scan, me, number, Take::Fresh)?;
                if claim["won"] != true {
                    return Ok(json!({"issue": number, "started": false, "claim": "lost",
                        "owner": claim["owner"]}));
                }
                if let Some(staged) = &staged {
                    staged.refuse_spent(fleet.scan.claim_of(number))?;
                }
            }
            DispatchStep::Survey => {
                let surveyed = survey(fleet, number)?;
                // An open PR on an issue no claim held is one a released claim kept.
                plan = if held { surveyed.plan.clone() } else { surveyed.plan.clone().resumed() };
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
                brief = Some(match &staged {
                    Some(staged) => staged.write_brief(fleet, cut, &issue.title)?,
                    None => write_brief(fleet, cut, number, &issue.title, plan.variant)?,
                });
            }
            DispatchStep::Start => {
                let cut = worktree.as_ref().ok_or_else(undecided)?;
                let launch = launch.as_str();
                if let Some((staged, claim)) = staged.as_ref().zip(fleet.scan.claim_of(number)) {
                    staged.keep_start(cut, claim)?;
                }
                terminal = Some(match &staged {
                    Some(staged) => cut.start(&staged.command(launch, cut)?, fleet.now)?,
                    None => cut.start(launch, fleet.now)?,
                });
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
    if let Some(staged) = &staged {
        started["prompt"] = json!(staged.prompt());
        started["stage"] = staged.json();
    }
    if let Some(discarded) = discarded {
        started["discarded"] = discarded;
    }
    Ok(started)
}

/// Writes the brief of the worker of issue `number` into its worktree → the
/// file.
pub(crate) fn write_brief(
    fleet: &Fleet,
    worktree: &Worktree,
    number: u64,
    title: &str,
    variant: Variant,
) -> Result<PathBuf> {
    let path = worktree.path.display().to_string();
    let fields = WorkerFields {
        number,
        title,
        branch: &worktree.branch,
        worktree: &path,
        base_branch: &fleet.config.base_branch,
        gate_command: &crate::own_command().fixed(" gate --train").own(),
        land_command: &crate::own_command().fixed(" land --issue ").number(number).own(),
        wake_command: &crate::own_command().fixed(" wake").own(),
    };
    let prompt = worker_prompt(&fields, variant).map_err(|e| anyhow!(e))?;
    worktree.write_brief(&prompt)
}

/// Writes into `worktree` the join-only brief of the worker that joins `pr`,
/// which closes issue `number`, read as `issue` → the file. In a bound repo
/// that worker is one in the landing role of the issue's workflow.
pub(crate) fn write_join_brief(
    fleet: &Fleet,
    worktree: &Worktree,
    (number, issue): (u64, &IssueRead),
    pr: u64,
) -> Result<PathBuf> {
    match &fleet.config.bound {
        Some(bound) => Staged::enter(fleet, bound, number, &issue.labels)?.write_brief(
            fleet,
            worktree,
            &issue.title,
        ),
        None => write_brief(fleet, worktree, number, &issue.title, Variant::Join { pr }),
    }
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

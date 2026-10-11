//! Starting a worker on an issue.

use afkconvoy_core::prompt::{WorkerFields, worker_prompt};
use afkconvoy_core::worker::{
    DispatchStep, branch_comment, dispatch_steps, recorded_branches, worktree_name,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::{Me, Take, take};
use crate::fleet::Fleet;
use crate::git::REMOTE;
use crate::orca::{Source, Worktree};

/// How a worker is started on this machine.
pub struct Agent {
    /// The worker launch command: opaque, handed to orca as it was given.
    pub command: String,
    /// How long an agent is given to be ready for its prompt, in seconds.
    pub ready_timeout: u64,
}

/// `afkconvoy dispatch`: start one fresh worker on one issue, start to finish.
///
///   {"started": true, "claim": "won"|"held", worktree, branch, terminal}
///   {"started": false, "claim": "lost", "owner": {…}}   a peer holds the issue
///
/// A step that fails is an error with the claim still held: the next tick
/// finds a claim with no worker and continues it.
pub fn dispatch(fleet: &mut Fleet, me: &Me, agent: &Agent, number: u64) -> Result<Value> {
    let issue = fleet.gh.issue(number)?;
    if issue.is_closed() {
        // before the claim: never lock a closed issue
        bail!("issue #{number} is closed, not open — nothing to dispatch");
    }
    let held = fleet.scan.claim_of(number).is_some_and(|claim| claim.is_held_by(&me.instance));
    let mut worktree: Option<Worktree> = None;
    let mut brief = None;
    let mut terminal: Option<String> = None;
    let undecided = || anyhow!("a dispatch step ran before the one that feeds it");
    for step in dispatch_steps(held) {
        match step {
            DispatchStep::Claim => {
                let claim = take(&fleet.repo, &mut fleet.scan, me, number, Take::Fresh)?;
                if claim["won"] != true {
                    return Ok(json!({"issue": number, "started": false, "claim": "lost",
                        "owner": claim["owner"]}));
                }
            }
            DispatchStep::Cut => {
                // What the REMOTE has now, never a local branch of that name.
                let base = &fleet.config.base_branch;
                let branch_ref = format!("refs/heads/{base}");
                let sha = fleet
                    .repo
                    .remote_sha(&branch_ref)?
                    .ok_or_else(|| anyhow!("the remote has no branch {base:?}"))?;
                fleet.repo.git(&[
                    "fetch",
                    "--quiet",
                    "--no-write-fetch-head",
                    "--no-tags",
                    REMOTE,
                    &branch_ref,
                ])?;
                let source = Source::of(&fleet.repo)?;
                let name = worktree_name(number, &issue.title);
                worktree = Some(Worktree::cut(&source, &name, &sha, Some(number))?);
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
                let prompt = worker_prompt(&WorkerFields {
                    number,
                    title: &issue.title,
                    branch: &cut.branch,
                    worktree: &path,
                    base_branch: &fleet.config.base_branch,
                    gate_command: &fleet.config.gate_command,
                })
                .map_err(|e| anyhow!(e))?;
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
    Ok(json!({"issue": number, "started": true, "claim": if held { "held" } else { "won" },
        "worktree": cut.path, "branch": cut.branch, "terminal": terminal}))
}

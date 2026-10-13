//! A train worker that stopped with the train not landed: nudging it, and
//! giving the train up when it stays silent.

use afkconvoy_core::abandon::{
    AbandonStep, Rider, TrainRoute, abandon, abandoned_reason, standing, train_route,
};
use afkconvoy_core::claims::Claim;
use afkconvoy_core::joining::JoinRecord;
use afkconvoy_core::prompt::{rejoin_pointer, train_nudge_line};
use afkconvoy_core::stopped::Silent;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::train::{Tend, on_train, tend, train_refs};
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::Me;
use crate::dispatch::write_join_brief;
use crate::fleet::Fleet;
use crate::gh::{Comment, IssueRead};
use crate::git::REMOTE;
use crate::judge::{escalate_claim, nudge_worker, progress};
use crate::orca::{Source, Worktree};
use crate::train::Train;
use crate::watch::{join_record, record_join};

/// The train worker as it stopped on the tip it was told about.
pub struct StoppedOn<'a> {
    pub train: &'a Train,
    pub worktree: &'a Worktree,
    pub state: WorkerState,
    /// When it was told of that tip.
    pub told_at: Option<u64>,
}

/// A PR of mine on the train being abandoned, with what an abandon step needs
/// of it.
struct Riding {
    claim: Claim,
    issue: IssueRead,
    record: JoinRecord,
    comments: Vec<Comment>,
}

/// The tick's look at a train worker that stopped with the train not landed:
/// left while it shows life, nudged once, and silent again the train is
/// abandoned (`abandon::train_route`).
pub fn stopped_train(fleet: &mut Fleet, me: &Me, stopped: &StoppedOn) -> Result<Outcome> {
    match route(fleet, stopped)? {
        TrainRoute::Leave => Ok(Outcome::Nothing),
        TrainRoute::Nudge(unnudged) => {
            nudge_worker(stopped.worktree, unnudged, train_nudge_line, fleet.now)?;
            Ok(Outcome::Nudged)
        }
        TrainRoute::Abandon(silent) => {
            abandon_train(fleet, me, stopped, silent)?;
            Ok(Outcome::Abandoned)
        }
    }
}

fn route(fleet: &Fleet, stopped: &StoppedOn) -> Result<TrainRoute> {
    let progress = progress(stopped.worktree, &stopped.train.base)?;
    let nudged = stopped.worktree.nudged()?;
    let handed = stopped.worktree.handed(stopped.told_at)?;
    Ok(train_route(stopped.state, &progress, nudged, handed, fleet.now))
}

/// Gives the train up with nothing landed: its refs deleted — only while the
/// line is still at the tip its worker went silent on — its worktree cleared
/// and kept, and each PR of mine that was on it recorded and its worker told
/// to join the next train; one taken off an abandoned train before is
/// escalated instead. What the train already put on the base branch is not
/// touched: it has landed, and is settled from there.
fn abandon_train(fleet: &mut Fleet, me: &Me, stopped: &StoppedOn, silent: Silent) -> Result<Value> {
    let train = stopped.train;
    let prs = fleet.gh.open_prs()?;
    let joins = train.joins(&fleet.repo)?;
    let mut riders = vec![];
    let mut riding = vec![];
    for join in on_train(&joins, &prs) {
        // A peer's PR on the train is its own fleet's: joining again there, from its next tick.
        let Some(claim) = fleet.scan.claim_of(join.issue).filter(|c| c.is_held_by(&me.instance))
        else {
            continue;
        };
        let claim = claim.clone();
        let (record, comments) = join_record(fleet, join.pr)?;
        let issue = fleet.gh.issue(join.issue)?;
        riders.push(Rider { issue: join.issue, pr: join.pr, standing: standing(&record) });
        riding.push((join.issue, Riding { claim, issue, record, comments }));
    }
    let of = |issue: u64| -> Result<&Riding> {
        let found = riding.iter().find(|(number, _)| *number == issue);
        Ok(&found.ok_or_else(|| anyhow!("issue #{issue} was not on the train"))?.1)
    };
    let source = Source::of(&fleet.repo)?;
    let done = abandon(silent, &riders, |step| {
        match step {
            AbandonStep::Drop => drop_train(fleet, train)?,
            AbandonStep::Clear => stopped.worktree.clear(&train.base)?,
            AbandonStep::Record { issue, pr } => {
                let rider = of(issue)?;
                let record = JoinRecord { abandoned: Some(fleet.now), ..rider.record.clone() };
                record_join(fleet, pr, &record, &rider.comments)?;
            }
            AbandonStep::Tell { issue, pr } => {
                // With no worker there to tell, the next tick continues the claim onto the join.
                let Some(worktree) = Worktree::of_issue(&source, issue)? else { return Ok(()) };
                let Some(terminal) = worktree.terminal()? else { return Ok(()) };
                let brief = write_join_brief(fleet, &worktree, (issue, &of(issue)?.issue), pr)?;
                worktree.tell(&terminal, &rejoin_pointer(&brief.display().to_string()))?;
            }
            AbandonStep::Escalate { issue, pr } => {
                let worktree = Worktree::of_issue(&source, issue)?;
                let claim = of(issue)?.claim.clone();
                // No proof of a workless branch is offered: the worktree is kept.
                let reason = abandoned_reason(pr);
                escalate_claim(fleet, &claim, &reason, Some(pr), worktree.as_ref(), None)?;
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(json!({"action": "abandoned", "train": train.tip, "issues": done.again,
        "escalated": done.escalated, "worktree": stopped.worktree.path}))
}

/// Deletes the train's refs: the line only while it is still at the tip that
/// was read — a PR that joined meanwhile is a train its worker is told about,
/// not one to give up — then the record of its last red run.
fn drop_train(fleet: &Fleet, train: &Train) -> Result<()> {
    let refs = train_refs(fleet.repo.layout(), &fleet.config.base_branch);
    let lease = format!("--force-with-lease={}:{}", refs.line, train.tip);
    let dropped =
        fleet.repo.try_git(&["push", "--quiet", REMOTE, &lease, &format!(":{}", refs.line)])?;
    if !dropped.ok {
        match fleet.repo.remote_sha(&refs.line)? {
            None => {}
            Some(tip) if tip == train.tip => {
                bail!("the landing train's refs could not be deleted: {}", dropped.stderr.trim())
            }
            Some(_) => bail!("the landing train moved while it was being abandoned; it was kept"),
        }
    }
    if fleet.repo.remote_sha(&refs.red)?.is_some() {
        fleet.repo.git(&["push", "--quiet", REMOTE, &format!(":{}", refs.red)])?;
    }
    Ok(())
}

/// `afkconvoy abandon`: give the landing train up by hand. Refused unless the
/// tick would — its worker stopped on the tip it was told about, was nudged,
/// and stayed silent a grace period more.
pub fn abandon_by_hand(fleet: &mut Fleet, me: &Me) -> Result<Value> {
    let train = Train::read(&fleet.repo, &fleet.config)?;
    if train.tip == train.base {
        bail!("nothing is on the landing train; nothing was abandoned");
    }
    let Some(worktree) = Worktree::of_train(&Source::of(&fleet.repo)?)? else {
        bail!("the landing train has no worktree on this host; nothing was abandoned");
    };
    let state = worktree.state(fleet.now)?;
    let told = worktree.told()?;
    let tip = told.as_ref().map(|told| told.tip.as_str());
    if tend(state, tip, &train.tip) != Tend::Stopped {
        bail!(
            "the landing train is not abandoned: its worker is at work, gone, or was not told \
             of what the train holds now — the tick tends it; nothing was changed"
        );
    }
    let told_at = told.and_then(|told| told.at);
    let stopped = StoppedOn { train: &train, worktree: &worktree, state, told_at };
    let TrainRoute::Abandon(silent) = route(fleet, &stopped)? else {
        bail!(
            "the landing train is not abandoned: its worker showed a sign of life within the \
             grace period, or was not nudged yet; nothing was changed"
        );
    };
    abandon_train(fleet, me, &stopped, silent)
}

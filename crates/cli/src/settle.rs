//! Settling a claim whose work has landed, and finding the landings that were
//! cut short of closing their issue.

use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::landing::{LandedPr, closes_grep};
use afkconvoy_core::left::{Kept, KeptBecause};
use afkconvoy_core::plan::{PullRequest, closing_pr};
use afkconvoy_core::settle::{
    Follow, SettleStep, carried_steps, cut_landing, follow, landed_comment, settle_steps,
};
use afkconvoy_core::worker::{WorkerState, carried_by};
use anyhow::{Result, bail};

use crate::board::last_word;
use crate::claims::{Me, release};
use crate::fleet::Fleet;
use crate::found::Found;
use crate::gh::Tracker;
use crate::git::{REMOTE, Repo};
use crate::landing::joins_in;
use crate::orca::{Ended, Source, Unremoved, Worktree};
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

/// Whether the work of `issue` is on the base branch: a closing its
/// landing made is no closing under its worker.
pub(crate) fn landed(fleet: &Fleet, issue: u64) -> Result<bool> {
    let base_tip = fleet.task_tip(issue)?;
    Ok(landing_of(&fleet.repo, &fleet.gh.tracker, &base_tip, issue)?.is_some())
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
/// settle goes on: the claim is released, and the worktree is noted kept for
/// a later tick to look at again (`remove_kept`), and why is in `found`.
/// `carried_by` is the PR, on the branch `carrier`, that carries this
/// issue's.
fn remove_settled(
    fleet: &Fleet,
    found: &mut Found,
    issue: u64,
    worktree: &Worktree,
    carried_by: Option<(u64, &str)>,
) -> Result<Option<u64>> {
    let wait = || -> Result<Option<u64>> {
        let file = worktree.note("convoy-settle-wait")?;
        let since =
            std::fs::read_to_string(&file).ok().and_then(|s| s.parse().ok()).unwrap_or(fleet.now);
        if !file.exists() {
            std::fs::write(file, since.to_string())?;
        }
        Ok(Some(since))
    };
    let mut listed = None;
    if worktree.holds_a_worker() {
        match worktree.state(fleet.now)? {
            WorkerState::Busy => return wait(),
            // One still ending is waited for like one still working: the
            // grace, not this tick, says when that is a finding.
            WorkerState::Idle { .. } => match worktree.end_settled()? {
                (Ended::Unconfirmed(_), _) => return wait(),
                (Ended::Confirmed, confirmed) => listed = confirmed,
            },
            WorkerState::Gone => {}
        }
    }
    // A stopped orphan can never leave on its own: it is ended, and only a
    // process that outlives that keeps the worktree.
    let busy = match worktree.end_stopped_orphans(listed)? {
        Some(listed) => crate::hot::working_directories_of(&listed),
        None => crate::hot::working_directories()?,
    };
    if crate::hot::occupies(&busy, &worktree.path) {
        return wait();
    }
    if let Some(kept) = settled_keeps(fleet, issue, worktree, carried_by.map(|(_, b)| b))? {
        // The step says why before the note is written: a note that cannot
        // be still leaves the reason in the tick log.
        let said = kept.unwrap_or(Kept { because: KeptBecause::Unread, unpushed: None });
        found.kept.push((issue, worktree.path.clone(), said));
        if let Some(kept) = kept {
            let by = carried_by.map(|(by, _)| by);
            KeptAtSettle::of(issue, &kept, by, fleet.now).write(worktree)?;
        }
        return Ok(None);
    }
    crate::account::refresh_stopped(&fleet.repo, &fleet.gh, issue)?;
    let gone = !worktree.path.exists();
    worktree.remove()?;
    // Orca removes a worktree whose directory is already gone from its list
    // alone: git's registration of it is removed here (sunfmin/control#982).
    if gone {
        fleet.repo.remove_worktree(&worktree.path)?;
    }
    Ok(None)
}

/// Whether settling keeps `worktree`, and why where git read it. A worktree
/// git can still read is kept by `settle_keeps`, what it holds read by
/// `leaving_settled`, the branch `carrier` known. One git can no longer read
/// — its registration already gone, or its directory (sunfmin/control#982),
/// where git cannot even be started — is kept unless the head orca lists is
/// on the remote.
fn settled_keeps(
    fleet: &Fleet,
    issue: u64,
    worktree: &Worktree,
    carrier: Option<&str>,
) -> Result<Option<Option<Kept>>> {
    let readable = worktree.path.is_dir()
        && Repo::new(worktree.path.clone()).try_git(&["rev-parse", "HEAD"])?.ok;
    if readable {
        let leaving = crate::judge::leaving_settled(fleet, issue, worktree, carrier)?;
        let keeps = afkconvoy_core::left::settle_keeps(&leaving.left);
        return Ok(keeps.then(|| leaving.left.kept().copied()));
    }
    let listed = crate::orca::listed_head(&worktree.path)?;
    let on_remote = crate::orca::commit_is_on_remote(
        &fleet.repo,
        fleet.config.lands_on(),
        &worktree.branch,
        &listed,
    );
    Ok((!on_remote).then_some(None))
}

/// The note a settle leaves in the worktree it kept (sunfmin/control#1161):
/// the issue, why it was kept, and the PR that carries the issue's where one
/// does. Each later tick looks at that worktree again (`remove_kept`), and
/// `doctor` names it (`kept_at_settle`). It goes with the worktree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct KeptAtSettle {
    pub issue: u64,
    pub kept_because: String,
    pub unpushed: Option<u64>,
    pub carried_by: Option<u64>,
    /// When a settle first kept it.
    pub since: u64,
}

const KEPT_AT_SETTLE: &str = "convoy-settle-kept";

impl KeptAtSettle {
    fn of(issue: u64, kept: &Kept, carried_by: Option<u64>, now: u64) -> KeptAtSettle {
        KeptAtSettle {
            issue,
            kept_because: kept.because.key().to_string(),
            unpushed: kept.unpushed,
            carried_by,
            since: now,
        }
    }

    /// The note in `worktree`, where a settle left one.
    pub(crate) fn of_worktree(worktree: &Worktree) -> Option<KeptAtSettle> {
        let file = worktree.note(KEPT_AT_SETTLE).ok()?;
        serde_json::from_slice(&std::fs::read(file).ok()?).ok()
    }

    /// Writes it into `worktree`, keeping when an earlier settle first kept it.
    fn write(mut self, worktree: &Worktree) -> Result<()> {
        if let Some(earlier) = Self::of_worktree(worktree).filter(|e| e.issue == self.issue) {
            self.since = earlier.since;
        }
        std::fs::write(worktree.note(KEPT_AT_SETTLE)?, serde_json::to_vec(&self)?)?;
        Ok(())
    }

    fn forget(worktree: &Worktree) -> Result<()> {
        let file = worktree.note(KEPT_AT_SETTLE)?;
        if file.exists() {
            std::fs::remove_file(file)?;
        }
        Ok(())
    }
}

/// Every worktree of this checkout a settle kept, with its note.
pub(crate) fn kept_at_settle(repo: &Repo) -> Result<Vec<(Worktree, KeptAtSettle)>> {
    let linked = Worktree::linked(&Source::of(repo)?)?;
    Ok(linked
        .into_iter()
        .filter_map(|(_, worktree)| {
            let kept = KeptAtSettle::of_worktree(&worktree)?;
            Some((worktree, kept))
        })
        .collect())
}

/// Looks again at each worktree a settle kept, whose claim is gone
/// (sunfmin/control#1161): it is removed once it holds nothing only it holds
/// — its commits on a branch of the remote, the carrier's among them while
/// its PR is open, or their change on the base branch once that PR, or the
/// round that took it, has landed — and no process is in it. One whose issue
/// is claimed again is that claim's: its note goes, and its end decides.
/// One that cannot be read or removed is that worktree's alone
/// (sunfmin/control#1204): each other is still looked at, and the answer
/// names each that failed, with why.
pub(crate) fn remove_kept(fleet: &Fleet) -> Result<Vec<Unremoved>> {
    let kept = kept_at_settle(&fleet.repo)?;
    if kept.is_empty() {
        return Ok(vec![]);
    }
    let open = fleet.gh.open_prs()?;
    let busy = crate::hot::working_directories()?;
    let mut unremoved = vec![];
    for (worktree, note) in kept {
        if crate::hot::occupies(&busy, &worktree.path) && fleet.scan.claim_of(note.issue).is_none()
        {
            continue;
        }
        let carrier = note
            .carried_by
            .and_then(|by| open.iter().find(|pr| pr.number == by))
            .map(|pr| pr.head_branch.as_str());
        if let Err(error) = look_again(fleet, &worktree, &note, carrier) {
            unremoved.push(Unremoved { path: worktree.path, issue: Some(note.issue), error });
        }
    }
    Ok(unremoved)
}

/// Looks again at one worktree a settle kept, as `remove_kept` says,
/// `carrier` the branch of the PR that carries its issue's while it is open.
fn look_again(
    fleet: &Fleet,
    worktree: &Worktree,
    note: &KeptAtSettle,
    carrier: Option<&str>,
) -> Result<()> {
    if fleet.scan.claim_of(note.issue).is_some() {
        return KeptAtSettle::forget(worktree);
    }
    let leaving = crate::judge::leaving_settled(fleet, note.issue, worktree, carrier)?;
    if afkconvoy_core::left::settle_keeps(&leaving.left) {
        if let Some(kept) = leaving.left.kept() {
            KeptAtSettle::of(note.issue, kept, note.carried_by, fleet.now).write(worktree)?;
        }
        return Ok(());
    }
    crate::account::refresh_stopped(&fleet.repo, &fleet.gh, note.issue)?;
    worktree.remove()
}

/// Settles my claim on `issue`, whose work is on the base branch: what only
/// the tick can do for it — `land` runs inside the worktree and holds no
/// instance id. The claim is deleted last, so a settling that failed on the
/// way is the next tick's to finish.
pub fn settle(fleet: &mut Fleet, found: &mut Found, me: &Me, issue: u64) -> Result<Option<u64>> {
    let closed = fleet.issues.issue(issue)?.is_closed();
    let base_tip = fleet.task_tip(issue)?;
    let landed = landing_of(&fleet.repo, &fleet.gh.tracker, &base_tip, issue)?.is_some();
    let comments = fleet.issues.comments(issue)?;
    // Another open PR carries its PR: its work goes on there (docs/adr/0083).
    let by = carried_by(comments.iter().map(|c| c.body.as_str()));
    let carried = match by {
        Some(by) if !closed && !landed => fleet.gh.open_prs()?.iter().any(|pr| pr.number == by),
        _ => false,
    };
    let accounted = afkconvoy_core::account::left(comments.iter().map(|c| c.body.as_str()));
    if let Some(end) = afkconvoy_core::account::settled_end(landed, accounted).filter(|_| !carried)
    {
        crate::account::leave(&fleet.repo, &fleet.gh, issue, end, fleet.now)?;
    }
    let steps = if carried { carried_steps() } else { settle_steps(closed) };
    for step in steps {
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
                fleet.issues.close_issue(issue)?;
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
                // The proving branch's repair worker keeps its worktree
                // while the repo declares a proving branch (docs/adr/0090).
                let read = fleet.issues.issue(issue)?;
                if fleet.config.proving_branch.is_some()
                    && crate::promotion_repair::is_any(&read.labels, read.body.as_deref())
                {
                    continue;
                }
                let carrier = match by.filter(|_| carried) {
                    Some(by) => fleet.gh.open_prs()?.into_iter().find(|pr| pr.number == by),
                    None => None,
                };
                let carried_by = carrier.as_ref().map(|pr| (pr.number, pr.head_branch.as_str()));
                if let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?
                    && let Some(since) = remove_settled(fleet, found, issue, &worktree, carried_by)?
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

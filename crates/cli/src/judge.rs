//! A claim with no PR: asking after its worker, and the transitions that end
//! it when the worker stopped — `close` and `escalate`.

use std::time::UNIX_EPOCH;

use afkconvoy_core::blocked::{Blocked, blocked_route};
use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::{Claim, releasable};
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::nudge_line;
use afkconvoy_core::stopped::{
    NudgeStep, Progress, Route, Silent, Tail, Unnudged, Workless, route, screen_request,
    settled_by_state, stalled_reason, standing_verdict,
};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition;
use afkconvoy_core::transition::{
    CloseStep, EscalateStep, Escalation, Relabel, Released, escalation_begun, escalation_comment,
};
use afkconvoy_core::verdict::{Verdict, latest_verdict};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::board::last_word;
use crate::claims::{Me, delete};
use crate::copies::Copies;
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::orca::{Source, Worktree};
use crate::park::{park_claim, standings};
use crate::team::keep;

/// A worktree's git progress against the base branch's tip on the remote —
/// never the local branch of that name, against which a worktree freshly cut
/// from the remote would read as ahead with no work in it.
pub fn progress(worktree: &Worktree, base_tip: &str) -> Result<Progress> {
    let repo = Repo { dir: worktree.path.clone() };
    let ahead = repo.try_git(&["rev-list", "--count", &format!("{base_tip}..HEAD")])?;
    let committed = repo.try_git(&["log", "-1", "--format=%ct"])?;
    let dirty = !repo.git_raw(&["status", "--porcelain"])?.trim().is_empty();
    // What git would stage, as the worker left it: an ignored build directory says nothing.
    let listed = repo.git_raw(&["ls-files", "-z", "--cached", "--others", "--exclude-standard"])?;
    let touched = listed
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::fs::symlink_metadata(repo.dir.join(path)).ok()?.modified().ok())
        .filter_map(|at| Some(at.duration_since(UNIX_EPOCH).ok()?.as_secs()))
        .max();
    Ok(Progress {
        commits_ahead: ahead.stdout.parse().ok().filter(|_| ahead.ok),
        dirty,
        last_commit: committed.stdout.parse().ok().filter(|_| committed.ok),
        touched,
    })
}

/// My claim on `issue`: `None` when the issue has none — already gone counts
/// as released. Every transition that ends a claim refuses another's.
pub(crate) fn mine(fleet: &Fleet, me: &Me, issue: u64) -> Result<Option<Claim>> {
    Ok(releasable(fleet.scan.claim_of(issue), &me.instance)?.cloned())
}

pub(crate) fn nothing_held(issue: u64) -> Value {
    json!({"issue": issue, "released": true, "detail": "no claim is held on it: nothing was done"})
}

/// The worktree of `issue` on this machine, and the proof that its branch
/// holds no work when it does not.
pub(crate) fn worktree_and_work(
    fleet: &Fleet,
    issue: u64,
) -> Result<(Option<Worktree>, Option<Workless>)> {
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    let workless = match &worktree {
        Some(worktree) => Workless::of(&progress(worktree, &fleet.base_tip()?)?),
        None => None,
    };
    Ok((worktree, workless))
}

/// `afkconvoy escalate`: hand one of my claims to a human. The steps already
/// taken are read from GitHub and the rest are done in order — comment the
/// reason, swap what hands the issue to the fleet for the escalate label
/// (`transition::Relabel`), release the claim last —
/// so it can be run again from wherever it was cut, and posts one comment.
/// The PR, the branch and the worktree are kept, except a worktree whose
/// branch holds no work.
pub fn escalate(fleet: &mut Fleet, me: &Me, issue: u64, reason: &str) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    let pr = closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number);
    escalate_claim(fleet, &claim, reason, pr, worktree.as_ref(), workless.as_ref())?;
    Ok(json!({"issue": issue, "action": "escalated", "released": true,
        "worktree": if workless.is_some() { "removed" } else { "kept" }}))
}

/// Escalates `claim`. `pr` is the issue's open PR, when it has one: the
/// status board keeps how far the claim got.
pub(crate) fn escalate_claim(
    fleet: &mut Fleet,
    claim: &Claim,
    reason: &str,
    pr: Option<u64>,
    worktree: Option<&Worktree>,
    workless: Option<&Workless>,
) -> Result<Released> {
    let issue = claim.number;
    let labels = fleet.gh.issue(issue)?.labels;
    let comments = fleet.gh.comments(issue)?;
    let said = escalation_begun(comments.iter().map(|c| c.body.as_str()), &claim.sha);
    // A bound repo's escalate label is its team's to name.
    let team_label = match &fleet.config.bound {
        Some(bound) => Some(keep(&fleet.gh, &Copies::here()?, &bound.team)?.team.escalate_label),
        None => None,
    };
    let swap = Relabel::of(&labels, &fleet.config, team_label.as_deref());
    let begun = Escalation::read(said, swap.done());
    transition::escalate(issue, begun, workless, |step| {
        match step {
            EscalateStep::Comment => {
                fleet.gh.comment(issue, &escalation_comment(&claim.sha, reason))?
            }
            EscalateStep::Relabel => {
                if let Some(label) = &swap.add {
                    fleet.gh.add_label(issue, label)?;
                }
                for label in &swap.remove {
                    fleet.gh.remove_label(issue, label)?;
                }
            }
            EscalateStep::Board => last_word(fleet, claim, Stage::Escalated, pr, &[])?,
            EscalateStep::Release => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            EscalateStep::RemoveWorktree => {
                if let Some(worktree) = worktree {
                    worktree.remove()?;
                }
            }
        }
        Ok(())
    })
}

/// `afkconvoy close`: close one of my claims whose issue needed no change —
/// close the issue → release → remove the worktree. Refused unless the
/// worktree here shows a branch with no commit past the base and nothing
/// uncommitted.
pub fn close(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    let Some(workless) = workless else {
        bail!(
            "issue #{issue} is not closed as needing no change: {}; nothing was changed",
            match worktree {
                Some(_) => "its branch holds work (commits past the base, or uncommitted files)",
                None => "it has no worktree on this host to show its branch holds no work",
            }
        );
    };
    close_claim(fleet, &claim, worktree.as_ref(), &workless)?;
    Ok(json!({"issue": issue, "action": "closed", "released": true}))
}

pub(crate) fn close_claim(
    fleet: &mut Fleet,
    claim: &Claim,
    worktree: Option<&Worktree>,
    workless: &Workless,
) -> Result<Released> {
    let issue = claim.number;
    transition::close(issue, workless, |step| {
        match step {
            CloseStep::CloseIssue => fleet.gh.close_issue(issue)?,
            CloseStep::Board => last_word(fleet, claim, Stage::Closed, None, &[])?,
            CloseStep::Release => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            CloseStep::RemoveWorktree => {
                if let Some(worktree) = worktree {
                    worktree.remove()?;
                }
            }
        }
        Ok(())
    })
}

/// A stopped worker of mine with no PR, and what was gathered to route it.
struct Stopped {
    claim: Claim,
    worktree: Worktree,
    progress: Progress,
    /// The verdict that stands for the worker here.
    verdict: Option<Verdict>,
    /// The URL of the comment that verdict is in.
    said: Option<String>,
    route: Route,
}

/// What asking after the worker of a PR-less claim of mine came to.
enum Asked {
    /// Its state alone settles it (`stopped::settled_by_state`): nothing on
    /// GitHub was read.
    Settled(Route),
    Stopped(Box<Stopped>),
}

/// Asks after the worker of my claim on `issue`; `None` when I hold none. A
/// worker at work, or gone, costs no GitHub read.
fn ask_after(fleet: &Fleet, me: &Me, issue: u64) -> Result<Option<Asked>> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(None) };
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    let state = match &worktree {
        Some(worktree) => Some(worktree.state(fleet.now)?),
        None => None,
    };
    if let Some(settled) = settled_by_state(state) {
        return Ok(Some(Asked::Settled(settled)));
    }
    let (Some(worktree), Some(state)) = (worktree, state) else {
        bail!("issue #{issue}: a worker that stopped has a worktree and a state");
    };
    let progress = progress(&worktree, &fleet.base_tip()?)?;
    let comments = fleet.gh.comments(issue)?;
    let verdict = latest_verdict(comments.iter().map(|c| c.body.as_str()));
    let said =
        verdict.as_ref().and_then(|verdict| Some(comments.get(verdict.comment)?.url.clone()));
    let verdict = standing_verdict(verdict, said.as_deref(), worktree.answered()?.as_deref());
    let said = said.filter(|_| verdict.is_some());
    let route = route(state, &progress, verdict.as_ref(), worktree.nudged()?, fleet.now);
    Ok(Some(Asked::Stopped(Box::new(Stopped { claim, worktree, progress, verdict, said, route }))))
}

/// Types a nudge at the worker in `worktree` — `line` makes it from where the
/// worker's brief is — and records it there.
pub(crate) fn nudge_worker(
    worktree: &Worktree,
    unnudged: Unnudged,
    line: fn(&str) -> String,
    now: u64,
) -> Result<String> {
    let Some(terminal) = worktree.terminal()? else {
        bail!("no live terminal in {} to take the nudge", worktree.path.display());
    };
    let brief = worktree.brief()?.display().to_string();
    afkconvoy_core::stopped::nudge(unnudged, |step| match step {
        NudgeStep::Tell => worktree.tell(&terminal, &line(&brief)),
        NudgeStep::Record => worktree.record_nudge(now),
    })?;
    Ok(terminal)
}

/// The end of a silent worker's screen, for the reason its claim is escalated
/// with. The read is soft: an escalation never waits on a terminal.
pub(crate) fn where_it_stopped(worktree: &Worktree, silent: &Silent) -> Tail {
    let screen = worktree.terminal().and_then(|terminal| match terminal {
        Some(terminal) => worktree.screen(&terminal, screen_request(silent)),
        None => Ok(vec![]),
    });
    Tail::of(&screen.unwrap_or_default())
}

/// `afkconvoy nudge`: tell the worker of one of my claims, stopped with no PR
/// and no verdict, to carry on. Refused for any worker the tick would not
/// nudge — at work, heard from within grace, one that declared a verdict, or
/// one already nudged.
pub fn nudge(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Value> {
    let Some(Asked::Stopped(found)) = ask_after(fleet, me, issue)? else {
        bail!(
            "issue #{issue} has no stopped worker of {} on this host to nudge; nothing was sent",
            me.instance
        );
    };
    let found = *found;
    let Route::Nudge(unnudged) = found.route else {
        bail!(
            "the worker on issue #{issue} is not nudged: {}; nothing was sent",
            match found.route {
                Route::Stalled(_) => "it was already nudged, and a second silence escalates",
                Route::Leave => "it showed a sign of life within the grace period",
                _ => "it declared a verdict",
            }
        );
    };
    let terminal = nudge_worker(&found.worktree, unnudged, nudge_line, fleet.now)?;
    Ok(json!({"issue": issue, "action": "nudged", "terminal": terminal}))
}

/// The tick's look at one PR-less claim of mine: a worker that is gone is
/// continued, one that stopped is routed (`stopped::route`) → `Done` when that
/// ended the claim.
pub fn judge(fleet: &mut Fleet, me: &Me, agent: &Agent, issue: u64) -> Result<Outcome> {
    let found = match ask_after(fleet, me, issue)? {
        None | Some(Asked::Settled(Route::Leave)) => return Ok(Outcome::Nothing),
        Some(Asked::Settled(_)) => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        Some(Asked::Stopped(found)) => *found,
    };
    let Stopped { claim, worktree, progress, verdict, said, route } = found;
    let workless = Workless::of(&progress);
    let said = said.as_deref();
    let reason = match route {
        Route::Leave => return Ok(Outcome::Nothing),
        Route::Continue => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        Route::Nudge(unnudged) => {
            nudge_worker(&worktree, unnudged, nudge_line, fleet.now)?;
            return Ok(Outcome::Nudged);
        }
        Route::Close(workless) => {
            close_claim(fleet, &claim, Some(&worktree), &workless)?;
            return Ok(Outcome::Done);
        }
        Route::Blocked => {
            let named = verdict.map(|verdict| verdict.blocked_by).unwrap_or_default();
            match blocked_route(&standings(fleet, issue, &named)?) {
                Blocked::Park(parkable) => {
                    park_claim(fleet, &claim, &parkable, Some(&worktree), workless.as_ref())?;
                    return Ok(Outcome::Done);
                }
                Blocked::Redispatch => {
                    // Every blocker closed: the issue is worked again, from what is here.
                    dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
                    // Last: a start that failed is made again by the next tick.
                    if let Some(said) = said {
                        worktree.record_answered(said)?;
                    }
                    return Ok(Outcome::Continued);
                }
                Blocked::Escalate(reason) => {
                    format!("{reason}: {}", said.unwrap_or("(its comment)"))
                }
            }
        }
        Route::Escalate(why) => why.reason(said.unwrap_or("(its comment)")),
        Route::Stalled(silent) => stalled_reason(&silent, &where_it_stopped(&worktree, &silent)),
    };
    // A claim the tick judges has no PR.
    escalate_claim(fleet, &claim, &reason, None, Some(&worktree), workless.as_ref())?;
    Ok(Outcome::Done)
}

/// A label as one path segment of a GitHub API URL.
pub fn label_segment(label: &str) -> String {
    let mut out = String::new();
    for byte in label.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

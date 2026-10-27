//! A claim with no PR: asking after its worker, and the transitions that end
//! it when the worker stopped — `close` and `escalate`.

use std::time::UNIX_EPOCH;

use afkconvoy_core::blocked::{Blocked, blocked_route};
use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::{Claim, releasable};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::left::{Held, Left};
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::nudge_line;
use afkconvoy_core::role::followed;
use afkconvoy_core::stage_record::stage_records;
use afkconvoy_core::stopped::{
    Background, FOOTER_LINES, NudgeStep, Progress, Route, Silent, Tail, Unnudged, Why, Workless,
    past_grace, route, screen_request, settled_by_state, stalled_reason, standing_verdict,
};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition;
use afkconvoy_core::transition::{
    CloseStep, EscalateStep, Escalation, FleetFault, Relabel, Released, escalation_begun,
    escalation_comment,
};
use afkconvoy_core::verdict::{Verdict, latest_verdict};
use afkconvoy_core::waiting::{Back, Waits, back};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::board::{escalated, last_word};
use crate::claims::{Me, delete};
use crate::copies::Copies;
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::exclusive;
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::orca::{Source, Worktree};
use crate::park::{park_claim, standings};
use crate::team::keep;

/// A worktree's git progress since its merge base with the base branch on
/// the remote. Changes landed only on the base branch are no work of this
/// branch's, and the local base branch may be out of date.
pub fn progress(worktree: &Worktree, base_tip: &str) -> Result<Progress> {
    let repo = Repo::new(worktree.path.clone());
    let merge_base = repo.try_git(&["merge-base", base_tip, "HEAD"])?;
    let ahead = repo.try_git(&["rev-list", "--count", &format!("{}..HEAD", merge_base.stdout)])?;
    let base_tree =
        repo.try_git(&["rev-parse", "--verify", &format!("{}^{{tree}}", merge_base.stdout)])?;
    let head_tree = repo.try_git(&["rev-parse", "--verify", "HEAD^{tree}"])?;
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
        commits_ahead: ahead.stdout.parse().ok().filter(|_| merge_base.ok && ahead.ok),
        trees_equal: (merge_base.ok && base_tree.ok && head_tree.ok)
            .then_some(base_tree.stdout == head_tree.stdout),
        dirty,
        last_commit: committed.stdout.parse().ok().filter(|_| committed.ok),
        touched,
    })
}

/// What refutes an `already-satisfied` verdict, including the committed and
/// uncommitted paths a worker must account for. Read failures stay refusals.
pub(crate) fn refuted_work(
    fleet: &Fleet,
    issue: u64,
    worktree: Option<&Worktree>,
) -> Result<String> {
    let Some(worktree) = worktree else {
        return Ok("it has no worktree on this host to show its branch holds no work".to_string());
    };
    let repo = Repo::new(worktree.path.clone());
    let changed = repo.try_git(&[
        "diff",
        "--no-ext-diff",
        "--name-only",
        "--no-renames",
        &format!("{}...HEAD", fleet.task_tip(issue)?),
        "--",
    ])?;
    let dirty = repo.git_raw(&["status", "--porcelain"])?;
    let mut reason = "its branch holds work, or its tree could not be read".to_string();
    if !changed.ok {
        reason.push_str(&format!("\n\nTree comparison failed: {}", changed.stderr));
    } else if !changed.stdout.is_empty() {
        reason.push_str(&format!("\n\nFiles changed since the merge base:\n{}", changed.stdout));
    }
    if !dirty.trim().is_empty() {
        reason.push_str(&format!("\n\nUncommitted files:\n{dirty}"));
    }
    Ok(reason)
}

/// My claim on `issue`: `None` when the issue has none — already gone counts
/// as released. Every transition that ends a claim refuses another's.
pub(crate) fn mine(fleet: &Fleet, me: &Me, issue: u64) -> Result<Option<Claim>> {
    Ok(releasable(fleet.scan.claim_of(issue), &me.instance)?.cloned())
}

pub(crate) fn nothing_held(issue: u64) -> Value {
    json!({"issue": issue, "released": true, "detail": "no claim is held on it: nothing was done"})
}

/// The worktree of `issue` on this machine.
pub(crate) fn worktree_of(fleet: &Fleet, issue: u64) -> Result<Option<Worktree>> {
    Worktree::of_issue(&Source::of(&fleet.repo)?, issue)
}

/// What a released claim leaves of its worktree, and the subjects of the
/// commits only it holds, newest first.
pub(crate) struct Leaving {
    pub left: Left,
    pub unpushed: Vec<String>,
}

/// What `worktree` holds that the remote does not: anything uncommitted, and
/// the commits of its branch on neither the branch the remote has of that
/// name nor the base branch → what becomes of it as its claim ends.
pub(crate) fn leaving(fleet: &Fleet, issue: u64, worktree: &Worktree) -> Result<Leaving> {
    let mut known = vec![fleet.task_tip(issue)?];
    known.extend(fleet.repo.fetched(&format!("refs/heads/{}", worktree.branch))?);
    let repo = Repo::new(worktree.path.clone());
    let dirty = !repo.git_raw(&["status", "--porcelain"])?.trim().is_empty();
    let mut args = vec!["log", "--format=%s", "HEAD", "--not"];
    args.extend(known.iter().map(String::as_str));
    let logged = repo.try_git(&args)?;
    let unpushed: Vec<String> = match logged.ok {
        true => logged.stdout.lines().map(str::to_string).collect(),
        false => vec![],
    };
    let count = logged.ok.then_some(unpushed.len() as u64);
    Ok(Leaving { left: Left::of(&Held { dirty, unpushed: count }), unpushed })
}

/// What a released claim left of its worktree here, as a command answers it:
/// `worktree` is "removed", "kept" or "none" — none on this host — and one
/// kept says why (`kept_because`) and how many commits only it holds.
pub(crate) fn left_json(left: Option<&Left>) -> Value {
    match left {
        None => json!({"worktree": "none"}),
        Some(Left::Removed(_)) => json!({"worktree": "removed"}),
        Some(Left::Kept(kept)) => json!({"worktree": "kept",
            "kept_because": kept.because.key(), "unpushed": kept.unpushed}),
    }
}

/// Adds what `left_json` says to the object `answer`.
pub(crate) fn with_left(mut answer: Value, left: Option<&Left>) -> Value {
    if let (Some(answer), Value::Object(said)) = (answer.as_object_mut(), left_json(left)) {
        answer.extend(said);
    }
    answer
}

/// The worktree of `issue` on this machine, and the proof that its branch
/// holds no work when it does not.
pub(crate) fn worktree_and_work(
    fleet: &Fleet,
    issue: u64,
) -> Result<(Option<Worktree>, Option<Workless>)> {
    let worktree = worktree_of(fleet, issue)?;
    let workless = match &worktree {
        Some(worktree) => Workless::of(&progress(worktree, &fleet.task_tip(issue)?)?),
        None => None,
    };
    Ok((worktree, workless))
}

/// `afkconvoy escalate`: hand one of my claims to a human. The steps already
/// taken are read from GitHub and the rest are done in order — comment the
/// reason, swap what hands the issue to the fleet for the escalate label
/// (`transition::Relabel`), release the claim last —
/// so it can be run again from wherever it was cut, and posts one comment.
/// The PR and the branch are kept, and so is the worktree when it holds what
/// the remote does not; the terminal of its worker is closed.
pub fn escalate(fleet: &mut Fleet, me: &Me, issue: u64, reason: &str) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let worktree = worktree_of(fleet, issue)?;
    let pr = closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number);
    let why = Handed::new("by hand", reason.to_string());
    let left = escalate_claim(fleet, &claim, &why, pr, worktree.as_ref())?;
    let answer = json!({"issue": issue, "action": "escalated", "released": true});
    Ok(with_left(answer, left.as_ref()))
}

/// Why a claim is escalated, said twice.
pub(crate) struct Handed {
    /// In the few words a tick's line has room for.
    pub brief: String,
    /// As the comment on the issue opens.
    pub reason: String,
    /// A host fix is needed: no consult is asked on a later tick.
    fault: Option<FleetFault>,
    refusal_exhausted: bool,
}

impl Handed {
    pub(crate) fn new(brief: &str, reason: String) -> Handed {
        Handed { brief: brief.to_string(), reason, fault: None, refusal_exhausted: false }
    }

    pub(crate) fn fleet_fault(fault: FleetFault, reason: String) -> Handed {
        Handed {
            brief: fault.brief().to_string(),
            reason,
            fault: Some(fault),
            refusal_exhausted: false,
        }
    }

    pub(crate) fn refusal_exhausted(reason: String) -> Handed {
        Handed {
            brief: "no harness left after task refusal".into(),
            reason,
            fault: None,
            refusal_exhausted: true,
        }
    }

    /// The same of a claim at the stage `stage`, which the brief opens with.
    pub(crate) fn at(stage: Option<&str>, brief: &str, reason: String) -> Handed {
        match stage {
            Some(stage) => Handed::new(&format!("{stage}: {brief}"), reason),
            None => Handed::new(brief, reason),
        }
    }
}

/// Escalates `claim`, for `why`. `pr` is the issue's open PR, when it has
/// one: the status board keeps how far the claim got. The comment and the
/// status board end with the lines that answer the escalation
/// (`waiting::Waits::answer_lines`), and the fleet's view notes it, for the
/// tick's line. `worktree` is the issue's on this host, when it has one: it
/// is removed when it holds nothing the remote does not, and kept otherwise —
/// the comment then names what only it holds → what became of it.
pub(crate) fn escalate_claim(
    fleet: &mut Fleet,
    claim: &Claim,
    why: &Handed,
    pr: Option<u64>,
    worktree: Option<&Worktree>,
) -> Result<Option<Left>> {
    let issue = claim.number;
    let leaving = worktree.map(|worktree| leaving(fleet, claim.number, worktree)).transpose()?;
    let labels = fleet.gh.issue(issue)?.labels;
    let comments = fleet.gh.comments(issue)?;
    let bodies = || comments.iter().map(|c| c.body.as_str());
    let said = escalation_begun(bodies(), &claim.id);
    // A bound repo's escalate label is its team's to name.
    let kept =
        match fleet.config.bound.as_ref().filter(|_| !afkconvoy_core::plan::is_repair(&labels)) {
            Some(bound) => {
                Some(keep(&fleet.gh, &Copies::of_host()?, &bound.team, &fleet.config.labels())?)
            }
            None => None,
        };
    let team = kept.as_ref().map(|kept| &kept.team);
    let swap = Relabel::of(&labels, &fleet.config, team);
    // The one workflow a bound repo's issue follows: a ruling puts its label back.
    let flow = team.and_then(|team| match team.flows(&labels).as_slice() {
        [flow] => team.labelled(flow).map(|workflow| workflow.name.clone()),
        _ => None,
    });
    let back = match &kept {
        Some(kept) => {
            // An escalation cut short after its relabel reads the workflow
            // off what it left, as a ruling does.
            let workflow = followed(&kept.team, &labels, bodies()).ok();
            let records: Vec<_> = stage_records(bodies()).into_iter().map(|(_, r)| r).collect();
            back(&kept.team, workflow, &records)
        }
        None => Back::Ready {
            ready: fleet.config.ready_label.clone(),
            escalate: fleet.config.escalate_label.clone(),
        },
    };
    let answer = Waits::Escalated(back).answer_lines(issue, &fleet.gh.tracker);
    let begun = Escalation::read(said, swap.done());
    let spare = leaving.as_ref().and_then(|leaving| leaving.left.spare());
    let only_here = match (&leaving, worktree) {
        (Some(leaving), Some(worktree)) => only_here(claim, worktree, leaving),
        _ => None,
    };
    transition::escalate(issue, begun, spare, |step| {
        match step {
            EscalateStep::Comment => {
                let reason = match &only_here {
                    Some(only_here) => format!("{}\n\n{only_here}\n\n{answer}", why.reason.trim()),
                    None => format!("{}\n\n{answer}", why.reason.trim()),
                };
                let comment = match why.fault {
                    Some(fault) => fault.comment(&claim.id, flow.as_deref(), &reason),
                    None if why.refusal_exhausted => {
                        transition::refusal_exhausted_comment(&claim.id, flow.as_deref(), &reason)
                    }
                    None => escalation_comment(&claim.id, flow.as_deref(), &reason),
                };
                fleet.gh.comment(issue, &comment)?
            }
            EscalateStep::Relabel => {
                if let Some(label) = &swap.add {
                    fleet.gh.add_label(issue, label)?;
                }
                for label in &swap.remove {
                    fleet.gh.remove_label(issue, label)?;
                }
            }
            EscalateStep::Board => {
                let kept = leaving.as_ref().and_then(|leaving| leaving.left.kept().copied());
                escalated(fleet, claim, pr, &answer, kept)?
            }
            EscalateStep::Release => {
                delete(&fleet.repo, &mut fleet.scan, issue, &claim.sha)?;
            }
            EscalateStep::RemoveWorktree => {
                if let Some(worktree) = worktree {
                    worktree.remove()?;
                }
            }
            EscalateStep::CloseTerminal => {
                if let Some(worktree) = worktree {
                    worktree.close_terminals()?;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;
    fleet.escalated.push((issue, why.brief.clone()));
    Ok(leaving.map(|leaving| leaving.left))
}

/// What an escalation says of a worktree kept for what only it holds, so that
/// whoever answers knows that work is on one host alone: what it holds, and
/// each commit the remote lacks by its subject. `None` of one that goes.
fn only_here(claim: &Claim, worktree: &Worktree, leaving: &Leaving) -> Option<String> {
    let kept = leaving.left.kept()?;
    let host = claim.owner.as_ref().map(|owner| owner.host.as_str()).unwrap_or("its host");
    let mut lines = vec![format!(
        "**Work only one host holds**: the worktree `{}` on `{host}` is kept — it holds {}.",
        worktree.path.display(),
        kept.holds()
    )];
    lines.extend(leaving.unpushed.iter().map(|subject| format!("- {subject}")));
    Some(lines.join("\n"))
}

/// `afkconvoy close`: close one of my claims whose issue needed no change —
/// close the issue → release → remove the worktree. Refused unless the
/// worktree here shows no commits or tree changes past the merge base and
/// nothing uncommitted.
pub fn close(fleet: &mut Fleet, me: &Me, issue: u64) -> Result<Value> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(nothing_held(issue)) };
    let (worktree, workless) = worktree_and_work(fleet, issue)?;
    let Some(workless) = workless else {
        bail!(
            "issue #{issue} is not closed as needing no change: {}; nothing was changed",
            refuted_work(fleet, issue, worktree.as_ref())?
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

/// A `blocked` no park answers, in the few words a tick's line has room for.
pub(crate) const BLOCKED_BRIEF: &str = "blocked by what the fleet does not work";

/// A stopped worker of mine with no PR, and what was gathered to route it.
struct Stopped {
    claim: Claim,
    worktree: Worktree,
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
    let progress = progress(&worktree, &fleet.task_tip(issue)?)?;
    let comments = fleet.gh.comments(issue)?;
    let bodies = || comments.iter().map(|c| c.body.as_str());
    let verdict = latest_verdict(bodies());
    let said =
        verdict.as_ref().and_then(|verdict| Some(comments.get(verdict.comment)?.url.clone()));
    let verdict = standing_verdict(verdict, said.as_deref(), worktree.answered()?.as_deref());
    let said = said.filter(|_| verdict.is_some());
    let handed = worktree.handed(None)?;
    // What the worker said is carried out whatever it waits on: its screen is
    // read only when it said nothing.
    let nudged = worktree.nudged()?;
    let background = match verdict {
        Some(_) => None,
        None => {
            let past_grace = past_grace(state, &progress, nudged, handed, fleet.now);
            own_background(fleet, &worktree, past_grace)?
        }
    };
    let route = route(state, &progress, verdict.as_ref(), nudged, (handed, background), fleet.now);
    Ok(Some(Asked::Stopped(Box::new(Stopped { claim, worktree, verdict, said, route }))))
}

/// The command doctor can name for a stopped PR-less claim. Use the same
/// reading and routing as the tick and `nudge`: a verdict, recent progress,
/// background work or a prior nudge must never receive a refused nudge fix.
/// Cases that need the tick's consult or health handling name `tick` itself.
pub(crate) fn stopped_command(fleet: &Fleet, me: &Me, issue: u64) -> Result<&'static str> {
    let Some(Asked::Stopped(found)) = ask_after(fleet, me, issue)? else { return Ok("tick") };
    if crate::health::holding(&found.worktree) {
        return Ok("tick");
    }
    Ok(match found.route {
        Route::Nudge(_) => "nudge",
        Route::Blocked => {
            let named = found.verdict.map(|verdict| verdict.blocked_by).unwrap_or_default();
            match blocked_route(&standings(fleet, issue, &named)?) {
                Blocked::Redispatch => "dispatch",
                Blocked::Park(_) => "park",
                Blocked::Escalate(_) => "tick",
            }
        }
        _ => "tick",
    })
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
        NudgeStep::Tell => worktree.tell_at(&terminal, &line(&brief), now),
        NudgeStep::Record => worktree.record_nudge(now),
    })?;
    Ok(terminal)
}

/// The background work the stopped worker in `worktree` started itself and
/// waits on, as its harness shows it at the end of its screen — or as the
/// host's `exclusive` queue holds a command of its: since when it
/// was first seen here, under the gate limit. Read only of a worker
/// `past_grace` (`stopped::past_grace`) — one with a sign of life is not
/// waiting, and is seen waiting afresh when it next stops — and soft: a
/// terminal that cannot be read shows none, and a silence never waits on one.
pub(crate) fn own_background(
    fleet: &Fleet,
    worktree: &Worktree,
    past_grace: bool,
) -> Result<Option<Background>> {
    if !past_grace {
        worktree.background_since(false, fleet.now)?;
        return Ok(None);
    }
    let screen = worktree.terminal().and_then(|terminal| match terminal {
        Some(terminal) => worktree.screen(&terminal, FOOTER_LINES),
        None => Ok(vec![]),
    });
    let waiting = Harness::any_shows_background(&screen.unwrap_or_default());
    let since = worktree.background_since(waiting, fleet.now)?;
    // A command of its in the host's `exclusive` queue is work it waits on
    // too, since it asked to run: busy, not silent (docs/adr/0019).
    let since = match (since, exclusive::queued_in(&worktree.path)) {
        (Some(seen), Some(queued)) => Some(seen.min(queued)),
        (seen, queued) => seen.or(queued),
    };
    Ok(since.map(|since| Background { since, limit: fleet.config.gate_seconds() }))
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
                Route::Waiting(_) => "it waits on background work of its own, still running",
                _ => "it declared a verdict",
            }
        );
    };
    let terminal = nudge_worker(&found.worktree, unnudged, nudge_line, fleet.now)?;
    Ok(json!({"issue": issue, "action": "nudged", "terminal": terminal}))
}

/// The tick's look at one PR-less claim of mine: a worker that is gone is
/// continued, one that stopped is routed (`stopped::route`) → `Done` when that
/// ended the claim. `start` false: no worker is started under the claim — its
/// worker declared, and no slot is free — and what would start one is left
/// for a later tick, as `Nothing`.
pub fn judge(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    issue: u64,
    start: bool,
) -> Result<Outcome> {
    let found = match ask_after(fleet, me, issue)? {
        None | Some(Asked::Settled(Route::Leave)) => return Ok(Outcome::Nothing),
        Some(Asked::Settled(_)) if !start => return Ok(Outcome::Nothing),
        Some(Asked::Settled(_)) => {
            dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            return Ok(Outcome::Continued);
        }
        Some(Asked::Stopped(found)) => *found,
    };
    let Stopped { claim, worktree, verdict, said, route } = found;
    let said = said.as_deref();
    let reason = match route {
        Route::Leave => return Ok(Outcome::Nothing),
        Route::Waiting(since) => return Ok(Outcome::Waiting(since)),
        Route::Continue if !start => return Ok(Outcome::Nothing),
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
                    park_claim(fleet, &claim, &parkable, Some(&worktree))?;
                    return Ok(Outcome::Done);
                }
                Blocked::Redispatch if !start => return Ok(Outcome::Nothing),
                Blocked::Redispatch => {
                    // Every blocker closed: the issue is worked again, from what is here.
                    dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
                    return Ok(Outcome::Continued);
                }
                Blocked::Escalate(reason) => Handed::new(
                    BLOCKED_BRIEF,
                    format!("{reason}: {}", said.unwrap_or("(its comment)")),
                ),
            }
        }
        Route::Escalate(why) => {
            let mut reason = why.reason(said.unwrap_or("(its comment)"));
            if why == Why::Refuted {
                reason.push_str(&format!("\n\n{}", refuted_work(fleet, issue, Some(&worktree))?));
            }
            Handed::new(why.brief(), reason)
        }
        Route::Stalled(silent) => Handed::new(
            "its worker went silent",
            stalled_reason(&silent, &where_it_stopped(&worktree, &silent)),
        ),
    };
    // A claim the tick judges has no PR.
    escalate_claim(fleet, &claim, &reason, None, Some(&worktree))?;
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

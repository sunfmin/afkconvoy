//! A claim with no PR: asking after its worker, and the transitions that end
//! it when the worker stopped — `close` and `escalate`.

use std::time::UNIX_EPOCH;

use afkconvoy_core::account::End;
use afkconvoy_core::blocked::Blocked;
use afkconvoy_core::board::Stage;
use afkconvoy_core::claims::{Claim, releasable};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::left::{Held, Left};
use afkconvoy_core::plan::{Status, closing_pr};
use afkconvoy_core::prompt::nudge_line;
use afkconvoy_core::role::followed;
use afkconvoy_core::stage_record::stage_records;
use afkconvoy_core::stopped::{
    Background, FOOTER_LINES, NudgeStep, Progress, Route, Silence, Silent, Tail, Unnudged, Why,
    Workless, past_grace, route, screen_request, settled_by_state, silence, stalled_reason,
    standing_verdict,
};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition;
use afkconvoy_core::transition::{
    CloseStep, EscalateStep, Escalation, FleetFault, Relabel, Released, escalation_begun,
    escalation_comment,
};
use afkconvoy_core::verdict::Verdict;
use afkconvoy_core::waiting::{Back, Waits, back};
use afkconvoy_core::worker::WorkerState;
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
use crate::park::{park_claim, routed};
use crate::remote::Fresh;
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
    let mut progress = Progress {
        commits_ahead: ahead.stdout.parse().ok().filter(|_| merge_base.ok && ahead.ok),
        trees_equal: (merge_base.ok && base_tree.ok && head_tree.ok)
            .then_some(base_tree.stdout == head_tree.stdout),
        dirty,
        last_commit: committed.stdout.parse().ok().filter(|_| committed.ok),
        touched,
    };
    crate::turn_answer::worker_progress(worktree, &mut progress)?;
    Ok(progress)
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
    leaving_with(fleet, issue, worktree, false)
}

/// The same comparison against the fleet view's existing remote listing.
pub(crate) fn leaving_at_open(fleet: &Fleet, issue: u64, worktree: &Worktree) -> Result<Leaving> {
    leaving_with(fleet, issue, worktree, true)
}

fn leaving_with(fleet: &Fleet, issue: u64, worktree: &Worktree, at_open: bool) -> Result<Leaving> {
    let branch = format!("refs/heads/{}", worktree.branch);
    let mut known =
        if at_open { vec![fleet.task_tip_at_open(issue)?] } else { vec![fleet.task_tip(issue)?] };
    known.extend(if at_open {
        fleet.fetched_at_open(&branch)?
    } else {
        fleet.remote.fetched(&branch, Fresh::Process)?
    });
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
    let answer = crate::checkout::name_printed(
        &fleet.repo.dir,
        &Waits::Escalated(back).answer_lines(issue, &fleet.gh.tracker),
    );
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
            EscalateStep::Account => {
                crate::account::leave(&fleet.repo, &fleet.gh, issue, End::Escalated, fleet.now)?
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
            CloseStep::Account => {
                crate::account::leave(&fleet.repo, &fleet.gh, issue, End::Closed, fleet.now)?
            }
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

/// Who reads a worker's silence, and so whether the read moves anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    /// The tick, worker health and `nudge`: the read stamps the since of the
    /// worker's own background work (`Worktree::background_since`).
    Acting,
    /// `doctor`: the same judgment, writing nothing, so the since stays where
    /// the tick put it.
    Reporting,
}

/// Asks after the worker of my claim on `issue`; `None` when I hold none. A
/// worker at work, or gone, costs no GitHub read. A dispatch whose worker has
/// not taken its brief is no worker to judge: it is continued, never judged
/// silent (docs/adr/0005).
fn ask_after(fleet: &Fleet, me: &Me, issue: u64, look: Look) -> Result<Option<Asked>> {
    let Some(claim) = mine(fleet, me, issue)? else { return Ok(None) };
    if crate::dispatch::incomplete(fleet, issue)? {
        return Ok(Some(Asked::Settled(Route::Continue)));
    }
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    let state = match &worktree {
        Some(worktree) => Some(worktree.state(fleet.now)?),
        None => None,
    };
    if let Some(settled) = settled_by_state(state) {
        // An open turn has no since. A later wait starts when it is seen again.
        if look == Look::Acting
            && matches!(state, Some(WorkerState::Busy))
            && let Some(worktree) = &worktree
        {
            worktree.background_since(false, None, fleet.now)?;
            end_goal_past_verdict(fleet, &claim, worktree)?;
        }
        return Ok(Some(Asked::Settled(settled)));
    }
    let (Some(worktree), Some(state)) = (worktree, state) else {
        bail!("issue #{issue}: a worker that stopped has a worktree and a state");
    };
    let progress = progress(&worktree, &fleet.task_tip(issue)?)?;
    let handed = worktree.handed(None)?;
    let nudged = worktree.nudged()?;
    let past_grace = past_grace(state, &progress, nudged, handed, fleet.now);
    // Within grace no worker is silent, whatever it declared: a look for its
    // silence alone reads nothing on GitHub.
    if look == Look::Reporting && !past_grace {
        return Ok(Some(Asked::Settled(Route::Leave)));
    }
    let comments = fleet.gh.comments(issue)?;
    let verdict = crate::gh::verdict_since(&comments, claim.since);
    let said =
        verdict.as_ref().and_then(|verdict| Some(comments.get(verdict.comment)?.url.clone()));
    let verdict = standing_verdict(verdict, said.as_deref(), worktree.answered()?.as_deref());
    let said = said.filter(|_| verdict.is_some());
    // What the worker said is carried out whatever it waits on: its screen is
    // read only when it said nothing.
    let background = match verdict {
        Some(_) => None,
        None => own_background(fleet, &worktree, past_grace, look)?,
    };
    let route = route(state, &progress, verdict.as_ref(), nudged, (handed, background), fleet.now);
    Ok(Some(Asked::Stopped(Box::new(Stopped { claim, worktree, verdict, said, route }))))
}

/// Ends the goal of the busy worker in `worktree` once a verdict of its
/// stands: its harness would carry it on round its brief at cost, never
/// stopping for the tick to carry the verdict out. A worker its harness has
/// not continued since it was handed its brief costs no GitHub read: the
/// one at its first turn is at work, not past its verdict.
fn end_goal_past_verdict(fleet: &Fleet, claim: &Claim, worktree: &Worktree) -> Result<()> {
    let Some((harness, afkconvoy_core::harness::Goal::Active)) = crate::goal::of(worktree) else {
        return Ok(());
    };
    let since = worktree.handed(claim.since)?.unwrap_or_default();
    if !crate::goal::continued(worktree, harness, since)? {
        return Ok(());
    }
    let comments = fleet.gh.comments(claim.number)?;
    let verdict = crate::gh::verdict_since(&comments, claim.since);
    let said =
        verdict.as_ref().and_then(|verdict| Some(comments.get(verdict.comment)?.url.clone()));
    if standing_verdict(verdict, said.as_deref(), worktree.answered()?.as_deref()).is_some() {
        crate::goal::end(worktree, harness)?;
    }
    Ok(())
}

/// A held claim's worker found silent, with the command the tick runs next
/// for it: what `doctor` reports a silent worker from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Silenced {
    /// `nudge` for a worker due its one nudge with no PR; `tick` otherwise —
    /// the tick nudges a worker at a stage or landing with its own line,
    /// restarts or escalates one silent past its nudge.
    pub command: &'static str,
    /// It was nudged, and stayed silent a grace period more.
    pub nudged: bool,
    /// How long it waited on its own background work, past the gate limit.
    pub waited: Option<u64>,
    /// It declared a verdict past grace, which the tick has yet to carry out.
    pub declared: bool,
}

impl Silenced {
    fn declared(command: &'static str) -> Silenced {
        Silenced { command, nudged: false, waited: None, declared: true }
    }

    fn of(silence: Silence, nudge: &'static str) -> Option<Silenced> {
        match silence {
            Silence::Nudge(_) => {
                Some(Silenced { command: nudge, nudged: false, waited: None, declared: false })
            }
            Silence::Stalled(silent) => Some(Silenced {
                command: "tick",
                nudged: silent.nudged,
                waited: silent.waited,
                declared: false,
            }),
            Silence::Gone | Silence::Lively | Silence::Waiting(_) => None,
        }
    }
}

/// Whether the worker of my claim on `issue`, at `status`, is silent, through
/// the read the tick routes it by — writing nothing. `None` for one at work,
/// gone, within grace, waiting on its own background work inside the gate
/// limit, obeying its landing turn's wait, held by a shared harness outage,
/// or a dispatch whose worker has not taken its brief. One with no PR that
/// declared past grace is reported too, with the command that carries its
/// verdict out.
pub(crate) fn silenced(
    fleet: &Fleet,
    me: &Me,
    issue: u64,
    status: Status,
) -> Result<Option<Silenced>> {
    if status == Status::NoPr {
        let Some(Asked::Stopped(found)) = ask_after(fleet, me, issue, Look::Reporting)? else {
            return Ok(None);
        };
        if held_for_report(fleet, &found.worktree)? {
            return Ok(None);
        }
        return Ok(match found.route {
            Route::Nudge(unnudged) => Silenced::of(Silence::Nudge(unnudged), "nudge"),
            Route::Stalled(silent) => Silenced::of(Silence::Stalled(silent), "nudge"),
            Route::Blocked => {
                let named = found.verdict.map(|verdict| verdict.blocked_by).unwrap_or_default();
                let command = match routed(fleet, issue, &named)? {
                    Blocked::Redispatch => "dispatch",
                    Blocked::Park(_) => "park",
                    Blocked::Escalate(_) => "tick",
                };
                Some(Silenced::declared(command))
            }
            Route::Close(_) | Route::Escalate(_) => Some(Silenced::declared("tick")),
            Route::Leave | Route::Waiting(_) | Route::Continue => None,
        });
    }
    if mine(fleet, me, issue)?.is_none() || crate::dispatch::incomplete(fleet, issue)? {
        return Ok(None);
    }
    let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)? else {
        return Ok(None);
    };
    if crate::turn_answer::waiting(&worktree) {
        return Ok(None);
    }
    let state = worktree.state(fleet.now)?;
    if settled_by_state(Some(state)).is_some() || held_for_report(fleet, &worktree)? {
        return Ok(None);
    }
    let silence = silence_of(fleet, issue, &worktree, state, None, Look::Reporting)?;
    Ok(Silenced::of(silence, "tick"))
}

/// Whether worker health holds the worker in `worktree`, for a report that
/// runs no observation first. A recorded wait on background work holds only
/// while that work still shows: the record is as old as the last tick's
/// observation, and the tick re-observes before it acts.
fn held_for_report(fleet: &Fleet, worktree: &Worktree) -> Result<bool> {
    if !crate::health::holding(worktree) {
        return Ok(false);
    }
    if crate::health::holding_past_background(worktree) {
        return Ok(true);
    }
    Ok(own_background(fleet, worktree, true, Look::Reporting)?.is_some())
}

/// The silence of the stopped worker in `worktree` that declared nothing: its
/// progress, the nudge and the hand-off recorded here — `handed` whatever it
/// was handed after it started — and, past grace, its own background work.
pub(crate) fn silence_of(
    fleet: &Fleet,
    issue: u64,
    worktree: &Worktree,
    state: WorkerState,
    handed: Option<u64>,
    look: Look,
) -> Result<Silence> {
    let progress = progress(worktree, &fleet.task_tip(issue)?)?;
    let (nudged, handed) = (worktree.nudged()?, worktree.handed(handed)?);
    let past_grace = past_grace(state, &progress, nudged, handed, fleet.now);
    let background = own_background(fleet, worktree, past_grace, look)?;
    Ok(silence(state, &progress, nudged, handed, background, fleet.now))
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

/// What of a worker's own background work shows right now. Writes nothing.
struct SeenBackground {
    /// Its harness shows a shell or a monitor still running.
    showing: bool,
    /// Since when the exclusive queue has held a command of its.
    queued: Option<u64>,
}

/// The one read of a worker's own background work: the end of its screen, and
/// the host's exclusive queue. Soft, as `own_background`: a terminal that
/// cannot be read shows none.
fn read_own_background(worktree: &Worktree) -> SeenBackground {
    let screen = worktree.terminal().and_then(|terminal| match terminal {
        Some(terminal) => worktree.screen(&terminal, FOOTER_LINES),
        None => Ok(vec![]),
    });
    SeenBackground {
        showing: Harness::any_shows_background(&screen.unwrap_or_default()),
        queued: exclusive::queued_in(&worktree.path),
    }
}

/// The background work the stopped worker in `worktree` started itself and
/// waits on. One read of its screen and the exclusive queue answers both
/// whether any shows and since when, and that since is the one stamp
/// (`Worktree::background_since`): the tick line, worker health and a later
/// tick all name it. Read only of a worker `past_grace`
/// (`stopped::past_grace`) — one with a sign of life is not waiting, the
/// stamp is cleared, and a later wait starts when it is seen again. Soft: a
/// terminal that cannot be read shows none, and a silence never waits on one.
/// `Look::Reporting` reads the since the stamp would hold and writes nothing.
pub(crate) fn own_background(
    fleet: &Fleet,
    worktree: &Worktree,
    past_grace: bool,
    look: Look,
) -> Result<Option<Background>> {
    if !past_grace {
        if look == Look::Acting {
            worktree.background_since(false, None, fleet.now)?;
        }
        return Ok(None);
    }
    let seen = read_own_background(worktree);
    // A command of its in the host's `exclusive` queue is work it waits on
    // too, since it asked to run: busy, not silent (docs/adr/0019).
    let waiting = seen.showing || seen.queued.is_some();
    let since = match look {
        Look::Acting => worktree.background_since(waiting, seen.queued, fleet.now)?,
        Look::Reporting => waiting.then(|| worktree.background_seen(seen.queued, fleet.now)),
    };
    let limit = fleet.config.gate_seconds();
    // A land turn with a candidate on the chain waits on the chain, which
    // keeps its own time: past one gate's limit, as a round's members wait
    // for its slow gate, it is not silent. On 2026-10-29 a member restarted
    // so ended its round's slow gate (sunfmin/control#244).
    let prefix = fleet.repo.layout().under("candidates/");
    if waiting
        && crate::candidates::landing(
            &fleet.records_at_open(&format!("{prefix}*")),
            &prefix,
            &worktree.branch,
        )
    {
        return Ok(Some(Background { since: fleet.now, limit }));
    }
    Ok(since.map(|since| Background { since, limit }))
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
    let Some(Asked::Stopped(found)) = ask_after(fleet, me, issue, Look::Acting)? else {
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
    let found = match ask_after(fleet, me, issue, Look::Acting)? {
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
            match routed(fleet, issue, &named)? {
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

/// What `afkconvoy nudge --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const NUDGE_HELP: &str = r#"## When to run it

To send a worker its nudge by hand. A tick does this on its own only when worker health or the capacity rule asks the worker to carry on; silence alone starts observation, and what no rule classes goes to Jev, where the host settings name a key command, or else to the operator. Do it yourself when you have checked the worker’s scene and want it to carry on.

## What counts as silent

A stopped worker with no verdict or progress is observed over at least 300 seconds. Actual terminal output, git progress, runtime reports and process activity are evidence, never proof of quota exhaustion by themselves. Structured error codes and a harness's explicit usage-limit and task-refusal messages are classed by rule: Codex's `Usage limit reached … Increase your limits to continue using codex` is an account quota fault, with no observation delay. Codex's `We take extra care with some cybersecurity requests. If you're doing authorized security work, apply for Daybreak to get broader access.` is a refusal of this task by rule, also with no observation delay. This issue continues on the next harness in host kit order that can start for its role. The issue remembers each refusing harness on GitHub and skips it at every later stage and start; the harness stays healthy for other issues. With none left, the issue escalates at once with the refusal line as evidence and no consult. What no rule classes is judged by one Jev request on the worker's latest screen where the host settings name a key command; without one, or where the key command, TypeSafe or Jev's answers settle nothing, the worker waits for the operator and its scene is preserved.

Account quota with no reset time continues on the next harness in host kit order that can start for the worker's role. A reset within the one-hour recovery window may be waited for; a later reset continues instead. Repeated waits on unchanged evidence never restart that window. After one hour the fleet acts on the fault class or escalates the issue to a human with its evidence. Doctor names the worker, its class and its next check, and a worker waiting for the operator with the reason and a line of its screen.

Codex's `Selected model is at capacity. Please try a different model.` is a capacity stop by rule, also with no diagnosis or observation delay. The first stop waits {{CAPACITY_WAIT_SECONDS}} seconds before nudging the same session; the next waits {{CAPACITY_SECOND_WAIT_SECONDS}} seconds. After {{MAX_CAPACITY_STOPS}} stops in the episode, the worker continues on the next eligible harness in host kit order, keeping its worktree, branch, stage, claim and PR. The episode survives brief working turns and runner restarts, ending after {{CAPACITY_HEALTHY_SECONDS}} seconds observed healthy or a different session. Capacity faults no harness for other workers. Tick-log worker-health lines name the rule, waits and nudges; doctor's host kit reports capacity_stops per harness, retained after episodes end.

Confirmed background work, waiting for a human or dependency and completed idle sessions are left alone. A worker's gate waiting in the exclusive queue is background work too. The tick does not turn a long wait into permission for another writer. Continuation closes the old agent and confirms its terminal, runtime and process tree are gone before starting the successor in the same worktree, branch, stage, claim and PR. Structured transient rate limits respect Retry-After and get two retries before session fallback. Account and model recovery probes run hourly, in the login shell a worker gets, and open no terminal; successful recovery affects new starts without moving healthy successors back.

## Who runs it

The operator, in a checkout of the repo, on the host the worker runs on.

## What it reads and writes

It asks orca for the worker's state and reads the issue's comments for a verdict. Immediately before sending, it checks that the terminal still has a harness ready for a prompt: a bare shell receives nothing. It types one line at the worker's terminal and records the nudge in the worktree. Nothing on GitHub or the remote changes.

## What it answers

{"issue", "action": "nudged", "terminal"}

Errors, each with nothing sent: "has no stopped worker" — the instance holds no claim on the issue, or its worker is at work or gone; "is not nudged" — it was nudged already, showed a sign of life within the grace period, waits on background work of its own, or declared a verdict; the claim is another instance's; the worktree has no live terminal.

## What trips you up

- `--instance` is required, and must be the instance that holds the claim.
- It is refused wherever a tick would not nudge: it cannot be used to hurry a worker.
- One nudge per worker. A worker silent again after it is escalated by the next tick.
- To tell a worker something it must know, busy or stopped, use `afkconvoy tell`: a nudge only says carry on."#;

/// What `afkconvoy escalate --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const ESCALATE_HELP: &str = r#"## When to run it

To hand a claim to a human by hand. A tick does this on its own for every failure: an `escalate` verdict, a worker silent past its nudge, a blocker that cannot be parked on. Do it yourself when you can see a claim will not finish and the fleet has not given it up yet.

## Who runs it

The operator, in a checkout of the repo.

## What it reads and writes

On GitHub it comments `--reason` on the issue, swaps the config's `ready_label` for its `escalate_label`, and writes the status board's last word. In a bound repo it swaps the issue's flow label for the `escalate_label` of the team instead, and the comment records which workflow the issue left: `afkconvoy rule` puts that label back. On the remote it releases the claim. The PR and the branch are kept. The worktree is removed when its tree is clean and every commit of its branch is on the remote — pushed, or on the base branch: a later dispatch recreates it at the pushed tip. Otherwise it is kept, and the comment and the status board say what it holds that exists on this host only, the comment listing each commit the remote does not have by its subject. In orca it closes the terminal of the issue's worker: a worktree that is kept has no worker left in it.

## What it answers

{"issue", "action": "escalated", "released": true, "worktree", "kept_because", "unpushed"} — worktree is "removed", "kept", or "none" when the issue has none on this host. A kept one says why: kept_because is "dirty", "unpushed" or "unread", and unpushed is how many commits only it holds (null when unread).

{"issue", "released": true, "detail"} — no claim is held on the issue: nothing was done.

Errors: the claim is another instance's ("is not this fleet's claim").

## What trips you up

- `--instance` is required, and must be the instance that holds the claim.
- It escalates only a claim. On an issue nobody holds it answers `released` with a `detail`, and has neither commented nor relabelled.
- Cut short, run it again with the same reason: it goes on from the step it stopped at, and still posts one comment.
- It stops no worker: one still at work goes on, on an issue the fleet gave up."#;

/// What `afkconvoy close --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const CLOSE_HELP: &str = r#"## When to run it

To close, by hand, a claim whose issue needed no change. A tick does this on its own for a worker that stopped with an `already-satisfied` verdict and no work on its branch. Do it yourself when a plan keeps showing such a claim under `mine` as `no_pr`.

## Who runs it

The operator, in a checkout of the repo, on the host the worker ran on.

## What it reads and writes

It closes the issue on GitHub, writes the status board's last word, releases the claim on the remote, and removes the worktree.

## What it answers

{"issue", "action": "closed", "released": true} {"issue", "released": true, "detail"} — no claim is held on the issue: nothing was done.

Errors: the claim is another instance's ("is not this fleet's claim"); the worktree shows work, or there is no worktree on this host to show there is none ("is not closed as needing no change"). Nothing was changed.

## What trips you up

- `--instance` is required, and must be the instance that holds the claim.
- It does not read the verdict. Any claim of the instance whose branch has no commits past its merge base with the remote base branch, has the same tree as that merge base and has nothing uncommitted is closed — one whose worker has only just started as well. A clean branch only behind the base branch qualifies. A refusal names files the branch changed since the merge base and anything uncommitted."#;

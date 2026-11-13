//! Starting a worker on an issue.

use std::path::PathBuf;

use afkconvoy_core::casting::{self, Cast};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::launch::LaunchCommand;
use afkconvoy_core::line::Line;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::{Switched, WorkerFields, switched, worker_prompt};
use afkconvoy_core::recovery::{Recovery, Tier, Variant, superseded};
use afkconvoy_core::role::ISSUE_ENV;
use afkconvoy_core::selection::Selection;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::worker::{DispatchStep, branch_comment, dispatch_steps, recorded_branches};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::claims::{Me, Take, take};
use crate::fleet::Fleet;
use crate::forget::forget;
use crate::found::Found;
use crate::gh::{IssueRead, Tracker, gone};
use crate::git::Repo;
use crate::launch::Launches;
use crate::orca::{Source, Worktree};
use crate::recovery::{Survey, own_heads, survey};
use crate::remote::{Fresh, Push};
use crate::role::Staged;

/// A dispatch is incomplete until its worker has taken the brief. Keep it
/// under the claim, including across a tick or host runner that ends during
/// startup. A later claim must never inherit an earlier one's failed start.
#[derive(Deserialize, Serialize)]
struct Starting {
    claim: String,
    error: String,
    /// A harness ready once, still waiting to take its brief. This terminal
    /// belongs to this claim alone, and is reused only while it remains live.
    #[serde(default)]
    ready: Option<String>,
    /// Since when a start has waited for the previous worker's terminal to
    /// finish closing (`orca::StillClosing`): that wait is no failed start
    /// until it passes `CLOSE_BOUND`.
    #[serde(default)]
    closing_since: Option<u64>,
}

/// How long, in seconds, a worker start waits for its predecessor's terminal
/// to finish closing before the start counts as failed (sunfmin/control#559).
pub(crate) const CLOSE_BOUND: u64 = 300;

fn starting_file(fleet: &Fleet, number: u64) -> Result<PathBuf> {
    fleet.repo.note(&format!("convoy-dispatch-{number}.json"))
}

fn starting(fleet: &Fleet, number: u64) -> Result<Option<Starting>> {
    let file = starting_file(fleet, number)?;
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", file.display())),
    };
    let kept: Starting = serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot read {}", file.display()))?;
    Ok(fleet.scan.claim_of(number).filter(|claim| claim.id == kept.claim).map(|_| kept))
}

/// The claim's worker has not taken its brief yet, even if its harness is ready.
pub(crate) fn incomplete(fleet: &Fleet, number: u64) -> Result<bool> {
    Ok(starting(fleet, number)?.is_some())
}

fn keep_starting(fleet: &Fleet, number: u64, error: &str) -> Result<()> {
    let Some(claim) = fleet.scan.claim_of(number) else { return Ok(()) };
    let kept = Starting {
        claim: claim.id.clone(),
        error: error.to_string(),
        ready: None,
        closing_since: None,
    };
    std::fs::write(starting_file(fleet, number)?, serde_json::to_vec(&kept)?)?;
    Ok(())
}

fn keep_ready(fleet: &Fleet, number: u64, terminal: &str) -> Result<()> {
    let Some(claim) = fleet.scan.claim_of(number) else { return Ok(()) };
    let kept = Starting {
        claim: claim.id.clone(),
        error: String::new(),
        ready: Some(terminal.to_string()),
        closing_since: None,
    };
    std::fs::write(starting_file(fleet, number)?, serde_json::to_vec(&kept)?)?;
    Ok(())
}

fn forget_starting(fleet: &Fleet, number: u64) -> Result<()> {
    match std::fs::remove_file(starting_file(fleet, number)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// How a worker is started on this machine.
pub struct Agent {
    /// The worker launch command of each harness. In an unbound repo the
    /// default's is the command, with harness setup for a worktree added;
    /// in a bound repo what starts the worker in its role is added to its
    /// harness's (`role::Staged::command`).
    pub launches: Launches,
    /// How long an agent is given to be ready for its prompt, in seconds.
    pub ready_timeout: u64,
}

/// A worker's selected harness and model, as its worker selection chose
/// them (`selection::Selection`), with where the model came from.
pub struct WorkerSelection<'a> {
    pub harness: Harness,
    pub model: Option<&'a str>,
    pub source: Option<&'a str>,
}

impl<'a> From<&'a Selection> for WorkerSelection<'a> {
    fn from(chosen: &'a Selection) -> WorkerSelection<'a> {
        WorkerSelection {
            harness: chosen.harness(),
            model: chosen.model.as_deref(),
            source: chosen.source,
        }
    }
}

impl Agent {
    /// Starts an interactive worker with the fleet's selected model and
    /// freshly read host-user effort, and waits until it can take its brief.
    /// Outside a role it starts already trusting its worktree. Shell-only
    /// auxiliaries use `start`. An issue given here is exported for every
    /// command that worker runs; the train and watches carry no issue context.
    /// `tracker` is the fleet's, which the worker's commands read (`start`).
    #[allow(clippy::too_many_arguments)]
    pub fn start_worker(
        &self,
        worktree: &Worktree,
        tracker: &Tracker,
        selection: WorkerSelection<'_>,
        command: &str,
        in_role: bool,
        issue: Option<u64>,
        now: u64,
    ) -> Result<String> {
        let WorkerSelection { harness, model, source } = selection;
        let prepared = crate::codex_launch::prepare_worker(
            harness, worktree, model, source, command, in_role,
        )?;
        let command = match issue {
            Some(issue) => Line::new()
                .fixed("export ")
                .fixed(ISSUE_ENV)
                .fixed("=")
                .number(issue)
                .fixed(" && ")
                .launch(&prepared.command)
                .done(),
            None => prepared.command,
        };
        let terminal = self.start(worktree, tracker, harness, &command, None, now)?;
        std::fs::write(
            worktree.note("convoy-worker-model")?,
            prepared.model.as_deref().unwrap_or_default(),
        )?;
        if let Some(settings) = &prepared.settings {
            crate::codex_launch::record(worktree, harness, settings)?;
        }
        worktree.await_ready(
            &terminal,
            self.ready_timeout,
            harness,
            &self.launches.worker_line(harness, &command),
        )?;
        Ok(terminal)
    }

    /// Starts every kind of worker with its harness's host network settings
    /// and, when given, its terminal title. The worktree keeps `tracker`, the
    /// fleet's, before the worker starts (`Worktree::keep_tracker`): each
    /// command it runs acts on the issues its fleet does, even where the
    /// checkout list does not hold its checkout yet.
    pub fn start(
        &self,
        worktree: &Worktree,
        tracker: &Tracker,
        harness: Harness,
        command: &str,
        title: Option<&str>,
        now: u64,
    ) -> Result<String> {
        worktree.keep_tracker(tracker)?;
        let terminal = worktree.start(&self.launches.worker_line(harness, command), title, now)?;
        // Record what actually started, never a replacement whose old
        // terminal could not be ended.
        worktree.keep_harness(harness)?;
        let binding = worktree.note("convoy-worker-command-key")?;
        std::fs::write(binding, self.launches.command_key(harness).unwrap_or_default())?;

        Ok(terminal)
    }

    /// The worker launch command a worker of `harness` is started with. An
    /// error is a harness that cannot be started on this host: no other is
    /// ever started in its place.
    pub fn command(&self, harness: Harness) -> Result<LaunchCommand> {
        self.launches.command(harness)
    }

    /// Make a named, installed harness visible to casting before its range is
    /// read. No model is inferred for a newly discovered harness.
    pub fn discover_casting(&self, labels: &[String]) -> Result<()> {
        for harness in
            labels.iter().filter_map(|label| label.strip_prefix("harness:").and_then(Harness::read))
        {
            if !crate::kit::load()?.harnesses.iter().any(|held| held.harness == harness) {
                let _ = self.command(harness);
            }
        }
        Ok(())
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
///   {"started": false, "brief_pending": true, …}
///                                  readiness succeeded but the prompt is still
///                                  settling; a later tick uses that terminal
///   {"started": false, "claim": "lost", "owner": {…}}   a peer holds the issue
///   {"started": false, "skipped": "blocked", "blocked_by": [n…]}
///                                  an open blocker, read just before the claim
///   {"started": false, "gone": "deleted"|"transferred"|"missing", "released": bool}
///                                  GitHub says the issue is gone: a claim held
///                                  on it is let go (`forget`)
///
/// A step that fails is an error with the claim still held: the next tick
/// finds a claim with no worker and continues it — but on an issue that is
/// gone, which no worker is started on again. Nothing is torn down unless
/// `from` is `Fresh`.
pub fn dispatch(
    fleet: &mut Fleet,
    found: &mut Found,
    me: &Me,
    agent: &Agent,
    number: u64,
    from: StartFrom,
) -> Result<Dispatched> {
    dispatch_continuing(fleet, found, me, agent, number, from, None)
}

/// What a dispatch came to: how its start ended, which its callers act on,
/// and the answer `dispatch` gives, which says it again with what was
/// started.
#[derive(Debug)]
pub struct Dispatched {
    pub start: Start,
    pub answer: Value,
}

/// How a dispatch's start ended, when it did not fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// A worker was started under the claim, and took its brief.
    Started,
    /// A worker was started under the claim, its brief still to hand it.
    BriefPending,
    /// A peer won the claim: nothing was started.
    Lost,
    /// The issue had these open blockers just before its claim: nothing was
    /// claimed or started.
    Blocked(Vec<u64>),
    /// GitHub says the issue is gone: nothing was started.
    Gone,
}

impl Dispatched {
    fn blocked(number: u64, blockers: Vec<u64>) -> Dispatched {
        let answer = json!({"issue": number, "started": false, "skipped": "blocked",
            "blocked_by": blockers});
        Dispatched { start: Start::Blocked(blockers), answer }
    }

    fn lost(number: u64, owner: Value) -> Dispatched {
        let answer = json!({"issue": number, "started": false, "claim": "lost", "owner": owner});
        Dispatched { start: Start::Lost, answer }
    }

    fn gone(number: u64, said: &str, released: bool) -> Dispatched {
        let answer = json!({"issue": number, "started": false, "gone": said, "released": released});
        Dispatched { start: Start::Gone, answer }
    }

    /// Whether a worker was started and took its brief.
    pub fn started(&self) -> bool {
        self.start == Start::Started
    }
}

/// `dispatch`, whose brief also says what of the work of the worker before
/// it is there, where `continuing` says that worker was ended mid-way
/// (`switch`).
pub(crate) fn dispatch_continuing(
    fleet: &mut Fleet,
    found: &mut Found,
    me: &Me,
    agent: &Agent,
    number: u64,
    from: StartFrom,
    continuing: Option<&Continuing>,
) -> Result<Dispatched> {
    let previous = starting(fleet, number)?;
    let ready = previous.as_ref().and_then(|previous| previous.ready.as_deref());
    let started = start(fleet, me, agent, number, from, continuing, ready);
    let Some(how) = started.as_ref().err().and_then(gone).cloned() else {
        return match started {
            Ok(started) => {
                if started.started() {
                    forget_starting(fleet, number)?;
                }
                Ok(started)
            }
            Err(error) if error.is::<crate::paused::Held>() || error.is::<Closed>() => Err(error),
            Err(error) if error.is::<crate::orca::StillClosing>() => {
                wait_for_close(fleet, found, me, number, previous, error)
            }
            Err(error) => {
                // Waiting to hand a ready worker its brief is no failed
                // start, even if it subsequently disappears or refuses it.
                // Nor is waiting for the previous worker's terminal to close.
                let previous = previous.filter(|p| p.ready.is_none() && p.closing_since.is_none());
                start_failed(fleet, found, me, number, previous, error)
            }
        };
    };
    let held = fleet.scan.claim_of(number).is_some_and(|claim| claim.is_held_by(&me.instance));
    let released = held && forget(fleet, found, me, number)? == Outcome::Done;
    if held && !released {
        // GitHub has it after all: the failure is as it was.
        return started;
    }
    if !released {
        found.forgotten.push((how.clone(), false));
    }
    let said = how.word();
    forget_starting(fleet, number)?;
    Ok(Dispatched::gone(number, said, released))
}

/// A trust gap hands the claim on at once: restarting the same command would
/// only report the same launch defect. Another failed start keeps the claim
/// for continuation; a second hands it on with both startup errors.
fn start_failed(
    fleet: &mut Fleet,
    found: &mut Found,
    me: &Me,
    number: u64,
    previous: Option<Starting>,
    error: anyhow::Error,
) -> Result<Dispatched> {
    let Some(claim) =
        fleet.scan.claim_of(number).filter(|claim| claim.is_held_by(&me.instance)).cloned()
    else {
        return Err(error);
    };
    let failed = format!("{error:#}");
    keep_starting(fleet, number, &failed)?;
    let trust_gap = error.is::<afkconvoy_core::launch::TrustGap>();
    let why = if trust_gap {
        crate::judge::Handed::fleet_fault(
            afkconvoy_core::transition::FleetFault::WorktreeTrust,
            failed.clone(),
        )
    } else {
        let Some(previous) = previous else { return Err(error) };
        crate::judge::Handed::fleet_fault(
            afkconvoy_core::transition::FleetFault::WorkerStart,
            format!(
                "The worker start failed again before it could take its brief.\n\n\
                 Previous start: {}\n\nFailed start: {failed}",
                previous.error
            ),
        )
    };
    let pr = closing_pr(&fleet.gh.open_prs()?, number).map(|pr| pr.number);
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, number)?;
    crate::judge::escalate_claim(fleet, found, &claim, &why, pr, worktree.as_ref())?;
    forget_starting(fleet, number)?;
    let context = if trust_gap {
        "worker start found a worktree trust gap; the claim was escalated"
    } else {
        "worker start failed again; the claim was escalated"
    };
    Err(error.context(context))
}

/// A start held up only by the previous worker's terminal, still closing
/// after the fleet closed it, waits for that close: it keeps the claim and
/// since when it has waited, and stays `StillClosing`, which fails no tick.
/// A later tick retries it. Still there past `CLOSE_BOUND`, it is a failed
/// start again, and escalates saying how long it was waited for.
fn wait_for_close(
    fleet: &mut Fleet,
    found: &mut Found,
    me: &Me,
    number: u64,
    previous: Option<Starting>,
    error: anyhow::Error,
) -> Result<Dispatched> {
    let Some(claim) =
        fleet.scan.claim_of(number).filter(|claim| claim.is_held_by(&me.instance)).cloned()
    else {
        return Err(error);
    };
    let failed = format!("{error:#}");
    let since = previous.and_then(|previous| previous.closing_since).unwrap_or(fleet.now);
    let waited = fleet.now.saturating_sub(since);
    if waited < CLOSE_BOUND {
        let kept = Starting {
            claim: claim.id.clone(),
            error: failed,
            ready: None,
            closing_since: Some(since),
        };
        std::fs::write(starting_file(fleet, number)?, serde_json::to_vec(&kept)?)?;
        return Err(error);
    }
    let why = crate::judge::Handed::fleet_fault(
        afkconvoy_core::transition::FleetFault::WorkerStart,
        format!(
            "The worker start failed again before it could take its brief.\n\n\
             The previous worker's terminal was still there after close, \
             waited for {waited}s (the bound is {CLOSE_BOUND}s).\n\nFailed start: {failed}"
        ),
    );
    let pr = closing_pr(&fleet.gh.open_prs()?, number).map(|pr| pr.number);
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, number)?;
    crate::judge::escalate_claim(fleet, found, &claim, &why, pr, worktree.as_ref())?;
    forget_starting(fleet, number)?;
    // No longer a wait: this start failed.
    Err(anyhow!(
        "{failed}; waited {waited}s for it to close; worker start failed again; the claim was escalated"
    ))
}

/// A dispatch found its issue closed: nothing was claimed or started. A
/// tick whose frontier still had it, read before it closed, skips it and
/// fails nothing (sunfmin/control#569); any other caller fails on it.
#[derive(Debug)]
pub(crate) struct Closed(pub u64);
impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "issue #{} is closed, not open — nothing to dispatch", self.0)
    }
}
impl std::error::Error for Closed {}

/// What the brief of a worker that continues one ended mid-way says of it.
pub(crate) struct Continuing {
    /// The harness the worker ended ran on.
    pub from: Harness,
    /// The last lines its terminal showed, read before it was closed.
    pub said: Vec<String>,
}

/// Validate the successor before ending the old worker.
pub(crate) fn continuation_harness(fleet: &Fleet, agent: &Agent, number: u64) -> Result<Harness> {
    let issue = fleet.gh.issue(number)?;
    let harness = match fleet
        .config
        .bound
        .as_ref()
        .filter(|_| !afkconvoy_core::plan::is_repair(&issue.labels))
    {
        Some(bound) => Staged::enter(fleet, bound, number, &issue.labels)?.harness(),
        None => fleet.select_in(None, &issue.labels, Some(number))?.harness(),
    };
    crate::role::startable(&agent.launches, harness, "worker continuation")?;
    Ok(harness)
}

/// `dispatch`, but for an issue found gone on the way, which it fails on.
/// Freeze the destination used to prepare the stage, after its preflight.
/// A concurrent selection requires a new preparation, never a stale brief.
fn pin_target(fleet: &Fleet, number: u64, prepared: &str) -> Result<()> {
    let pinned = crate::target::pin(&fleet.repo, &fleet.config, number, None)?;
    if pinned != prepared {
        bail!(
            "issue #{number}'s target became {pinned:?} while dispatch was prepared for {prepared:?}; run dispatch again"
        );
    }
    Ok(())
}

fn start(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    number: u64,
    from: StartFrom,
    continuing: Option<&Continuing>,
    ready: Option<&str>,
) -> Result<Dispatched> {
    let issue = fleet.gh.issue(number)?;
    if issue.is_closed() {
        // before the claim: never lock a closed issue
        return Err(Closed(number).into());
    }
    let target = fleet.task_branch(number)?;
    agent.discover_casting(&issue.labels)?;
    // In a bound repo the worker performs a stage, or the join that follows
    // the last, in its role. Read before the claim: an issue no worker can
    // be started on is never locked.
    let staged = match fleet
        .config
        .bound
        .as_ref()
        .filter(|_| !afkconvoy_core::plan::is_repair(&issue.labels))
    {
        Some(bound) => Some(Staged::enter(fleet, bound, number, &issue.labels)?),
        None => None,
    };
    let held = fleet.scan.claim_of(number).is_some_and(|claim| claim.is_held_by(&me.instance));
    if staged.is_none() && issue.labels.contains(&fleet.config.escalate_label) {
        bail!("issue #{number} is escalated: the host's watch or a human must answer it first");
    }
    // One worker selection says what the worker runs: a followed label's
    // model, or the harness user default, read once (`selection::select`).
    // Paused, refused and unsafe starts are its answers too.
    let chosen = match &staged {
        Some(staged) => staged.selection().clone(),
        None => fleet.select_in(None, &issue.labels, Some(number))?,
    };
    let (harness, model) = (chosen.harness(), chosen.model.clone());
    let unmatched = chosen.cast.unmatched.clone().map(|name| (name, chosen.cast.clone()));
    let launch = agent.command(harness)?;
    if let Some(staged) = staged.as_ref().filter(|_| held) {
        staged.refuse_spent(fleet.scan.claim_of(number))?;
    }
    let by_hand = from == StartFrom::Fresh;
    let answered = if held && staged.is_none() && !by_hand {
        crate::park::cleared_verdict(fleet, number)?
    } else {
        None
    };
    let mut found: Option<Survey> = None;
    let mut plan = Recovery::discarded();
    let mut worktree: Option<Worktree> = None;
    let mut brief = None;
    let mut terminal: Option<String> = None;
    let mut brief_pending = false;
    let mut discarded = None;
    let mut worker_untold = None;
    let undecided = || anyhow!("a dispatch step ran before the one that feeds it");
    let interrupted = "dispatch ended before the worker took its brief";
    if held {
        pin_target(fleet, number, &target)?;
        if !by_hand
            && continuing.is_none()
            && let Some(ready) = ready
        {
            keep_ready(fleet, number, ready)?;
        } else {
            keep_starting(fleet, number, interrupted)?;
        }
    }
    for step in dispatch_steps(held, by_hand) {
        match step {
            DispatchStep::Claim => {
                // A tick's rebuild can be minutes older than this: a blocker
                // added since holds the issue back all the same.
                let edges = fleet.gh.blocked_by(number)?;
                let mut blockers = fleet.gh.blockers(edges.clone());
                if staged.as_ref().is_some_and(Staged::is_consult) {
                    let unworked = crate::blockers::unworked(
                        &fleet.repo,
                        &fleet.gh.tracker,
                        &[(number, edges.clone())],
                    )?;
                    if crate::blockers::can_hand_on(number, &edges, &unworked) {
                        blockers.clear()
                    }
                }
                if !blockers.is_empty() {
                    blockers.sort_unstable();
                    return Ok(Dispatched::blocked(number, blockers));
                }
                pin_target(fleet, number, &target)?;
                let claim = take(&fleet.repo, &mut fleet.scan, me, number, Take::Fresh)?;
                if claim["won"] != true {
                    return Ok(Dispatched::lost(number, claim["owner"].clone()));
                }
                keep_starting(fleet, number, interrupted)?;
                if let Some(staged) = &staged {
                    staged.refuse_spent(fleet.scan.claim_of(number))?;
                }
            }
            DispatchStep::Survey => {
                let surveyed = survey(fleet, number)?;
                // An open PR on an issue no claim held is one a released claim kept.
                plan = if held { surveyed.plan.clone() } else { surveyed.plan.clone().taken_up() };
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
                    Tier::Fresh => cut_at(fleet, &issue, number, &target)?,
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
                if let Some(continuing) = continuing {
                    let pr = closing_pr(&fleet.gh.open_prs()?, number).map(|pr| pr.number);
                    continue_brief(fleet, cut, Some(number), pr, continuing)?;
                }
                if let Some(handoff) = crate::turn_answer::pending(cut)? {
                    let file = brief.as_ref().ok_or_else(undecided)?;
                    let mut text = std::fs::read_to_string(file)?;
                    text.push_str(&format!("\n\n{}\n", handoff.line));
                    cut.write_brief(&text)?;
                }
            }
            DispatchStep::Start => {
                let cut = worktree.as_ref().ok_or_else(undecided)?;
                if !by_hand
                    && continuing.is_none()
                    && let Some(ready) = ready
                    && cut.live(ready)?
                {
                    terminal = Some(ready.to_string());
                    keep_ready(fleet, number, ready)?;
                    continue;
                }
                let launch = launch.as_str();
                if let Some((staged, claim)) = staged.as_ref().zip(fleet.scan.claim_of(number)) {
                    staged.keep_start(cut, claim)?;
                }
                terminal = Some(match &staged {
                    Some(staged) => agent.start_worker(
                        cut,
                        &fleet.gh.tracker,
                        (&chosen).into(),
                        &staged.command(launch, cut, model.as_deref())?,
                        true,
                        None,
                        fleet.now,
                    )?,
                    // Every harness, session or not, takes its model on its
                    // start line as its worker selection says.
                    None => agent.start_worker(
                        cut,
                        &fleet.gh.tracker,
                        (&chosen).into(),
                        &chosen.start_line(launch),
                        false,
                        Some(number),
                        fleet.now,
                    )?,
                });
                worker_untold = note_start(
                    fleet,
                    number,
                    staged.as_ref(),
                    continuing,
                    harness,
                    model.as_deref(),
                );
                keep_ready(fleet, number, terminal.as_deref().ok_or_else(undecided)?)?;
            }
            // `Agent::start_worker` returns only once its worker is ready.
            DispatchStep::AwaitReady => {}
            DispatchStep::Deliver => {
                let (cut, handle) =
                    worktree.as_ref().zip(terminal.as_ref()).ok_or_else(undecided)?;
                crate::issue_updates::starting(cut, issue.body.as_deref().unwrap_or(""))?;
                match cut.deliver(handle, brief.as_ref().ok_or_else(undecided)?) {
                    Ok(()) => {}
                    Err(error) if error.is::<crate::orca::IdleTimeout>() => brief_pending = true,
                    Err(error) => return Err(error),
                }
            }
        }
    }
    let cut = worktree.ok_or_else(undecided)?;
    // Last: a failed start leaves the verdict standing for the next tick.
    // This belongs to dispatch so a continuation by hand answers it too.
    if !brief_pending && let Some(said) = answered {
        cut.record_answered(&said)?;
    }
    let mut started = json!({"issue": number, "started": !brief_pending,
        "claim": if held { "held" } else { "won" },
        "tier": plan.tier.number(), "action": plan.tier.word(),
        "prompt": plan.variant.word(), "reason": plan.reason,
        "worktree": cut.path, "branch": cut.branch, "terminal": terminal, "harness": harness,
        "model": model});
    if brief_pending {
        started["brief_pending"] = json!(true);
    }
    if let Some(staged) = &staged {
        started["prompt"] = json!(staged.prompt());
        started["stage"] = staged.json();
    }
    started["skipped"] = if staged.is_some() {
        started["stage"]["skipped"].clone()
    } else {
        json!(fleet.skipped_before(harness, None, Some(number)))
    };
    if let Some(discarded) = discarded {
        started["discarded"] = discarded;
    }
    if let Some(error) = worker_untold {
        started["worker_untold"] = json!(error);
    }
    if let Some((name, ran)) = &unmatched {
        started["unmatched"] = json!(name);
        // The worker runs: a comment that cannot be written costs only itself.
        if let Err(error) = tell_unmatched(fleet, number, name, ran) {
            started["unmatched_untold"] = json!(format!("{error:#}"));
        }
    }
    let start = if brief_pending { Start::BriefPending } else { Start::Started };
    Ok(Dispatched { start, answer: started })
}

/// Posts the worker-start record when this start is its own row. A landing
/// worker is not one, and neither is a continuation of the same harness. A
/// comment that cannot be written costs only itself: the worker has started.
fn note_start(
    fleet: &Fleet,
    number: u64,
    staged: Option<&Staged>,
    continuing: Option<&Continuing>,
    harness: Harness,
    model: Option<&str>,
) -> Option<String> {
    let landing = staged.is_some_and(|staged| staged.prompt() == "join");
    let from = continuing.map(|continuing| continuing.from);
    if !afkconvoy_core::account::own_row(landing, from, harness) {
        return None;
    }
    let claim = fleet.scan.claim_of(number)?;
    // A start already on record for this claim and harness is this worker.
    // Restarting it after it died does not open another row.
    let comments = match fleet.gh.comments(number) {
        Ok(comments) => comments,
        Err(error) => return Some(format!("{error:#}")),
    };
    if afkconvoy_core::account::already_noted(
        comments.iter().map(|comment| comment.body.as_str()),
        &claim.id,
        harness,
    ) {
        return None;
    }
    let body = afkconvoy_core::account::worker_comment(&afkconvoy_core::account::Begun {
        at: fleet.now,
        claim: &claim.id,
        since: claim.since,
        harness: harness.key(),
        model,
        stage: staged.map(Staged::stage),
        role: staged.map(Staged::role),
        consult: staged.is_some_and(Staged::is_consult),
    });
    fleet.gh.comment(number, &body).err().map(|error| format!("{error:#}"))
}

/// Tells issue `number` once that `name`, a model label it carries, is not
/// followed, and that the worker runs the user's default (`casting::unmatched_told`).
fn tell_unmatched(fleet: &Fleet, number: u64, name: &str, ran: &Cast) -> Result<()> {
    let comments = fleet.gh.comments(number)?;
    if casting::unmatched_told(comments.iter().map(|c| c.body.as_str()), name) {
        return Ok(());
    }
    let notice = ran.notice.as_deref().unwrap_or("");
    fleet.gh.comment(number, &casting::unmatched_comment(name, ran, notice))
}

/// Adds to the brief in `worktree` what of the work of the worker before is
/// there (`prompt::switched`): its commits ahead of the base branch, its open
/// `pr`, and what that worker last said.
pub(crate) fn continue_brief(
    fleet: &Fleet,
    worktree: &Worktree,
    number: Option<u64>,
    pr: Option<u64>,
    continuing: &Continuing,
) -> Result<()> {
    let branch = number
        .map(|n| fleet.task_branch(n))
        .transpose()?
        .unwrap_or_else(|| fleet.config.base_branch.clone());
    let base = fleet
        .repo
        .remote()
        .fetched(&format!("refs/heads/{branch}"), Fresh::Process)?
        .ok_or_else(|| anyhow!("target branch {branch:?} disappeared"))?;
    let log = Repo::new(worktree.path.clone()).git(&[
        "log",
        "--oneline",
        "--no-decorate",
        "-n",
        "20",
        &format!("{base}..HEAD"),
    ])?;
    let commits: Vec<String> = log.lines().map(str::to_string).collect();
    let added = switched(&Switched {
        from: continuing.from.key(),
        base_branch: &branch,
        commits: &commits,
        pr,
        said: &continuing.said,
    });
    let brief = worktree.brief()?;
    let mut text = std::fs::read_to_string(&brief)
        .with_context(|| format!("cannot read the worker's brief {}", brief.display()))?;
    text.push_str(&added);
    text.push_str(&format!(
        "\nUncommitted work to preserve:\n```\n{}\n```\n",
        Repo::new(worktree.path.clone()).git(&["status", "--short"])?
    ));
    if let Ok(evidence) = std::fs::read_to_string(worktree.note("convoy-recovery-evidence.json")?) {
        text.push_str(&format!("\n## Fault evidence and pending work\n{evidence}\nContinue from this scene; preserve existing work and complete the original task.\n"));
    }
    worktree.write_brief(&text)?;
    Ok(())
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
    brief_with(fleet, worktree, (number, title), |fields| worker_prompt(fields, variant))
}

/// Writes into `worktree` the brief `prompt` makes of the fields of issue
/// `number`, titled `title` → the file.
fn brief_with(
    fleet: &Fleet,
    worktree: &Worktree,
    (number, title): (u64, &str),
    prompt: impl FnOnce(&WorkerFields) -> Result<String, String>,
) -> Result<PathBuf> {
    let path = worktree.path.display().to_string();
    let fast_gate =
        fleet.config.gates.as_ref().map(|_| crate::own_command().fixed(" gate --fast").own());
    let fields = WorkerFields {
        number,
        tracker: &fleet.gh.tracker,
        title,
        branch: &worktree.branch,
        worktree: &path,
        base_branch: &fleet.task_branch(number)?,
        gate_command: &crate::own_command().fixed(" gate --land").own(),
        gate_alone_command: &crate::own_command().fixed(" gate").own(),
        lock_command: &crate::own_command().fixed(" lock").own(),
        ready_label: &fleet.config.ready_label,
        land_command: &crate::own_command().fixed(" land --issue ").number(number).own(),
        wake_command: &crate::own_command().fixed(" wake").own(),
        fast_gate_command: fast_gate.as_deref(),
    };
    let mut text = prompt(&fields).map_err(|e| anyhow!(e))?;
    text.push_str(&crate::delivery::brief(&fleet.repo, number)?);
    worktree.write_brief(&text)
}

/// Has orca cut a worktree for issue `number` at the tip the remote has for
/// `branch`, named for the fleet's tracker (`Tracker::worktree_name`).
fn cut_at(fleet: &Fleet, issue: &IssueRead, number: u64, branch: &str) -> Result<Worktree> {
    let sha = fleet
        .repo
        .remote()
        .fetched(&format!("refs/heads/{branch}"), Fresh::Process)?
        .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
    let source = Source::of(&fleet.repo)?;
    let name = fleet.gh.tracker.worktree_name(number, &issue.title);
    Worktree::cut(&source, &name, &sha, Some(number))
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
        fleet.remote.push(&Push::delete(&format!("refs/heads/{branch}")))?;
    }
    if let Some(worktree) = &worktree {
        worktree.remove()?;
    }
    Ok(json!({"closed_prs": closed, "deleted_branches": own,
        "removed_worktree": worktree.map(|worktree| worktree.path)}))
}

/// What `afkconvoy dispatch --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

To start a worker on one issue by hand. A tick does this on its own for every frontier issue it has a free slot for, and for every claim of its own left without a worker. Do it yourself when a plan shows an issue the fleet is not starting, or — with `--start fresh` — to throw away an attempt that went wrong and have the issue worked again. Run `recovery` first: it shows what this would continue from.

## Who runs it

The operator, in a checkout of the repo, on the host the worker is to run on.

## What it reads and writes

Before it claims an issue the instance does not hold, it reads the issue's blockers on GitHub once more: with any of them open nothing is claimed and no worker starts. A tick's rebuild can be minutes older than its dispatches, and a blocker added in between holds the issue back all the same.

It claims the issue for `--instance` unless that instance holds it already, looks at what survives of an earlier attempt, has orca put a worktree there, records the worktree's branch in a comment on the issue, writes the worker's brief into the worktree, starts the worker launch command in it — under `nice`, with the worktree's own `TMPDIR` — and hands the worker its prompt.

In a bound repo the issue's flow label names its workflow, and the worker performs the stage the issue's stage records put it at — the workflow's first while there are none — in the stage's role, with the verdicts declared so far, and the reason of a rejection, in its brief. The team's repo and each role's are asked for the commit at their head; one the fleet has no copy of yet is cloned and kept, read-only, under the cache directory (copies/<owner>/<repo>/<commit>; `afkconvoy help cache` says where that directory is and what moves it), once however many roles it holds. A role that is a subdirectory of its repo is copied out of that on its own (<commit>-role-<path>; <commit>-role+codex-<path>, with a link to the role's skills where Codex finds them, for a Codex worker), and so is the team's directory, without the directories of its roles and without a `.claude/` at its top (<commit>-team+rules-<path>): a worker is given its role's directory and, of the team's, the team rules (its `CLAUDE.md`) — nothing else above its role, and no neighbour's.

The terminal is opened on the worktree as ever, and the line it runs changes to the role's copy, puts the stage's facts in the environment, and starts the worker launch command with what puts the worker in its role: [`afkconvoy help worker-launch`](help:worker-launch) says what, for each harness. The worker's harness is the first eligible one in this host's kit order within its role's range. Its model is the issue's one model label of that harness, or that harness's user default (`afkconvoy help roster`).

Where the stage declared `escalate` and names a consult, the worker is started as that consult instead: in the consult's role, on a brief that names the skill, the stage that asked and its reason, and the two lines it ends with — `afkconvoy rule`, or `afkconvoy verdict escalate` to let it through to a human. AFKCONVOY_CONSULT is `true` in its environment, the guard refuses its commits, and it runs its own role's range, not the stage's.

Once the issue's last stage has passed to `land`, the worker is started in the workflow's landing role instead, on the landing brief with that role's landing skill named in it: it gates the issue's open PR and lands in a turn. Its stage is `land`; when it cannot proceed it may declare `blocked` or `escalate` (`afkconvoy verdict --help`).

## What it answers

{"issue", "started": true, "claim", "tier", "action", "prompt", "reason", "worktree", "branch", "terminal"}

- `claim`: won (claimed now) or held (the instance's already)
- `tier`, `action`: 1 reuse_worktree — the worktree is still on this host; 2 recreate_at_tip — a new worktree at the issue's pushed branch; 3 dispatch_fresh — a new worktree at the base branch
- `prompt`: the brief the worker got: fresh, continue, land (its PR is open: only the landing is left), or take_up (its PR is open and no claim held it: an escalation or a park kept it — take its landing turn) — in a bound repo, stage, consult, or land for a landing worker
- `stage`: in a bound repo: {flow, name, role, skill, writes, consult, harness, model, role_commit, role_dir, team_commit, team_dir} — the stage the worker performs, and the copies and commits fixed for it; name is `land` for a landing worker. For a consult (`consult: true`), name is the stage that asked, and role and skill are the consult's
- `reason`: why that tier, in words
- `discarded`: with `--start fresh`: {closed_prs, deleted_branches, removed_worktree}

{"issue", "started": false, "claim": "lost", "owner"} — another instance holds the claim. Nothing was started.

{"issue", "started": false, "skipped": "blocked", "blocked_by"} — the issue has an open blocker: blocked_by lists them, lowest number first. Nothing was claimed and nothing was started.

Errors: the issue is closed; in a bound repo, the issue carries no flow label, more than one, or one that names no workflow of the team, it is at no stage a worker performs (the error says where it is), it passed to `land` and no open PR closes it, the team does not load (the error carries its `faults`) — none of which claims the issue; the instance's claim on it is one a stage already declared under; the worker launch command resolves to nothing; orca fails, or the worker is not ready in time. After a failed start the claim is still held: until the worker takes its brief, the instance's next tick finds a claim with no worker and starts it again in the same worktree. A second failed start escalates and releases the claim, naming the start errors. A start held up only by the previous worker's terminal, still there after orca acknowledged closing it, is no failed start: it fails no tick, keeps the claim, and is tried again each tick until the terminal is gone; still there 300 seconds after the first such start, it escalates as a failed start, saying how long it was waited for. No nudge or silence judgment is made of an incomplete start.

## What trips you up

- `--instance` is required, and it is the instance that holds the claim afterwards. Give it the id of the fleet that is to look after the worker.
- `--start fresh` discards the previous attempt: it closes the issue's open PRs, deletes its branches on the remote and removes its worktree, unpushed work included.
- It checks that the issue is open and, unless the instance holds it already, that no blocker of it is open — nothing else of the frontier: an issue without the ready label, or one admission would hold back, is started all the same. A claim the instance holds is continued whatever blocks it.
- A role in a subdirectory is not the directory a human opens in a checkout: a `CLAUDE.md` or a `.claude/` above it there is not in the worker's copy, nor above it — but for the `CLAUDE.md` of the team's own directory, the team rules, which every worker is given. `afkconvoy check` notes each role that has another above it.
- It does not ask after a worker already there. On a claim whose worker is at work it starts another in the same worktree."#;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_that_did_not_start_says_how_typed_and_answers_as_before() {
        let blocked = Dispatched::blocked(3, vec![5, 9]);
        assert_eq!(blocked.start, Start::Blocked(vec![5, 9]));
        assert_eq!(
            blocked.answer,
            json!({"issue": 3, "started": false, "skipped": "blocked", "blocked_by": [5, 9]})
        );
        let lost = Dispatched::lost(3, json!("beta"));
        assert_eq!(lost.start, Start::Lost);
        assert_eq!(
            lost.answer,
            json!({"issue": 3, "started": false, "claim": "lost", "owner": "beta"})
        );
        let gone = Dispatched::gone(3, "deleted", true);
        assert_eq!(gone.start, Start::Gone);
        assert_eq!(
            gone.answer,
            json!({"issue": 3, "started": false, "gone": "deleted", "released": true})
        );
        assert!(![blocked, lost, gone].iter().any(Dispatched::started));
        let pending = Dispatched { start: Start::BriefPending, answer: json!({}) };
        assert!(!pending.started());
        assert!(Dispatched { start: Start::Started, answer: json!({}) }.started());
    }
}

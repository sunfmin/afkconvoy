//! Starting a worker on an issue.

use std::path::PathBuf;

use afkconvoy_core::casting::{self, Cast};
use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::launch::LaunchCommand;
use afkconvoy_core::line::Line;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::{
    StockFields, Switched, WorkerFields, stock_prompt, switched, worker_prompt,
};
use afkconvoy_core::recovery::{Recovery, Tier, Variant, superseded};
use afkconvoy_core::role::ISSUE_ENV;
use afkconvoy_core::stock;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition::escalated_at;
use afkconvoy_core::worker::{DispatchStep, branch_comment, dispatch_steps, recorded_branches};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::claims::{Me, Take, take};
use crate::fleet::Fleet;
use crate::forget::forget;
use crate::gh::{IssueRead, gone};
use crate::git::{REMOTE, Repo};
use crate::launch::Launches;
use crate::orca::{Source, Worktree};
use crate::recovery::{Survey, own_heads, survey};
use crate::role::Staged;

/// A dispatch is incomplete until its worker has taken the brief. Keep it
/// under the claim, including across a tick or host runner that ends during
/// startup. A later claim must never inherit an earlier one's failed start.
#[derive(Deserialize, Serialize)]
struct Starting {
    claim: String,
    error: String,
}

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

/// The claim has no worker yet, even if orca still holds its terminal.
pub(crate) fn incomplete(fleet: &Fleet, number: u64) -> Result<bool> {
    Ok(starting(fleet, number)?.is_some())
}

fn keep_starting(fleet: &Fleet, number: u64, error: &str) -> Result<()> {
    let Some(claim) = fleet.scan.claim_of(number) else { return Ok(()) };
    let kept = Starting { claim: claim.id.clone(), error: error.to_string() };
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

impl Agent {
    /// Starts an interactive worker with the fleet's selected model and
    /// freshly read host-user effort. Shell-only auxiliaries use `start`.
    /// An issue given here is exported for every command that worker runs;
    /// the train and inspections carry no issue context.
    pub fn start_worker(
        &self,
        worktree: &Worktree,
        selection: (Harness, Option<&str>),
        command: &str,
        in_role: bool,
        issue: Option<u64>,
        now: u64,
    ) -> Result<String> {
        let (harness, model) = selection;
        let prepared = if harness == Harness::Codex {
            Some(crate::codex_launch::prepare(worktree, model, command, in_role)?)
        } else {
            None
        };
        let command = prepared.as_ref().map_or(command, |(line, _)| line.as_str());
        let command = match issue {
            Some(issue) => Line::new()
                .fixed("export ")
                .fixed(ISSUE_ENV)
                .fixed("=")
                .number(issue)
                .fixed(" && ")
                .launch(command)
                .done(),
            None => command.to_string(),
        };
        let terminal = self.start(worktree, harness, &command, now)?;
        std::fs::write(worktree.note("convoy-worker-model")?, model.unwrap_or_default())?;
        if let Some((_, settings)) = prepared {
            crate::codex_launch::record(worktree, &settings)?;
        }
        Ok(terminal)
    }

    /// Starts every kind of worker with its harness's host network settings.
    pub fn start(
        &self,
        worktree: &Worktree,
        harness: Harness,
        command: &str,
        now: u64,
    ) -> Result<String> {
        let terminal = worktree.start(&self.launches.worker_line(harness, command), now)?;
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
///   {"started": false, "claim": "lost", "owner": {…}}   a peer holds the issue
///   {"started": false, "skipped": "blocked", "blocked_by": [n…]}
///                                  an open blocker, read just before the claim
///   {"started": false, "gone": "deleted"|"transferred", "released": bool}
///                                  GitHub says the issue is gone: a claim held
///                                  on it is let go (`forget`)
///
/// A step that fails is an error with the claim still held: the next tick
/// finds a claim with no worker and continues it — but on an issue that is
/// gone, which no worker is started on again. Nothing is torn down unless
/// `from` is `Fresh`.
pub fn dispatch(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    number: u64,
    from: StartFrom,
) -> Result<Value> {
    dispatch_continuing(fleet, me, agent, number, from, None)
}

/// `dispatch`, whose brief also says what of the work of the worker before
/// it is there, where `continuing` says that worker was ended mid-way
/// (`switch`).
pub(crate) fn dispatch_continuing(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    number: u64,
    from: StartFrom,
    continuing: Option<&Continuing>,
) -> Result<Value> {
    let previous = starting(fleet, number)?;
    let started = start(fleet, me, agent, number, from, continuing);
    let Some(how) = started.as_ref().err().and_then(gone).cloned() else {
        return match started {
            Ok(started) => {
                if started["started"] == true {
                    forget_starting(fleet, number)?;
                }
                Ok(started)
            }
            Err(error) if error.is::<crate::paused::Held>() => Err(error),
            Err(error) => start_failed(fleet, me, number, previous, error),
        };
    };
    let held = fleet.scan.claim_of(number).is_some_and(|claim| claim.is_held_by(&me.instance));
    let released = held && forget(fleet, me, number)? == Outcome::Done;
    if held && !released {
        // GitHub has it after all: the failure is as it was.
        return started;
    }
    if !released {
        fleet.forgotten.push((how.clone(), false));
    }
    let said = if how.deleted { "deleted" } else { "transferred" };
    forget_starting(fleet, number)?;
    Ok(json!({"issue": number, "started": false, "gone": said, "released": released}))
}

/// One failed start keeps the claim for continuation; another hands it on
/// with the startup errors, never with a judgment of worker silence.
fn start_failed(
    fleet: &mut Fleet,
    me: &Me,
    number: u64,
    previous: Option<Starting>,
    error: anyhow::Error,
) -> Result<Value> {
    let Some(claim) =
        fleet.scan.claim_of(number).filter(|claim| claim.is_held_by(&me.instance)).cloned()
    else {
        return Err(error);
    };
    let failed = format!("{error:#}");
    keep_starting(fleet, number, &failed)?;
    let Some(previous) = previous else { return Err(error) };
    let why = crate::judge::Handed::new(
        "its worker start failed again",
        format!(
            "The worker start failed again before it could take its brief.\n\n\
             Previous start: {}\n\nFailed start: {failed}",
            previous.error
        ),
    );
    let pr = closing_pr(&fleet.gh.open_prs()?, number).map(|pr| pr.number);
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, number)?;
    crate::judge::escalate_claim(fleet, &claim, &why, pr, worktree.as_ref())?;
    forget_starting(fleet, number)?;
    Err(error.context("worker start failed again; the claim was escalated"))
}

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
    let (harness, model) = match &fleet.config.bound {
        Some(bound) => {
            let staged = Staged::enter(fleet, bound, number, &issue.labels)?;
            (staged.harness(), staged.model().map(str::to_string))
        }
        None if issue.labels.contains(&fleet.config.escalate_label) => {
            let cast = fleet.stock_consult()?;
            (cast.harness, cast.model)
        }
        None => {
            let cast = fleet.cast(&issue.labels)?.map_err(|e| anyhow!(e))?;
            (cast.harness, cast.model)
        }
    };
    crate::codex_launch::selected(harness, model.as_deref())?;
    crate::role::startable(&agent.launches, harness, "worker continuation")?;
    Ok(harness)
}

/// `dispatch`, but for an issue found gone on the way, which it fails on.
fn start(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    number: u64,
    from: StartFrom,
    continuing: Option<&Continuing>,
) -> Result<Value> {
    let issue = fleet.gh.issue(number)?;
    if issue.is_closed() {
        // before the claim: never lock a closed issue
        bail!("issue #{number} is closed, not open — nothing to dispatch");
    }
    agent.discover_casting(&issue.labels)?;
    // In a bound repo the worker performs a stage, or the join that follows
    // the last, in its role. Read before the claim: an issue no worker can
    // be started on is never locked.
    let staged = match &fleet.config.bound {
        Some(bound) => Some(Staged::enter(fleet, bound, number, &issue.labels)?),
        None => None,
    };
    let held = fleet.scan.claim_of(number).is_some_and(|claim| claim.is_held_by(&me.instance));
    // In an unbound repo, an escalated issue is the stock consult's.
    let consult = match staged {
        None if issue.labels.contains(&fleet.config.escalate_label) => {
            stock_consult(fleet, number, held)?
        }
        _ => None,
    };
    // Before the claim too: an issue whose harness cannot be started here is
    // never locked. An unbound repo's stock consult runs what the host kit
    // names for it, and its other workers their issue's casting: one the
    // host kit does not hold runs its first, and the issue is told once
    // (`casting::unmatched_comment`); a claim already held on a paused
    // harness — its landing — runs the host kit's first.
    let (harness, model, unmatched) = match (&staged, &consult) {
        (Some(staged), _) => {
            (staged.harness(), staged.model().map(str::to_string), staged.unmatched().cloned())
        }
        (None, Some(_)) => {
            let stock = fleet.stock_consult()?;
            (stock.harness, stock.model.clone(), stock.unmatched.clone().map(|name| (name, stock)))
        }
        (None, None) => {
            let cast = match fleet.cast(&issue.labels) {
                Ok(Ok(cast)) => cast,
                Err(error) if held && error.is::<crate::paused::Held>() => fleet.first()?,
                Ok(Err(why)) => bail!("{why}: run `afkconvoy kit`"),
                Err(error) => return Err(error),
            };
            let unmatched = cast.unmatched.clone().map(|name| (name, cast.clone()));
            (cast.harness, cast.model, unmatched)
        }
    };
    // No worker is started on a harness paused on this host.
    if let Some(pause) = crate::paused::Pauses::of_host()?.of(harness)? {
        return Err(crate::paused::Held::of(&pause, fleet.now).into());
    }
    let launch = agent.command(harness)?;
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
    let interrupted = "dispatch ended before the worker took its brief";
    if held {
        keep_starting(fleet, number, interrupted)?;
    }
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
                keep_starting(fleet, number, interrupted)?;
                if let Some(staged) = &staged {
                    staged.refuse_spent(fleet.scan.claim_of(number))?;
                }
                // Its claim is the stock consult's once this says so.
                if let Some(claim) = fleet.scan.claim_of(number).filter(|_| consult.is_some()) {
                    fleet.gh.comment(number, &stock::asked_comment(&claim.id))?;
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
                brief = Some(match (&staged, &consult) {
                    (Some(staged), _) => staged.write_brief(fleet, cut, &issue.title)?,
                    (None, Some(asked)) => {
                        write_stock_brief(fleet, cut, number, &issue.title, asked)?
                    }
                    (None, None) => write_brief(fleet, cut, number, &issue.title, plan.variant)?,
                });
                if let Some(continuing) = continuing {
                    let pr = closing_pr(&fleet.gh.open_prs()?, number).map(|pr| pr.number);
                    continue_brief(fleet, cut, pr, continuing)?;
                }
            }
            DispatchStep::Start => {
                let cut = worktree.as_ref().ok_or_else(undecided)?;
                let launch = launch.as_str();
                if let Some((staged, claim)) = staged.as_ref().zip(fleet.scan.claim_of(number)) {
                    staged.keep_start(cut, claim)?;
                }
                // The stock consult writes no code: where it started is kept
                // to check, as a stage's consult's is.
                if let Some(claim) = fleet.scan.claim_of(number).filter(|_| consult.is_some()) {
                    cut.keep_writes_nothing(|tip| WritesNothing {
                        claim: claim.id.clone(),
                        tip,
                        stage: stock::ROLE.to_string(),
                        consult: true,
                    })?;
                }
                terminal = Some(match &staged {
                    Some(staged) => agent.start_worker(
                        cut,
                        (harness, model.as_deref()),
                        &staged.command(launch, cut)?,
                        true,
                        None,
                        fleet.now,
                    )?,
                    None => {
                        let launch = match &model {
                            Some(model) => harness.with_model(launch, model),
                            None => launch.to_string(),
                        };
                        agent.start_worker(
                            cut,
                            (harness, model.as_deref()),
                            &launch,
                            false,
                            Some(number),
                            fleet.now,
                        )?
                    }
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
        "worktree": cut.path, "branch": cut.branch, "terminal": terminal, "harness": harness,
        "model": model});
    if let Some(staged) = &staged {
        started["prompt"] = json!(staged.prompt());
        started["stage"] = staged.json();
    }
    if consult.is_some() {
        started["prompt"] = json!(stock::ROLE);
    }
    if let Some(discarded) = discarded {
        started["discarded"] = discarded;
    }
    if let Some((name, ran)) = &unmatched {
        started["unmatched"] = json!(name);
        // The worker runs: a comment that cannot be written costs only itself.
        if let Err(error) = tell_unmatched(fleet, number, name, ran, &issue.labels) {
            started["unmatched_untold"] = json!(format!("{error:#}"));
        }
    }
    Ok(started)
}

/// Tells issue `number` that `name`, a name it was given, matches nothing
/// held, and that the worker runs `ran` instead — once: not where a comment
/// already says it (`casting::unmatched_told`).
fn tell_unmatched(
    fleet: &Fleet,
    number: u64,
    name: &str,
    ran: &Cast,
    labels: &[String],
) -> Result<()> {
    let comments = fleet.gh.comments(number)?;
    if casting::unmatched_told(comments.iter().map(|c| c.body.as_str()), name) {
        return Ok(());
    }
    let kit = crate::kit::running()?.0.range();
    let mut holds = casting::holds(&kit);
    if let Some(unheld) = casting::unheld(labels, &kit) {
        holds.push_str(&format!(". {}", unheld.why));
    }
    fleet.gh.comment(number, &casting::unmatched_comment(name, ran, &holds))
}

/// Adds to the brief in `worktree` what of the work of the worker before is
/// there (`prompt::switched`): its commits ahead of the base branch, its open
/// `pr`, and what that worker last said.
pub(crate) fn continue_brief(
    fleet: &Fleet,
    worktree: &Worktree,
    pr: Option<u64>,
    continuing: &Continuing,
) -> Result<()> {
    let base = fleet.base_tip()?;
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
        base_branch: &fleet.config.base_branch,
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

/// What the stock consult of an unbound repo's escalated issue `number` is
/// asked — its latest escalation, but for its marker — when it is the
/// consult's: its consult due, or `held` under a claim it was asked under.
fn stock_consult(fleet: &Fleet, number: u64, held: bool) -> Result<Option<String>> {
    if !fleet.config.stock_consult() {
        return Ok(None);
    }
    let comments = fleet.gh.comments(number)?;
    let bodies = || comments.iter().map(|comment| comment.body.as_str());
    let asked = match fleet.scan.claim_of(number).filter(|_| held) {
        Some(claim) => stock::asked_under(bodies(), &claim.id),
        None => stock::due(bodies()),
    };
    let Some(at) = escalated_at(bodies()).filter(|_| asked) else { return Ok(None) };
    let said = comments.get(at).map_or("", |comment| comment.body.as_str());
    Ok(Some(said.split_once("-->").map_or(said, |(_, said)| said).trim().to_string()))
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

/// Writes into `worktree` the brief of the stock consult of issue `number`,
/// asked of the escalation that said `asked` → the file.
fn write_stock_brief(
    fleet: &Fleet,
    worktree: &Worktree,
    number: u64,
    title: &str,
    asked: &str,
) -> Result<PathBuf> {
    let stock = StockFields {
        asked,
        escalate_label: &fleet.config.escalate_label,
        doctor: &crate::own_command().fixed(" doctor").own(),
        plan: &crate::own_command().fixed(" plan").own(),
    };
    brief_with(fleet, worktree, (number, title), |fields| stock_prompt(fields, &stock))
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
    let fields = WorkerFields {
        number,
        tracker: &fleet.gh.tracker,
        title,
        branch: &worktree.branch,
        worktree: &path,
        base_branch: &fleet.config.base_branch,
        gate_command: &crate::own_command().fixed(" gate --train").own(),
        gate_alone_command: &crate::own_command().fixed(" gate").own(),
        lock_command: &crate::own_command().fixed(" lock").own(),
        ready_label: &fleet.config.ready_label,
        land_command: &crate::own_command().fixed(" land --issue ").number(number).own(),
        wake_command: &crate::own_command().fixed(" wake").own(),
    };
    worktree.write_brief(&prompt(&fields).map_err(|e| anyhow!(e))?)
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
/// `branch`, named for the fleet's tracker (`Tracker::worktree_name`).
fn cut_at(fleet: &Fleet, issue: &IssueRead, number: u64, branch: &str) -> Result<Worktree> {
    let sha = fleet
        .repo
        .fetched(&format!("refs/heads/{branch}"))?
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
        fleet.repo.git(&["push", "--quiet", REMOTE, &format!(":refs/heads/{branch}")])?;
    }
    if let Some(worktree) = &worktree {
        worktree.remove()?;
    }
    Ok(json!({"closed_prs": closed, "deleted_branches": own,
        "removed_worktree": worktree.map(|worktree| worktree.path)}))
}

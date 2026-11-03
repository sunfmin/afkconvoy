//! The tick's effect half: carries out the steps the core's pass hands it, and
//! the loop that runs one tick after another.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use afkconvoy_core::before::{Before, Lost};
use afkconvoy_core::lease::stamp_age;
use afkconvoy_core::lock::named;
use afkconvoy_core::plan::{Status, WorkingSet, handed_by};
use afkconvoy_core::position::Position;
use afkconvoy_core::takeover::run_said;
use afkconvoy_core::tick::{
    Outcome, Pass, TickStep, empty_streak, idle, pace, run_pass, sleeps_skipped,
};
use afkconvoy_core::wake::turn;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::admission::Admission;
use crate::advance::{advance, hand_to_human, run_again_unclaimed};
use crate::board::show;
use crate::claims::{Me, beat, goodbye};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::fleet_report::FleetReport;
use crate::forget::forget;
use crate::gh::{Gone, unreached};
use crate::git::Repo;
use crate::host_runner::Shared;
use crate::hot::{self, Hot};
use crate::judge::judge;
use crate::log::{self, Cause, Log};
use crate::panel::{self, Panel};
use crate::patrol;
use crate::settle::settle;
use crate::slow_gate;
use crate::stage::Flows;
use crate::standings::Standing;
use crate::stop::Stop;
use crate::takeover::{at_start, reclaim, unlock};
use crate::topic::{self, See};
use crate::wake::{During, Wakes};
use crate::watch::watch;

/// What failed: a step, or the rebuild's read of one claimed issue.
#[derive(Clone, Copy)]
enum Failed {
    Step(TickStep),
    Read(u64),
    StopTaken(u64),
    Delivery,
}

/// One step that failed, and why.
struct Failure {
    step: Failed,
    error: String,
    /// The topic that explains it, where one does.
    see: Option<See>,
    /// Whether it failed on a read GitHub could not be reached for.
    unreached: bool,
}

/// Since when a worker has waited on its own background work, as a tick's line
/// says it: `since 2026-10-20 23:03 UTC, 12m ago`.
pub(crate) fn waiting_since(since: u64, now: u64) -> String {
    let ago = match stamp_age(since, now) {
        Some(ago) if ago >= 60 => format!("{}m ago", ago / 60),
        Some(ago) => format!("{ago}s ago"),
        None => "just now".to_string(),
    };
    // To the minute: the seconds are left off.
    let stamped = log::utc(since);
    format!("since {} UTC, {ago}", &stamped[..stamped.len() - 3])
}

/// An issue as the line names it among those asked a turn again: with the
/// fleet locks it stopped at, closed since, where it had.
fn after_lock_said(issue: u64, locks: Option<&[u64]>) -> String {
    match locks {
        Some([]) => format!("#{issue} (the fleet lock it stopped at closed)"),
        Some([lock]) => format!("#{issue} (fleet lock #{lock} closed)"),
        Some(locks) => format!("#{issue} (fleet locks {} closed)", named(locks)),
        None => format!("#{issue}"),
    }
}

/// What one tick came to.
pub struct Ticked {
    pub pass: Pass,
    updates: Vec<Value>,
    failures: Vec<Failure>,
    /// The issues the pass escalated, each with why in a few words
    /// (`Fleet::escalated`): whichever step did.
    escalated: Vec<(u64, String)>,
    /// The issues a dispatch skipped, each with the open blockers it read
    /// just before the claim.
    blocked: Vec<(u64, Vec<u64>)>,
    /// The issues the pass found gone, each with whether it released a claim
    /// on it (`Fleet::forgotten`).
    forgotten: Vec<(Gone, bool)>,
    /// What the tick before saw, when this `run` made one: what the line
    /// names an outside change against.
    before: Option<Before>,
    /// The claims another fleet instance took since the tick before.
    lost: Vec<Lost>,
    /// Local workers ended because another host holds their issue's claim.
    stopped_taken: Vec<u64>,
    /// The closed claimed issues, each with who closed it (`Fleet::closers`).
    closers: Vec<(u64, String)>,
    /// The landings asked a turn again once the locks they stopped at closed
    /// (`Fleet::after_locks`).
    after_locks: Vec<(u64, Vec<u64>)>,
    /// What the pass's start and end of a cut said (`Fleet::cut_said`).
    cut_said: Vec<log::What>,
    /// The open issues, each with the label that hands it to the fleet
    /// (`plan::handed_by`).
    handed: Vec<(u64, String)>,
    /// What this tick saw as it ended, but for its count: the next one's
    /// `before`.
    saw: Before,
    /// Wake routes from the open issues this tick read, including those
    /// still held back by another blocker or by their labels.
    blockers: Vec<u64>,
    /// The concurrency in force (`concurrency::config_of`).
    concurrency: usize,
    /// The claims the pass could hold as it ended (`Pass::bound`).
    bound: usize,
    /// What its rebuild read: the pass was made over these.
    set: WorkingSet,
    flows: Flows,
    /// What admission read of the host, and the shares it held it to.
    admission: Admission,
    /// The clock the tick ran at: what a worker's wait is told against.
    now: u64,
}

/// `afkconvoy tick`: one reconciliation pass of the fleet of the checkout
/// `repo`, start to finish. It never waits
/// for the workers it started, and leaves nothing on this host a later tick
/// needs: the next one rebuilds from GitHub and the remote. Told to `stop`, it
/// finishes the step it is in and makes none of those left, which fail. What
/// the tick `before` saw, when there was one, words its line and decides
/// nothing.
pub fn tick(
    repo: &Repo,
    me: &Me,
    agent: &Agent,
    stop: &Stop,
    before: Option<&Before>,
) -> Result<(Ticked, Fleet)> {
    let _observation = crate::orca::Observation::begin();
    // Each harness paused on this host is probed every hour (docs/adr/0020):
    // one that answers is back in the host kit for this tick's rebuild. A
    // probe that cannot be made leaves it paused, and costs the tick nothing.
    // The host kit is written again once a day, by one fleet of the host
    // (docs/adr/0020): a failure is a line of the log, and costs the tick
    // nothing.
    let _ = crate::paused::probe(repo, me.instance.as_str(), &me.host, me.now);
    let _ = crate::kit::refresh(repo, me.instance.as_str(), &me.host, me.now);
    // A harness held since the agent setup was last given gets it now
    // (sunfmin/control#528); with none declared this reads the settings alone.
    let _ = crate::kit::give_new_agent_setup(repo, me.instance.as_str(), &me.host, me.now);
    let (mut fleet, mut open) = Fleet::read(repo, me.now)?;
    // This is independent of the previous tick's log view: retry failed
    // closes, and cover a runner that restarted after the host woke.
    let mut stopped_taken = vec![];
    let mut taken_failures = vec![];
    for claim in &fleet.scan.claims {
        if claim.owner.as_ref().is_none_or(|owner| owner.host == me.host) {
            continue;
        }
        let closed = (|| -> Result<bool> {
            let Some(worktree) = crate::judge::worktree_of(&fleet, claim.number)? else {
                return Ok(false);
            };
            if worktree.terminal()?.is_none() {
                return Ok(false);
            }
            worktree.close_terminals()?;
            anyhow::ensure!(
                worktree.terminal()?.is_none(),
                "the worker on #{} still has a terminal after close; its worktree was kept and the next tick retries",
                claim.number
            );
            Ok(true)
        })();
        match closed {
            Ok(true) => stopped_taken.push(claim.number),
            Ok(false) => {}
            Err(error) => taken_failures.push(Failure::of(Failed::StopTaken(claim.number), &error)),
        }
    }
    // The look listed the candidate chain with the rest of the remote.
    // Removal answers from that listing. The chain already holds the gate's
    // evidence. A failure leaves the directory for the next tick; the pass
    // itself still runs.
    let candidates = repo.layout().under("candidates/");
    if let Err(error) =
        crate::candidates::remove_listed(repo, &fleet.records_at_open(&format!("{candidates}*")))
    {
        eprintln!(
            "warning: a stopped candidate worktree will be removed on a later tick: {error:#}"
        );
    }
    // A removal orca already answered success for, while the directory remains.
    // No claim is required: the settle that met it has already released its own.
    if let Err(error) = crate::orca::finish_failed_removals(&fleet.repo, &fleet.config.base_branch)
    {
        eprintln!("warning: a worktree a removal left will be removed on a later tick: {error:#}");
    }
    // Cover interruption after an internal task landed but before final review.
    // A bad delivery does not prevent unrelated task reconciliation.
    let delivery_failure = crate::delivery::reconcile(&fleet).err();
    if let Err(error) = crate::legacy::retire_proposals(&mut fleet, me) {
        eprintln!("warning: legacy proposal retirement will retry: {error:#}");
    }
    let migration = crate::legacy::migrate(repo, &fleet.config, &fleet.gh, &me.host, me.now);
    if !matches!(&migration, Ok(answer) if answer["outcome"] == "idle") {
        let (outcome, failed) = match migration {
            Ok(answer) => (answer.to_string(), false),
            Err(error) => (format!("{error:#}"), true),
        };
        let _ = crate::log::Log::of(repo).and_then(|log| {
            log.append(&crate::log::Entry {
                at: me.now,
                instance: me.instance.as_str().into(),
                host: me.host.clone(),
                what: crate::log::What::Command {
                    command: "migrate".into(),
                    on: None,
                    by: "fleet".into(),
                    outcome,
                    failed,
                },
            })
        });
        // Migration can open or close issues. Its partial failure must not
        // prevent ordinary worker recovery or the fleet's heartbeat.
        (fleet, open) = Fleet::read(repo, me.now)?;
    }
    if crate::repair::tend(&mut fleet, me)? == Outcome::Done {
        (fleet, open) = Fleet::read(repo, me.now)?;
    }
    let updates = crate::issue_updates::reconcile(&fleet, &me.host, &open.issues);
    for issue in &open.issues.rows {
        // Dispatch reports a resolution failure against its issue; successful
        // discovery must precede the choice of range.
        let _ = agent.discover_casting(&issue.labels);
    }
    let (mut set, mut flows) = fleet.rebuild(&open, Some(&me.instance))?;
    let _health_events = crate::health::reconcile(&mut fleet, me, agent, &open, &flows, &set)?;
    fleet.invalidate_selection();
    // A checkout that names a harness, or that just cleared one, continues
    // a worker still on another harness. A harness that cannot start, or a
    // continuation that does not land, waits for the next tick.
    let _ = crate::named_harness::converge(&mut fleet, me, agent);
    // A roster entry that names no model the host kit holds is the team's to
    // correct: one issue is opened for it in the team's repo. One that cannot
    // be opened costs the tick nothing; the next tries again.
    if let (Some(bound), Ok(team), Ok(kit)) =
        (&fleet.config.bound, flows.team(), crate::kit::load())
    {
        let unmatched = afkconvoy_core::casting::unmatched_models(team, &kit.range());
        let _ = crate::roster_issue::open(&fleet.gh, &bound.team.repo, &unmatched);
    }
    // What no consult saw goes to one first (docs/adr/0018), and is read
    // again as the fleet's. A team that does not load puts nothing: the
    // rebuild says why of each issue.
    let put = crate::stage::put_to_consults(&fleet, &flows, &open.issues).unwrap_or_default();
    if !put.is_empty() {
        (fleet, open) = Fleet::read(repo, me.now)?;
        (set, flows) = fleet.rebuild(&open, Some(&me.instance))?;
    }
    // A busy host starts no new worker this tick (docs/adr/0019).
    let admission = Admission::read(&fleet.repo.dir)?;
    set.held_back = admission.holds();
    let me_said = me.instance.as_str();
    let lost = before.map(|before| before.lost(&fleet.scan.claims, me_said)).unwrap_or_default();
    let bound = set.bound(fleet.config.concurrency());
    // A claimed issue the rebuild could not read is this tick's error, and
    // only its own.
    let mut failures: Vec<Failure> = std::mem::take(&mut fleet.unread)
        .into_iter()
        .map(|(issue, error)| Failure::of(Failed::Read(issue), &error))
        .collect();
    if let Some(error) = delivery_failure {
        failures.push(Failure::of(Failed::Delivery, &error));
    }
    failures.extend(taken_failures);
    let mut blocked = vec![];
    let read = Read { set: &set, flows: &flows, issues: &open.issues };
    let pass = run_pass(&set, bound, set.held(), |step| {
        let made = match stop.told() {
            true => Err(anyhow!("this run was told to stop before it made the step")),
            false => carry_out(&mut fleet, (me, agent), &read, step, &mut blocked),
        };
        // Whatever a step raised, the rest of the tick is still owed.
        made.unwrap_or_else(|error| {
            if error.is::<crate::paused::Held>() {
                if let TickStep::Dispatch { issue } = step
                    && !fleet
                        .scan
                        .claim_of(issue)
                        .is_some_and(|claim| claim.is_held_by(&me.instance))
                {
                    // The harness refused admission before a claim was won:
                    // this must leave the slot free for another harness.
                    return Outcome::Blocked;
                }
                return Outcome::Nothing;
            }
            failures.push(Failure::of(Failed::Step(step), &error));
            Outcome::Failed
        })
    });
    let escalated = std::mem::take(&mut fleet.escalated);
    let forgotten = std::mem::take(&mut fleet.forgotten);
    let closers = std::mem::take(&mut fleet.closers);
    let after_locks = std::mem::take(&mut fleet.after_locks);
    let cut_said = std::mem::take(&mut fleet.cut_said);
    let (concurrency, bound) = (fleet.config.concurrency(), pass.bound);
    let issues = &open.issues.rows;
    let config = &fleet.config;
    let flow_labels = flows.flow_labels();
    let handed = issues
        .iter()
        .filter_map(|issue| {
            handed_by(&issue.labels, config, &flow_labels).map(|label| (issue.number, label))
        })
        .collect();
    let beat = fleet.scan.last_beat(me_said);
    let saw = Before::saw(
        (&set.mine, issues),
        (&fleet.scan.claims, me_said, beat),
        (config, &flow_labels),
    );
    let ticked = Ticked {
        pass,
        updates,
        failures,
        escalated,
        blocked,
        forgotten,
        before: before.cloned(),
        lost,
        stopped_taken,
        closers,
        after_locks,
        cut_said,
        handed,
        saw,
        blockers: issues
            .iter()
            .filter(|issue| afkconvoy_core::tracker::repo_labels(&issue.labels).len() <= 1)
            .flat_map(|issue| issue.open_blockers.iter().copied())
            .collect(),
        concurrency,
        bound,
        set,
        flows,
        admission,
        now: me.now,
    };
    Ok((ticked, fleet))
}

impl Failure {
    fn of(step: Failed, error: &anyhow::Error) -> Failure {
        let unreached = unreached(error).is_some();
        let see = topic::explains(error);
        Failure { step, error: format!("{error:#}"), see, unreached }
    }
}

/// What the tick's rebuild read: its steps act on this, and read nothing of
/// an issue's place in its workflow again.
struct Read<'r> {
    set: &'r WorkingSet,
    flows: &'r Flows,
    issues: &'r crate::gh::OpenIssues,
}

/// Carries out `step` → how it ended. A dispatch that read an open blocker
/// adds the issue and its blockers to `blocked`.
fn carry_out(
    fleet: &mut Fleet,
    (me, agent): (&Me, &Agent),
    read: &Read,
    step: TickStep,
    blocked: &mut Vec<(u64, Vec<u64>)>,
) -> Result<Outcome> {
    // Handle the last turn first: ask a wait again without a harness, or
    // hand an answer needing tree changes to its worker without waiting for
    // a silence judgment. Worker health must not hide that answer.
    if let TickStep::Watch { issue, start } = step {
        let dependencies =
            read.issues.blockers.get(&issue).map_or(&[][..], |edges| edges.as_slice());
        if let Some(outcome) =
            crate::watch::answer_turn(fleet, me, agent, issue, start, dependencies)?
        {
            return Ok(outcome);
        }
    }
    // A terminal left by an incomplete dispatch is no worker to judge for
    // silence. Continue the same claim before health holds or the nudge
    // ladder can mistake its shell for a harness.
    let retry = match step {
        TickStep::Judge { issue, start: true }
        | TickStep::Watch { issue, start: true }
        | TickStep::Advance { issue } => Some(issue),
        _ => None,
    };
    if let Some(issue) = retry
        && crate::dispatch::incomplete(fleet, issue)?
    {
        dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
        return Ok(Outcome::Continued);
    }
    // A shared harness outage is no worker's silence. Keep the claim and
    // progress, including its one nudge, until its pending prompt can probe.
    let issue = match step {
        TickStep::Judge { issue, .. }
        | TickStep::Watch { issue, .. }
        | TickStep::Advance { issue } => Some(issue),
        _ => None,
    };
    if let Some(issue) = issue {
        let source = crate::orca::Source::of(&fleet.repo)?;
        if let Some(worktree) = crate::orca::Worktree::of_issue(&source, issue)? {
            let state = worktree.state(fleet.now)?;
            if state != afkconvoy_core::worker::WorkerState::Gone
                && crate::health::holding(&worktree)
            {
                // An open turn has no since. A later wait starts when seen again.
                if state == afkconvoy_core::worker::WorkerState::Busy {
                    worktree.background_since(false, None, fleet.now)?;
                    return Ok(Outcome::Nothing);
                }
                // The stamp worker health's one read wrote. Missing, that
                // read has not run: one read then answers both.
                if let Some(since) = worktree.background_at() {
                    return Ok(Outcome::Waiting(since));
                }
                if let Some(background) = crate::judge::own_background(
                    fleet,
                    &worktree,
                    true,
                    crate::judge::Look::Acting,
                )? {
                    return Ok(Outcome::Waiting(background.since));
                }
                return Ok(Outcome::Nothing);
            }
        }
    }
    match step {
        TickStep::Settle { issue } => Ok(match settle(fleet, me, issue)? {
            Some(since) => Outcome::Waiting(since),
            None => Outcome::Done,
        }),
        TickStep::Forget { issue } => forget(fleet, me, issue),
        TickStep::Judge { issue, start } => judge(fleet, me, agent, issue, start),
        TickStep::Watch { issue, start } => watch(fleet, me, agent, issue, start),
        TickStep::Advance { issue } => advance(fleet, me, agent, read.flows, issue),
        TickStep::Escalate { issue } => {
            let calling = read.set.escalating.iter().find(|calling| calling.number == issue);
            match calling {
                // Nothing was handed over: the step did not do what it names.
                Some(_) if run_again_unclaimed(fleet, read.flows, issue)? => Ok(Outcome::Nothing),
                Some(calling) => {
                    let rounds = read.flows.rounds(issue);
                    let why = (read.flows.brief(issue), calling.reason.as_str());
                    hand_to_human(fleet, me, issue, (&why.0, why.1), rounds.as_deref())
                }
                None => Ok(Outcome::Nothing),
            }
        }
        TickStep::EndPatrol { patrol } => match read.set.patrols.get(patrol) {
            Some(row) => patrol::end(fleet, me, read.flows, row),
            None => Ok(Outcome::Nothing),
        },
        TickStep::Patrol { patrol } => match read.set.patrols.get(patrol) {
            Some(row) => patrol::run(fleet, me, agent, read.flows, row),
            None => Ok(Outcome::Nothing),
        },
        TickStep::Unlock { issue } => unlock(fleet, me, issue),
        TickStep::Reclaim { issue } => reclaim(fleet, me, agent, read.flows, issue),
        TickStep::Dispatch { issue } => {
            let started = dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            if started["gone"].is_string() {
                return Ok(Outcome::Gone);
            }
            if let Some(blockers) = started["blocked_by"].as_array() {
                blocked.push((issue, blockers.iter().filter_map(Value::as_u64).collect()));
                return Ok(Outcome::Blocked);
            }
            Ok(if started["started"] == true { Outcome::Done } else { Outcome::Lost })
        }
        TickStep::EndCut => match &read.set.cut {
            Some(row) => crate::cut::end(fleet, me, read.flows, row),
            None => Ok(Outcome::Nothing),
        },
        TickStep::Cut { busy } => crate::cut::start(fleet, me, agent, busy),
        TickStep::SlowGate => match &read.set.slow_gate {
            Some(slow) => slow_gate::open(fleet, me, slow),
            None => Ok(Outcome::Nothing),
        },
        TickStep::Board { issue, stage, pr } => show(fleet, me, issue, stage, pr),
        TickStep::StageLabel { issue } => read.flows.bring(fleet, issue),
        TickStep::Heartbeat => {
            beat(&fleet.repo, &mut fleet.scan, me)?;
            Ok(Outcome::Done)
        }
    }
}

fn words(step: TickStep) -> (&'static str, Option<u64>) {
    match step {
        TickStep::Settle { issue } => ("settle", Some(issue)),
        TickStep::Forget { issue } => ("forget", Some(issue)),
        TickStep::Judge { issue, .. } => ("judge", Some(issue)),
        TickStep::Watch { issue, .. } => ("watch", Some(issue)),
        TickStep::Advance { issue } => ("advance", Some(issue)),
        TickStep::Escalate { issue } => ("escalate", Some(issue)),
        TickStep::EndPatrol { .. } => ("end_patrol", None),
        TickStep::Patrol { .. } => ("patrol", None),
        TickStep::Unlock { issue } => ("unlock", Some(issue)),
        TickStep::Reclaim { issue } => ("reclaim", Some(issue)),
        TickStep::Dispatch { issue } => ("dispatch", Some(issue)),
        TickStep::EndCut => ("end_cut", None),
        TickStep::Cut { .. } => ("cut", None),
        TickStep::SlowGate => ("slow_gate", None),
        TickStep::Board { issue, .. } => ("board", Some(issue)),
        TickStep::StageLabel { issue } => ("stage_label", Some(issue)),
        TickStep::Heartbeat => ("heartbeat", None),
    }
}

impl Ticked {
    /// The patrol `step` is on, when it is on one: a step names it by its
    /// place in the working set.
    fn patrol(&self, step: TickStep) -> Option<&str> {
        match step {
            TickStep::EndPatrol { patrol } | TickStep::Patrol { patrol } => {
                self.set.patrols.get(patrol).map(|patrol| patrol.name.as_str())
            }
            _ => None,
        }
    }

    /// The stage an advance that handed `issue` on left it at.
    fn advanced_to(&self, issue: u64) -> Option<String> {
        let handed_on = (TickStep::Advance { issue }, Outcome::HandedOn);
        self.pass.did.contains(&handed_on).then(|| self.flows.at(issue)).flatten()
    }

    /// Why `step`, which ended in `outcome`, escalated the issue it is on, in
    /// a few words; `None` of a step that escalated none.
    fn escalated_by(&self, step: TickStep, outcome: Outcome) -> Option<&str> {
        let ends = matches!(
            step,
            TickStep::Judge { .. }
                | TickStep::Watch { .. }
                | TickStep::Advance { .. }
                | TickStep::Escalate { .. }
        );
        let issue = words(step).1.filter(|_| ends && outcome == Outcome::Done)?;
        self.escalated.iter().find(|(number, _)| *number == issue).map(|(_, why)| why.as_str())
    }

    /// The locks the landing of `issue` stopped at, when this pass asked its
    /// turn again once they closed.
    fn after_lock(&self, issue: u64) -> Option<&[u64]> {
        self.after_locks.iter().find(|(number, _)| *number == issue).map(|(_, by)| by.as_slice())
    }

    /// The open blockers a dispatch of `issue` read just before its claim,
    /// when it skipped the issue for them.
    fn blocked_by(&self, issue: u64) -> Option<&[u64]> {
        self.blocked.iter().find(|(number, _)| *number == issue).map(|(_, by)| by.as_slice())
    }

    /// Why `issue` was dispatched, when the cause changed on GitHub since the
    /// tick before: `labelled ready-for-agent since tick 111`.
    fn labelled(&self, issue: u64) -> Option<String> {
        let since = self.before.as_ref()?.labelled(issue)?;
        let label = self.handed.iter().find(|(number, _)| *number == issue)?;
        Some(format!("labelled {} since tick {since}", label.1))
    }

    /// Who closed `issue`, when the tick settled its claim and something
    /// other than the fleet closed it since the tick before.
    fn closed_outside(&self, issue: u64) -> Option<String> {
        let mine = self.set.mine.iter().find(|mine| mine.number == issue)?;
        let outside = mine.status == Status::Closed && self.before.as_ref()?.closed_outside(issue);
        let by = self.closers.iter().find(|(number, _)| *number == issue);
        outside.then(|| match by {
            Some((_, login)) => format!("closed by @{login}"),
            None => "closed on GitHub".to_string(),
        })
    }

    /// What this tick saw as it ended, as tick `count` of its `run`: the next
    /// one's `before`.
    pub fn saw(&self, count: u64) -> Before {
        Before { tick: count, ..self.saw.clone() }
    }

    fn named(&self, step: TickStep) -> Value {
        let (word, issue) = words(step);
        let mut named = json!({"do": word, "issue": issue});
        if let Some(patrol) = self.patrol(step) {
            named["patrol"] = json!(patrol);
        }
        named
    }

    /// What failed, as an answer names it: a step, or the rebuild's read.
    fn failed_named(&self, failed: Failed) -> Value {
        match failed {
            Failed::Step(step) => self.named(step),
            Failed::Read(issue) => json!({"do": "read", "issue": issue}),
            Failed::StopTaken(issue) => json!({"do": "stop_taken", "issue": issue}),
            Failed::Delivery => json!({"do": "delivery"}),
        }
    }

    /// What failed, as the line names it: `settle #3`, `patrol triage`.
    fn failed_said(&self, failed: Failed) -> String {
        let step = match failed {
            Failed::Step(step) => step,
            Failed::Read(issue) => return format!("read #{issue}"),
            Failed::StopTaken(issue) => return format!("stop taken worker #{issue}"),
            Failed::Delivery => return "reconcile deliveries".to_string(),
        };
        let (word, issue) = words(step);
        let issue = issue.map(|issue| format!(" #{issue}")).unwrap_or_default();
        let patrol = self.patrol(step).map(|p| format!(" {p}")).unwrap_or_default();
        format!("{word}{issue}{patrol}")
    }

    /// Each issue the tick read with where it stands, as a host watch
    /// lists the worktrees of this fleet: in a bound repo the position its
    /// stage records give it, in an unbound one how far its claim is.
    pub fn stages(&self) -> Vec<(u64, String)> {
        let positions = self.flows.positions();
        if !positions.is_empty() {
            let said = |(number, _, position): &(u64, _, Position)| {
                let at = |stage: &str| format!("at stage `{stage}`");
                let said = match position {
                    Position::Perform(stage, _) => at(&stage.name),
                    Position::Waiting(stage) => format!("{}, waiting on a human", at(&stage.name)),
                    Position::Consult(stage, ..) => format!("{}, with a consult", at(&stage.name)),
                    Position::ParkedForEvidence(stage) => {
                        format!("parked for evidence {}", at(&stage.name))
                    }
                    Position::Parked(stage) => format!("parked {}", at(&stage.name)),
                    Position::Escalating(stage, _) => format!("escalating {}", at(&stage.name)),
                    Position::Landing => "landing".to_string(),
                    Position::LandingParked => "parked at land".to_string(),
                    Position::LandingEscalating => "escalating at land".to_string(),
                    Position::Ended => "ended".to_string(),
                };
                (*number, said)
            };
            return positions.iter().map(said).collect();
        }
        let said = |status: Status| match status {
            Status::Landed | Status::Closed | Status::Gone => "landed or closed",
            Status::NoPr | Status::Landing | Status::AtStage => "claimed, at work",
        };
        self.set.mine.iter().map(|mine| (mine.number, said(mine.status).to_string())).collect()
    }

    /// Work this pass would start in a free slot if admission admitted it.
    pub fn admission_waiting(&self) -> Vec<Value> {
        if !self.pass.held_back(&self.set) {
            return vec![];
        }
        afkconvoy_core::tick::takes(&self.set).into_iter().map(|step| self.named(step)).collect()
    }

    /// What admission read of the host this tick.
    pub fn admission(&self) -> &Admission {
        &self.admission
    }

    /// The panel of `me` after this tick: what its rebuild read, with what the
    /// pass did since.
    pub fn panel(&self, fleet: &Fleet, me: &Me) -> Panel {
        let admission = Ok(self.admission.clone());
        let report = FleetReport::admitted(fleet, &self.flows, &self.set, admission);
        let mut panel =
            panel::read(fleet, Some(me), &self.set, &self.flows, &self.pass.did, &report);
        panel.declared = self.declared().min(self.pass.held);
        panel.held = self.pass.held - panel.declared;
        panel.bound = self.bound - panel.declared;
        panel.frontier = self.pass.frontier_remaining;
        panel.idle = self.idle();
        panel
    }

    /// The claims whose worker declared a verdict that the pass left held:
    /// what its bound was raised by and still is (`Pass::bound`).
    fn declared(&self) -> usize {
        self.bound.saturating_sub(self.concurrency.saturating_add(self.set.left_alone()))
    }

    /// Whether this instance is idle after the tick (GLOSSARY: Idle). A cut
    /// of mine going is not idle.
    pub fn idle(&self) -> bool {
        let pass = &self.pass;
        idle(pass.held, pass.frontier_remaining) && !pass.cutting(&self.set)
    }

    /// Whether this instance was at work in the tick, as a move onto a newer
    /// release reads it (`Stands::AtWork`): it held a claim — as its rebuild
    /// read the claims, or as its pass ended — or a cut of mine is going,
    /// which takes no slot.
    pub fn at_work(&self) -> bool {
        self.set.held().max(self.pass.held) > 0 || self.pass.cutting(&self.set)
    }

    /// What the pass's start and end of a cut said, for the log to keep.
    pub fn cut_said(&self) -> &[log::What] {
        &self.cut_said
    }

    /// Appends what the pass's start and end of a cut said to the log of
    /// `repo`, as a `run` keeps it: what a tick run once does. Soft, as any
    /// command's line is.
    pub fn log_cut_said(&self, repo: &Repo, me: &Me) {
        let log = Log::of(repo);
        for what in &self.cut_said {
            let (instance, host) = (me.instance.as_str().to_string(), me.host.clone());
            let entry = log::Entry { at: me.now, instance, host, what: what.clone() };
            let appended = log.as_ref().map_err(|error| anyhow!("{error:#}"));
            if let Err(error) = appended.and_then(|log| log.append(&entry)) {
                eprintln!("warning: the cut left no line in the tick log: {error:#}");
            }
        }
    }

    /// Whether a step of the tick failed on a read GitHub could not be
    /// reached for: nothing says the next tick's read fails too.
    pub fn unreached(&self) -> bool {
        self.failures.iter().any(|failure| failure.unreached)
    }

    /// The claims held, as the line says them: those that take a slot against
    /// the `concurrency` they are held under, then the ones a fleet lock has
    /// the tick leave alone and those whose worker declared a verdict not
    /// acted on yet, which take none, then those past the bound — a
    /// start takes every claim stamped with its host, whatever the bound. So
    /// the first never shows more held than its bound.
    fn holding(&self) -> Vec<String> {
        let (held, concurrency) = (self.pass.held, self.concurrency);
        let left_alone = self.set.left_alone().min(held);
        let declared = self.declared().min(held - left_alone);
        let taking = held - left_alone - declared;
        let mut parts = vec![format!("holding {}/{concurrency}", taking.min(concurrency))];
        if left_alone > 0 {
            parts.push(format!("{left_alone} left alone"));
        }
        if declared > 0 {
            parts.push(format!("{declared} taking no slot"));
        }
        if taking > concurrency {
            parts.push(format!("{} past the bound", taking - concurrency));
        }
        parts
    }

    pub fn json(&self) -> Value {
        let mut did: Vec<Value> = self
            .pass
            .did
            .iter()
            .map(|(step, outcome)| {
                let ended = match outcome {
                    Outcome::Done | Outcome::HandedOn => "done",
                    Outcome::Nudged => "nudged",
                    Outcome::Waiting(_) => "waiting",
                    Outcome::Continued => "continued",
                    Outcome::Restarted => "restarted",
                    Outcome::Asked => "asked",
                    Outcome::Told => "told",
                    Outcome::Nothing => "nothing",
                    Outcome::Lost => "lost",
                    Outcome::Blocked => "blocked",
                    Outcome::Gone => "gone",
                    Outcome::Superseded => "superseded",
                    Outcome::Failed => "failed",
                };
                let mut did = self.named(*step);
                did["outcome"] = json!(ended);
                if *step == TickStep::SlowGate
                    && let Some(slow) = &self.set.slow_gate
                {
                    did["evidence"] = json!(afkconvoy_core::slow_gate::said(slow));
                }
                if let TickStep::Advance { issue } = step
                    && let Some(stage) = self.advanced_to(*issue)
                {
                    did["stage"] = json!(stage);
                }
                if let Outcome::Waiting(since) = outcome {
                    did["waiting_since"] = json!(since);
                }
                if let Some(why) = self.escalated_by(*step, *outcome) {
                    did["escalated"] = json!(why);
                }
                if let (TickStep::Watch { issue, .. }, Outcome::Asked) = (step, outcome)
                    && let Some(locks) = self.after_lock(*issue)
                {
                    did["after_locks"] = json!(locks);
                }
                if let (TickStep::Dispatch { issue }, Outcome::Blocked) = (step, outcome)
                    && let Some(blockers) = self.blocked_by(*issue)
                {
                    did["blocked_by"] = json!(blockers);
                }
                did
            })
            .collect();
        let mut errors: Vec<Value> = self
            .failures
            .iter()
            .map(|failure| {
                let mut failed = self.failed_named(failure.step);
                failed["error"] = json!(failure.error);
                if let Some(see) = failure.see {
                    failed["see"] = json!(see);
                }
                failed
            })
            .collect();
        did.extend(self.updates.iter().cloned());
        errors.extend(self.updates.iter().filter(|step| step["outcome"] == "failed").cloned());
        let mut answer = json!({"did": did, "errors": errors, "held": self.pass.held,
            "frontier_remaining": self.pass.frontier_remaining, "idle": self.idle(),
            "locks": self.set.locks});
        if let Some(reason) = self.set.cut.as_ref().and_then(|cut| cut.not_cutting_here.as_ref()) {
            answer["not_cutting_here"] = json!(reason);
        }
        if self.pass.held_back(&self.set) {
            answer["held_back_by"] = json!(self.held_back_by());
        }
        answer
    }

    /// The thresholds of the host admission held the pass back by, each as
    /// it is said.
    fn held_back_by(&self) -> Vec<String> {
        self.set.held_back.iter().map(|hold| self.admission.said(hold)).collect()
    }

    /// One line for whoever watches the loop: what the tick did, and where the
    /// fleet stands.
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = vec![];
        if !self.set.locks.is_empty() {
            parts.push(format!("FLEET LOCK {} stands: only it is worked", named(&self.set.locks)));
        }
        let lead = parts.len();
        // A claim another fleet instance took since the tick before.
        parts.extend(self.lost.iter().map(|lost| {
            let stopped =
                if self.stopped_taken.contains(&lost.issue) { "; stopped its worker" } else { "" };
            format!("{}{stopped}", lost.said())
        }));
        for issue in &self.stopped_taken {
            if !self.lost.iter().any(|lost| lost.issue == *issue) {
                parts.push(format!(
                    "stopped its worker on #{issue}: its claim is held on another host"
                ));
            }
        }
        // An issue that left the fleet for a human says so, whichever step
        // handed it over: none of them names it as anything else.
        let escalated: Vec<String> =
            self.escalated.iter().map(|(issue, why)| format!("#{issue} ({why})")).collect();
        if !escalated.is_empty() {
            parts.push(format!("escalated {}", escalated.join(", ")));
        }
        // An issue that is gone says so, whichever step found it.
        for (gone, released) in &self.forgotten {
            let verb = if *released { "released" } else { "skipped" };
            parts.push(format!("{verb} #{} ({})", gone.issue, gone.said()));
        }
        // A claim settled on an issue closed outside the fleet is let go of:
        // nothing of the fleet's landed.
        let left: Vec<String> = self
            .pass
            .did
            .iter()
            .filter(|(step, outcome)| words(*step).0 == "settle" && *outcome == Outcome::Done)
            .filter_map(|(step, _)| words(*step).1)
            .filter_map(|issue| self.closed_outside(issue).map(|why| format!("#{issue} ({why})")))
            .collect();
        if !left.is_empty() {
            parts.push(format!("left {}", left.join(", ")));
        }
        for (verb, wanted) in [
            ("settled", "settle"),
            ("ended", "judge"),
            ("advanced", "advance"),
            ("unlocked", "unlock"),
            ("reclaimed", "reclaim"),
            ("dispatched", "dispatch"),
        ] {
            let issues: Vec<String> = self
                .pass
                .did
                .iter()
                .filter(|(step, outcome)| {
                    words(*step).0 == wanted
                        && matches!(outcome, Outcome::Done | Outcome::HandedOn)
                        && self.escalated_by(*step, *outcome).is_none()
                })
                .filter_map(|(step, _)| words(*step).1)
                .filter(|issue| wanted != "settle" || self.closed_outside(*issue).is_none())
                .map(|issue| match (wanted, self.advanced_to(issue), self.labelled(issue)) {
                    ("advance", Some(stage), _) => format!("#{issue} → {stage}"),
                    ("dispatch", _, Some(why)) => format!("#{issue} ({why})"),
                    _ => format!("#{issue}"),
                })
                .collect();
            if !issues.is_empty() {
                parts.push(format!("{verb} {}", issues.join(", ")));
            }
        }
        let skipped: Vec<String> = self
            .blocked
            .iter()
            .map(|(issue, by)| {
                let by: Vec<String> = by.iter().map(|blocker| format!("#{blocker}")).collect();
                let since = self.before.as_ref().and_then(|before| before.unblocked(*issue));
                match since {
                    Some(tick) => {
                        format!("#{issue} (blocked by {}, added since tick {tick})", by.join(", "))
                    }
                    None => format!("#{issue} (blocked by {})", by.join(", ")),
                }
            })
            .collect();
        if !skipped.is_empty() {
            parts.push(format!("skipped {}", skipped.join(", ")));
        }
        for (verb, kept) in [
            ("nudged", Outcome::Nudged),
            ("continued", Outcome::Continued),
            ("restarted", Outcome::Restarted),
            ("asked for a turn again for", Outcome::Asked),
            ("told its turn's answer to", Outcome::Told),
        ] {
            let issues: Vec<String> = self
                .pass
                .did
                .iter()
                .filter(|(_, outcome)| *outcome == kept)
                .filter_map(|(step, _)| words(*step).1)
                .map(|issue| {
                    after_lock_said(
                        issue,
                        self.after_lock(issue).filter(|_| kept == Outcome::Asked),
                    )
                })
                .collect();
            if !issues.is_empty() {
                parts.push(format!("{verb} {}", issues.join(", ")));
            }
        }
        // A worker left as it waits on its own background work, and since when:
        // it is not silent, and the line says why it was not nudged.
        for (step, outcome) in &self.pass.did {
            if let (TickStep::Settle { issue }, Outcome::Waiting(_)) = (step, outcome) {
                parts.push(format!("settling #{issue} once its worker leaves"));
            }
        }
        let waiting: Vec<String> = self
            .pass
            .did
            .iter()
            .filter(|(step, _)| !matches!(step, TickStep::Settle { .. }))
            .filter_map(|(step, outcome)| match outcome {
                Outcome::Waiting(since) => {
                    let since = waiting_since(*since, self.now);
                    Some(match words(*step).1 {
                        Some(issue) => format!("#{issue} ({since})"),
                        None => format!("the repo command ({since})"),
                    })
                }
                _ => None,
            })
            .collect();
        if !waiting.is_empty() {
            parts.push(format!("waiting on its own background work {}", waiting.join(", ")));
        }
        for (verb, wanted) in [("ran the patrol", "patrol"), ("ended the patrol", "end_patrol")] {
            let patrols: Vec<&str> = self
                .pass
                .did
                .iter()
                .filter(|(step, outcome)| words(*step).0 == wanted && *outcome == Outcome::Done)
                .filter_map(|(step, _)| self.patrol(*step))
                .collect();
            if !patrols.is_empty() {
                parts.push(format!("{verb} {}", patrols.join(", ")));
            }
        }
        if self.pass.did.contains(&(TickStep::SlowGate, Outcome::Done)) {
            let evidence = self
                .set
                .slow_gate
                .as_ref()
                .map(afkconvoy_core::slow_gate::said)
                .unwrap_or_default();
            parts.push(format!("opened an issue for the slow gate: {evidence}"));
        }
        if let Some(name) = self.set.cut.as_ref().and_then(|cut| cut.not_cutting_here.as_ref()) {
            parts.push(name.clone());
        }
        parts.extend(self.cut_said.iter().filter_map(log::cut_said));
        for update in &self.updates {
            let issue = &update["issue"];
            let comment = update["comment"].as_str().unwrap_or("issue updates");
            parts.push(if update["outcome"] == "failed" {
                format!("FAILED tell #{issue}: {}", update["error"].as_str().unwrap_or_default())
            } else if update["outcome"] == "queued" {
                format!("queued at #{issue}'s worker, not yet taken: {comment}")
            } else {
                format!("told #{issue}: {comment}")
            });
        }
        if self.pass.held_back(&self.set) {
            parts.push(format!(
                "admission holds back new workers: {}",
                self.held_back_by().join("; ")
            ));
        }
        // A tick whose every step failed had things to do: its failures say so.
        if parts.len() == lead && self.failures.is_empty() {
            parts.push("nothing to do".to_string());
        }
        parts.extend(self.holding());
        parts.push(format!("frontier {}", self.pass.frontier_remaining));
        // One cause is said once, however many steps it stopped.
        for (failure, steps) in log::by_cause(&self.failures, |failure| &failure.error) {
            if steps > 1 {
                parts.push(format!("FAILED {steps} steps: {}", failure.error));
                continue;
            }
            parts.push(format!("FAILED {}: {}", self.failed_said(failure.step), failure.error));
        }
        parts.join(" · ")
    }
}

/// How `run` paces itself; the knobs exist for the tests. What the host says
/// of hot workers is its host settings', read as they are used
/// (`host_settings`).
pub struct Pacing {
    /// Stop after this many ticks [default: never].
    pub ticks: Option<u64>,
    /// How long one second of sleep really takes.
    pub second: Duration,
    /// Panic as this tick ends, its line kept: the tests' broken instance.
    pub panic_at: Option<u64>,
}

/// What a start says it took, when it took or lost anything.
fn start_line(took: &Value) -> Option<String> {
    let issues = |key: &str| -> Vec<String> {
        let numbers = took[key].as_array().into_iter().flatten();
        numbers.filter_map(|n| n.as_u64()).map(|n| format!("#{n}")).collect()
    };
    let runs = |key: &str| -> Vec<String> {
        let keys = took[key].as_array().into_iter().flatten();
        keys.filter_map(|key| key.as_str()).map(run_said).collect()
    };
    let swept = runs("swept").len();
    let taken = [issues("taken"), runs("taken_runs")].concat();
    let lost = [issues("lost"), runs("lost_runs")].concat();
    let mut parts = vec![];
    if !taken.is_empty() {
        parts.push(format!("took {} stamped with this host", taken.join(", ")));
    }
    if !lost.is_empty() {
        parts.push(format!("lost {} to a peer", lost.join(", ")));
    }
    if swept > 0 {
        parts.push(format!("swept {swept} dead heartbeat(s) that held nothing"));
    }
    if parts.is_empty() { None } else { Some(parts.join(" · ")) }
}

/// The longest a `run` that sleeps goes without hearing that it was told to
/// stop.
const HEARS_WITHIN: Duration = Duration::from_millis(250);

/// How many seconds past its length a rest takes before the host is taken to
/// have slept through it.
const SLEPT_PAST: u64 = 60;

/// How a rest ended.
#[derive(PartialEq)]
enum Rested {
    /// It slept the whole of it.
    Slept,
    /// A wake ended it, naming what sent it, where it did.
    Woken(Option<String>),
    Stopped,
}

/// Sleeps for `sleep`, or until a wake, or until this run is told to stop →
/// how it ended. A nap the wall clock says took far longer than it was to is
/// the host waking from a sleep: said as it is noticed.
fn rest(wakes: &Wakes, stop: &Stop, sleep: Duration, (said, clock): (&Said, &Clock)) -> Rested {
    let mut left = sleep;
    while !stop.told() {
        let nap = left.min(HEARS_WITHIN);
        let before = clock();
        let woken = wakes.rest(nap);
        let took = clock().saturating_sub(before);
        if took > nap.as_secs() + SLEPT_PAST {
            said.keep(clock(), log::What::HostWoke { asleep: took - nap.as_secs() });
        }
        if let Some(woken) = woken {
            return Rested::Woken(woken.by);
        }
        if nap == left {
            return Rested::Slept;
        }
        left -= nap;
    }
    Rested::Stopped
}

/// What the `run` of the checkout `repo`, told to stop, says of its
/// heartbeat as it goes (`goodbye`).
fn stop_line(repo: &Repo, me: &Me) -> String {
    match goodbye(repo, me) {
        Ok(true) => "holding no claim, it took its heartbeat with it".to_string(),
        Ok(false) => "it left its heartbeat with the claims it holds: the next start on this \
                      host takes them"
            .to_string(),
        Err(error) => format!("FAILED to take its heartbeat with it: {error:#}"),
    }
}

/// The clock a `run` reads, in epoch seconds: shared with the thread that
/// hears its wakes.
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// One line a fleet instance says: one it keeps in its tick log as it says it,
/// or one that no tick log holds.
pub enum Line<'l> {
    /// Kept in its tick log (`Said::keep`).
    Kept(&'l log::Entry),
    /// Said and kept nowhere, as `run` printed and never logged it: what its
    /// start took.
    Said(&'l str),
    /// What it warns of: its startup probe's warnings, a worker launch
    /// command with no unattended flag, what it cannot do and does without —
    /// a tick log it cannot keep.
    Warning(&'l str),
}

/// Where a fleet instance's lines go besides its tick log: printed, by
/// `run --foreground`; in the host runner, to whoever waits for its first
/// tick and to the host runner's log.
pub type Echo = Arc<dyn Fn(Line<'_>) + Send + Sync>;

/// Where a fleet instance's part of the panel goes after each tick: in the
/// host runner, to the page it serves.
pub type Show = Arc<dyn Fn(Panel) + Send + Sync>;

/// What holds a fleet instance's loop — the host runner: the stop it obeys,
/// where its lines go, the wakes heard for it through the host runner
/// (docs/adr/0023), what the host runner holds for all its fleet instances
/// (how each stands, the watch running, the exclusive
/// queue), and where its part of the panel goes: `None` where none is
/// shown, and none is made.
pub struct Holder {
    pub stop: Stop,
    pub echo: Echo,
    pub wakes: Wakes,
    pub shared: Shared,
    pub show: Option<Show>,
}

/// The log a `run` keeps of itself (`log::Log`), and its echo: each line is
/// kept and then echoed, one at a time, whichever thread says it, so the two
/// hold the same lines in the same order, and whoever hears a line finds it
/// in the log. A log that cannot be written is said once and costs only what
/// `doctor` could have read of it: no tick waits for its log.
struct Said {
    log: Option<Log>,
    instance: String,
    host: String,
    echo: Echo,
    /// Held while a line is written and echoed.
    one: Mutex<()>,
}

impl Said {
    /// Opens the log of the checkout `repo` and says in it that `me`
    /// started, with `agent`; every line goes to `echo` too.
    fn open(repo: &Repo, me: &Me, agent: &Agent, echo: Echo) -> Said {
        let log = Log::of(repo);
        let instance = me.instance.as_str().to_string();
        let host = me.host.clone();
        let mut said = Said { log: log.ok(), instance, host, echo, one: Mutex::default() };
        // First words alone (`Launches::logged`): full commands stay in the host kit.
        let (worker_command, harness_commands) = agent.launches.logged();
        said.keep(me.now, log::What::Start { worker_command, harness_commands });
        said.log = said.log.filter(|log| log.file.exists());
        if said.log.is_none() {
            said.warn("this fleet instance keeps no tick log: `afkconvoy doctor` has none to read");
        }
        said
    }

    fn keep(&self, at: u64, what: log::What) {
        let (instance, host) = (self.instance.clone(), self.host.clone());
        let entry = log::Entry { at, instance, host, what };
        let _one = self.one.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(log) = &self.log {
            // Soft: the line is echoed too, and the next one is tried again.
            let _ = log.keep(&entry);
        }
        (self.echo)(Line::Kept(&entry));
    }

    /// Says `line`, which no log keeps, in turn with the lines kept.
    fn say(&self, line: &str) {
        let _one = self.one.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.echo)(Line::Said(line));
    }

    /// Warns of `warning`, which no log keeps, in turn with the lines kept.
    fn warn(&self, warning: &str) {
        let _one = self.one.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.echo)(Line::Warning(warning));
    }
}

impl hot::Hears for Said {
    fn turned(&self, at: u64, what: log::What) {
        self.keep(at, what);
    }

    fn warn(&self, warning: &str) {
        Said::warn(self, warning);
    }
}

/// `afkconvoy run`: tick after tick of the fleet of the checkout `repo`,
/// sleeping in between — a sleep a worker's
/// `afkconvoy wake` ends early, and that is skipped after a tick that left the
/// next one a worker to start (`Pass::follow_up_due`) or that failed on a read
/// GitHub could not be reached for. Told to stop — SIGTERM, SIGINT — it
/// finishes the step it is in and exits: no claim is released on the way out,
/// and the next start on this host takes them back (docs/adr/0003); holding
/// none, it removes its heartbeat. A start removes the heartbeats left by the
/// runs that could not (`at_start`). It shows its part of the panel to
/// `holder` after every tick, where that shows one. It keeps a line in its
/// log (`log::Log`) as each tick starts, with why, and as it ends, and one of
/// each event that reaches it — a wake
/// heard during a tick, said as it is heard — and prints each as it is
/// written (`Said`); nothing here ever reads the log. It watches what each
/// worker's processes take, and says as one turns hot and as it stops being
/// hot (`hot::watch`, docs/adr/0019). Resource conditions are kept for the
/// host watch's next look. It says how it stands as each tick begins and
/// ends, among the others its host runner holds (`Standing`, docs/adr/0023),
/// and begins no tick while a move holds them back (docs/adr/0032). It never
/// puts the host to sleep (docs/adr/0050). It obeys `holder`'s stop — whoever holds the
/// process listens for the signals — and says each line to its echo. It
/// ends in an error as its tracker is found public (`probe::public_tracker`).
pub fn run(
    repo: &Repo,
    me: &mut Me,
    agent: &Agent,
    pacing: &Pacing,
    clock: impl Fn() -> u64 + Send + Sync + 'static,
    holder: Holder,
) -> Result<Value> {
    let Holder { stop, echo, wakes, shared, show } = holder;
    let clock: Clock = Arc::new(clock);
    let mut shown = Panel::empty(&Fleet::open(repo, me.now)?, Some(me));
    let mut streak = 0;
    let mut skipped = 0;
    let mut count = 0;
    let mut started = false;
    // What the last tick that rebuilt saw: the next one's line names what
    // changed outside this host since.
    let mut before: Option<Before> = None;
    let said = Arc::new(Said::open(repo, me, agent, echo));
    let during: During = {
        let (said, clock) = (Arc::clone(&said), Arc::clone(&clock));
        Box::new(move |during, by| said.keep(clock(), log::What::WokenDuring { during, by }))
    };
    wakes.telling(during);
    let watching = shared.blockers.watch(me.instance.as_str(), &wakes);
    let hot = Hot::default();
    {
        let hears: Arc<dyn hot::Hears> = Arc::clone(&said) as _;
        let (second, clock, hot) = (pacing.second, Arc::clone(&clock), Arc::clone(&hot));
        hot::watch(
            repo,
            second,
            clock,
            hears,
            (hot, shared.resources.clone()),
            wakes.waker(),
            stop.clone(),
        );
    }
    let mut cause = Cause::Start;
    let mut by = None;
    let standing = Standing::open(me.instance.as_str(), &shared.standings);
    loop {
        if !standing.begins_unless_stopped(&stop) {
            said.keep(clock(), log::What::Stop { line: stop_line(repo, me) });
            return Ok(json!({"stopped": true, "ticks": count, "instance": me.instance.as_str()}));
        }
        // Each tick is a look of its own: what a look reads once, it reads
        // once in this tick, not once in the runner's life, so what a `run`
        // registered since is seen (docs/adr/0043).
        crate::look::begin();
        // A tracker made public ends this fleet instance before it works
        // anything more — within the hour a check that found it private
        // stands, and at once of one said anew — as a start's probe refuses
        // one: it is broken, and each start of it again
        // is refused until the tracker is private or another is said
        // (docs/adr/0027). A check that cannot be made costs nothing: the
        // tick reads GitHub too, and says why that fails.
        if let Ok(Some(why)) = crate::probe::public_tracker(repo, clock(), true) {
            return Err(See::Troubleshooting.refusal(why));
        }
        if let Some(why) = crate::probe::check_gate_needs(repo, clock())? {
            return Err(See::Troubleshooting.refusal(why));
        }
        count += 1;
        said.keep(clock(), log::What::TickStarted { count, cause, by: by.take() });
        me.now = clock();
        // Once: a start takes every claim stamped with this host (docs/adr/0003).
        if !started {
            match Fleet::open(repo, me.now).and_then(|mut fleet| at_start(&mut fleet, me)) {
                Ok(took) => {
                    started = true;
                    if let Some(line) = start_line(&took) {
                        said.say(&format!("start: {line}"));
                    }
                }
                Err(error) => {
                    said.say(&format!("start: FAILED to take this host's claims: {error:#}"))
                }
            }
        }
        let served = show.is_some();
        watching.begins();
        // A tick that could not even rebuild is a line, not the end of the loop.
        let ticked = turn(
            || wakes.forget(count),
            || {
                // The served panel belongs to this same tick's view.
                let _observation = crate::orca::Observation::begin();
                let (ticked, fleet) = tick(repo, me, agent, &stop, before.as_ref())?;
                let tracker = match fleet.gh.tracker.named() {
                    Some(named) => Ok(named.to_string()),
                    None => crate::checkout_list::named_repo(repo)
                        .map(|named| named.to_string())
                        .or_else(|_| crate::checkout_list::repo_of(repo)),
                };
                // Wake routing is best effort, like a command's wake: a
                // name this host cannot read never fails a completed tick.
                if let Ok(tracker) = tracker {
                    watching.saw(&tracker, ticked.blockers.iter().copied(), &fleet.gh.closed());
                }
                let panel = served.then(|| ticked.panel(&fleet, me));
                Ok::<_, anyhow::Error>((ticked, panel))
            },
        );
        wakes.ended();
        // A tick that failed whole is at work: it may have held claims.
        let at_work = ticked.as_ref().map_or(true, |(ticked, _)| ticked.at_work());
        let mut cut_said = vec![];
        let (line, sleep, logged) = match ticked {
            Ok((ticked, panel)) => {
                cut_said = ticked.cut_said().to_vec();
                if let Some(panel) = panel {
                    shown = panel;
                }
                let main = repo.main_checkout().unwrap_or_else(|_| repo.dir.clone());
                shared.resources.ticked(
                    clock(),
                    ticked.admission(),
                    main,
                    ticked.stages(),
                    ticked.admission_waiting(),
                )?;
                before = Some(ticked.saw(count));
                let pass = &ticked.pass;
                // A tick GitHub could not be reached in was not an empty one:
                // what it could not read may be work.
                let busy = pass.did_work() || ticked.unreached();
                streak = empty_streak(streak, busy, pass.held, pass.frontier_remaining);
                let due = pass.follow_up_due(ticked.bound) || ticked.unreached();
                skipped = sleeps_skipped(skipped, due);
                let sleep = if skipped > 0 { 0 } else { pace(busy, pass.held, streak) };
                let logged = (Some(ticked.bound), Some(ticked.json()), None);
                (ticked.line(), sleep, logged)
            }
            Err(error) => {
                skipped = sleeps_skipped(skipped, unreached(&error).is_some());
                let sleep = if skipped > 0 { 0 } else { pace(true, 0, 0) };
                let error = format!("{error:#}");
                (format!("FAILED: {error}"), sleep, (None, None, Some(error)))
            }
        };
        let hot_now = hot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        for what in cut_said {
            said.keep(clock(), what);
        }
        // A launch command a worker first needed in this tick may warn.
        for warning in agent.launches.warned() {
            said.warn(&warning);
        }
        let (bound, answer, error) = logged;
        // Shown before its line is kept: whoever that line answers finds it on the page.
        if let Some(show) = &show {
            // A tick that failed leaves its part as the last one found it.
            let line = line.clone();
            shown.tick = Some(panel::Tick { count, line, ended: clock(), sleep });
            shown.hot = hot_now.iter().map(panel::Hot::from).collect();
            show(shown.clone());
        }
        let tick = log::Tick { count, sleep, line, bound, answer, error };
        said.keep(clock(), log::What::Tick(tick));
        // Resource observations and logging finish before a release move.
        standing.ticked(at_work);
        if pacing.panic_at == Some(count) {
            panic!("told to panic as tick {count} ended (--panic-at-tick)");
        }
        if pacing.ticks.is_some_and(|ticks| count >= ticks) {
            return Ok(json!({"stopped": true, "ticks": count, "instance": me.instance.as_str()}));
        }
        cause = if sleep == 0 { Cause::FollowUp } else { Cause::Timer };
        if let Rested::Woken(woken) =
            rest(&wakes, &stop, pacing.second * sleep as u32, (&said, &clock))
            && sleep > 0
        {
            (cause, by) = (Cause::Woken, woken);
        }
        if stop.told() {
            said.keep(clock(), log::What::Stop { line: stop_line(repo, me) });
            return Ok(json!({"stopped": true, "ticks": count, "instance": me.instance.as_str()}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_when_a_worker_waits_is_said_in_utc() {
        assert_eq!(waiting_since(0, 30), "since 1970-01-01 00:00 UTC, 30s ago");
        let at = 1_792_537_380; // 2026-10-20 23:03 UTC
        assert_eq!(waiting_since(at, at + 720), "since 2026-10-20 23:03 UTC, 12m ago");
        assert_eq!(waiting_since(951_782_400, 0), "since 2000-02-29 00:00 UTC, just now");
    }

    #[test]
    fn a_landing_asked_again_after_a_lock_names_the_lock_that_closed() {
        assert_eq!(after_lock_said(491, Some(&[497])), "#491 (fleet lock #497 closed)");
        assert_eq!(after_lock_said(7, Some(&[8, 9])), "#7 (fleet locks #8, #9 closed)");
        assert_eq!(after_lock_said(7, Some(&[])), "#7 (the fleet lock it stopped at closed)");
        assert_eq!(after_lock_said(7, None), "#7");
    }
}

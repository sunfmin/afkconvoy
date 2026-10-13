//! The tick's effect half: carries out the steps the core's pass hands it, and
//! the loop that runs one tick after another.

use std::time::Duration;

use afkconvoy_core::lock::named;
use afkconvoy_core::plan::WorkingSet;
use afkconvoy_core::takeover::run_said;
use afkconvoy_core::tick::{Outcome, Pass, TickStep, empty_streak, pace, run_pass, sleeps_skipped};
use afkconvoy_core::wake::turn;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::advance::{advance, hand_to_human, run_again_unclaimed};
use crate::board::show;
use crate::claims::{Me, beat, goodbye};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::gh::unreached;
use crate::git::Repo;
use crate::judge::judge;
use crate::log::{self, Log};
use crate::panel::{self, Panel};
use crate::patrol;
use crate::precedent;
use crate::serve::Served;
use crate::settle::settle;
use crate::stage::Flows;
use crate::stop::Stop;
use crate::takeover::{at_start, reclaim, unlock};
use crate::tend::tend_train;
use crate::wake::Listener;
use crate::watch::watch;

/// One step that failed, and why.
struct Failure {
    step: TickStep,
    error: String,
    /// Whether it failed on a read GitHub could not be reached for.
    unreached: bool,
}

/// What one tick came to.
pub struct Ticked {
    pub pass: Pass,
    failures: Vec<Failure>,
    /// The issues the pass escalated, each with why in a few words
    /// (`Fleet::escalated`): whichever step did.
    escalated: Vec<(u64, String)>,
    /// The claims the pass could hold (`WorkingSet::bound`).
    bound: usize,
    /// What its rebuild read: the pass was made over these.
    set: WorkingSet,
    flows: Flows,
}

/// `afkconvoy tick`: one reconciliation pass, start to finish. It never waits
/// for the workers it started, and leaves nothing on this host a later tick
/// needs: the next one rebuilds from GitHub and the remote. Told to `stop`, it
/// finishes the step it is in and makes none of those left, which fail.
pub fn tick(me: &Me, agent: &Agent, stop: &Stop) -> Result<(Ticked, Fleet)> {
    let (mut fleet, open) = Fleet::read(me.now)?;
    let (set, flows) = fleet.rebuild(&open, Some(&me.instance))?;
    let bound = set.bound(fleet.config.concurrency());
    let mut failures = vec![];
    let read = Read { set: &set, flows: &flows };
    let pass = run_pass(&set, bound, set.held(), |step| {
        let made = match stop.told() {
            true => Err(anyhow!("this run was told to stop before it made the step")),
            false => carry_out(&mut fleet, me, agent, &read, step),
        };
        // Whatever a step raised, the rest of the tick is still owed.
        made.unwrap_or_else(|error| {
            let unreached = unreached(&error).is_some();
            failures.push(Failure { step, error: format!("{error:#}"), unreached });
            Outcome::Failed
        })
    });
    let escalated = std::mem::take(&mut fleet.escalated);
    let ticked = Ticked { pass, failures, escalated, bound, set, flows };
    Ok((ticked, fleet))
}

/// What the tick's rebuild read: its steps act on this, and read nothing of
/// an issue's place in its workflow again.
struct Read<'r> {
    set: &'r WorkingSet,
    flows: &'r Flows,
}

fn carry_out(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    read: &Read,
    step: TickStep,
) -> Result<Outcome> {
    match step {
        TickStep::Settle { issue } => {
            settle(fleet, me, issue)?;
            Ok(Outcome::Done)
        }
        TickStep::TendTrain => tend_train(fleet, me, agent),
        TickStep::Judge { issue } => judge(fleet, me, agent, issue),
        TickStep::Watch { issue } => watch(fleet, me, agent, issue),
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
        TickStep::EndProposal { proposal } => match read.set.proposals.get(proposal) {
            Some(row) => precedent::end(fleet, me, row),
            None => Ok(Outcome::Nothing),
        },
        TickStep::Propose { proposal } => match read.set.proposals.get(proposal) {
            Some(row) => precedent::propose(fleet, me, agent, read.flows, row),
            None => Ok(Outcome::Nothing),
        },
        TickStep::Unlock { issue } => unlock(fleet, me, issue),
        TickStep::Reclaim { issue } => reclaim(fleet, me, agent, read.flows, issue),
        TickStep::Dispatch { issue } => {
            let started = dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            Ok(if started["started"] == true { Outcome::Done } else { Outcome::Lost })
        }
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
        TickStep::TendTrain => ("tend_train", None),
        TickStep::Judge { issue } => ("judge", Some(issue)),
        TickStep::Watch { issue } => ("watch", Some(issue)),
        TickStep::Advance { issue } => ("advance", Some(issue)),
        TickStep::Escalate { issue } => ("escalate", Some(issue)),
        TickStep::EndPatrol { .. } => ("end_patrol", None),
        TickStep::Patrol { .. } => ("patrol", None),
        TickStep::EndProposal { .. } => ("end_proposal", None),
        TickStep::Propose { .. } => ("propose", None),
        TickStep::Unlock { issue } => ("unlock", Some(issue)),
        TickStep::Reclaim { issue } => ("reclaim", Some(issue)),
        TickStep::Dispatch { issue } => ("dispatch", Some(issue)),
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

    /// The issue whose ruling `step` proposes as a precedent, when it is on a
    /// proposal: a step names it by its place in the working set.
    fn proposal(&self, step: TickStep) -> Option<u64> {
        match step {
            TickStep::EndProposal { proposal } | TickStep::Propose { proposal } => {
                self.set.proposals.get(proposal).map(|proposal| proposal.ruling.issue)
            }
            _ => None,
        }
    }

    /// One step as an answer names it: what it does, and the issue or the
    /// patrol it is on. A step on a proposal names the issue that was ruled
    /// on.
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

    fn named(&self, step: TickStep) -> Value {
        let (word, issue) = words(step);
        let mut named = json!({"do": word, "issue": issue.or(self.proposal(step))});
        if let Some(patrol) = self.patrol(step) {
            named["patrol"] = json!(patrol);
        }
        named
    }

    /// The panel of `me` after this tick: what its rebuild read, with what the
    /// pass did since.
    pub fn panel(&self, fleet: &Fleet, me: &Me) -> Panel {
        let mut panel = panel::read(fleet, Some(me), &self.set, &self.flows, &self.pass.did);
        panel.held = self.pass.held;
        panel
    }

    /// Whether a step of the tick failed on a read GitHub could not be
    /// reached for: nothing says the next tick's read fails too.
    pub fn unreached(&self) -> bool {
        self.failures.iter().any(|failure| failure.unreached)
    }

    /// The claims held, as the line says them: those that take a slot against
    /// the `concurrency` they are held under, then the ones a fleet lock has
    /// the tick leave alone, which take none, then those past the bound — a
    /// start takes every claim stamped with its host, whatever the bound. So
    /// the first never shows more held than its bound.
    fn holding(&self) -> Vec<String> {
        let left_alone = self.set.left_alone().min(self.pass.held);
        let concurrency = self.bound.saturating_sub(self.set.left_alone());
        let taking = self.pass.held - left_alone;
        let mut parts = vec![format!("holding {}/{concurrency}", taking.min(concurrency))];
        if left_alone > 0 {
            parts.push(format!("{left_alone} left alone"));
        }
        if taking > concurrency {
            parts.push(format!("{} past the bound", taking - concurrency));
        }
        parts
    }

    pub fn json(&self) -> Value {
        let did: Vec<Value> = self
            .pass
            .did
            .iter()
            .map(|(step, outcome)| {
                let ended = match outcome {
                    Outcome::Done | Outcome::HandedOn => "done",
                    Outcome::Nudged => "nudged",
                    Outcome::Continued => "continued",
                    Outcome::Restarted => "restarted",
                    Outcome::Abandoned => "abandoned",
                    Outcome::Nothing => "nothing",
                    Outcome::Lost => "lost",
                    Outcome::Failed => "failed",
                };
                let mut did = self.named(*step);
                did["outcome"] = json!(ended);
                if let TickStep::Advance { issue } = step
                    && let Some(stage) = self.advanced_to(*issue)
                {
                    did["stage"] = json!(stage);
                }
                if let Some(why) = self.escalated_by(*step, *outcome) {
                    did["escalated"] = json!(why);
                }
                did
            })
            .collect();
        let errors: Vec<Value> = self
            .failures
            .iter()
            .map(|failure| {
                let mut failed = self.named(failure.step);
                failed["error"] = json!(failure.error);
                failed
            })
            .collect();
        json!({"did": did, "errors": errors, "held": self.pass.held,
            "frontier_remaining": self.pass.frontier_remaining, "locks": self.set.locks})
    }

    /// One line for whoever watches the loop: what the tick did, and where the
    /// fleet stands.
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = vec![];
        if !self.set.locks.is_empty() {
            parts.push(format!("FLEET LOCK {} stands: only it is worked", named(&self.set.locks)));
        }
        let lead = parts.len();
        // An issue that left the fleet for a human says so, whichever step
        // handed it over: none of them names it as anything else.
        let escalated: Vec<String> =
            self.escalated.iter().map(|(issue, why)| format!("#{issue} ({why})")).collect();
        if !escalated.is_empty() {
            parts.push(format!("escalated {}", escalated.join(", ")));
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
                .map(|issue| match self.advanced_to(issue) {
                    Some(stage) if wanted == "advance" => format!("#{issue} → {stage}"),
                    _ => format!("#{issue}"),
                })
                .collect();
            if !issues.is_empty() {
                parts.push(format!("{verb} {}", issues.join(", ")));
            }
        }
        for (verb, kept) in [
            ("nudged", Outcome::Nudged),
            ("continued", Outcome::Continued),
            ("restarted", Outcome::Restarted),
        ] {
            let issues: Vec<String> = self
                .pass
                .did
                .iter()
                .filter(|(_, outcome)| *outcome == kept)
                .filter_map(|(step, _)| words(*step).1.map(|issue| format!("#{issue}")))
                .collect();
            if !issues.is_empty() {
                parts.push(format!("{verb} {}", issues.join(", ")));
            }
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
        for (verb, wanted) in [
            ("proposed as a precedent the ruling on", "propose"),
            ("ended the proposal of the ruling on", "end_proposal"),
        ] {
            let issues: Vec<String> = self
                .pass
                .did
                .iter()
                .filter(|(step, outcome)| words(*step).0 == wanted && *outcome == Outcome::Done)
                .filter_map(|(step, _)| self.proposal(*step).map(|issue| format!("#{issue}")))
                .collect();
            if !issues.is_empty() {
                parts.push(format!("{verb} {}", issues.join(", ")));
            }
        }
        for (outcome, said) in [
            (Outcome::Done, "tended the train"),
            (Outcome::Nudged, "nudged the train worker"),
            (Outcome::Abandoned, "abandoned the train"),
        ] {
            if self.pass.did.contains(&(TickStep::TendTrain, outcome)) {
                parts.push(said.to_string());
            }
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
            let (word, issue) = words(failure.step);
            let issue = issue.or(self.proposal(failure.step));
            let issue = issue.map(|issue| format!(" #{issue}")).unwrap_or_default();
            let patrol = self.patrol(failure.step).map(|p| format!(" {p}")).unwrap_or_default();
            parts.push(format!("FAILED {word}{issue}{patrol}: {}", failure.error));
        }
        parts.join(" · ")
    }
}

/// How `run` paces itself; the knobs exist for the tests.
pub struct Pacing {
    /// Stop after this many ticks [default: never].
    pub ticks: Option<u64>,
    /// How long one second of sleep really takes.
    pub second: Duration,
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

/// Sleeps for `sleep`, or until a wake, or until this run is told to stop.
fn rest(wakes: &Listener, stop: &Stop, sleep: Duration) {
    let mut left = sleep;
    while !stop.told() {
        let nap = left.min(HEARS_WITHIN);
        if wakes.rest(nap) || nap == left {
            return;
        }
        left -= nap;
    }
}

/// What a `run` told to stop says of its heartbeat as it goes (`goodbye`).
fn stop_line(me: &Me) -> String {
    match Repo::here().and_then(|repo| goodbye(&repo, me)) {
        Ok(true) => "holding no claim, it took its heartbeat with it".to_string(),
        Ok(false) => "it left its heartbeat with the claims it holds: the next start on this \
                      host takes them"
            .to_string(),
        Err(error) => format!("FAILED to take its heartbeat with it: {error:#}"),
    }
}

/// The log a `run` keeps of itself (`log::Log`). One that cannot be written
/// is said once and costs only what `doctor` could have read of it: no tick
/// waits for its log.
struct Said {
    log: Option<Log>,
}

impl Said {
    /// Opens the log and says in it that `me` started, with `agent`.
    fn open(me: &Me, agent: &Agent) -> Said {
        let log = Repo::here().and_then(|repo| Log::of(&repo));
        let mut said = Said { log: log.ok() };
        // Its first word alone: what follows may name what a human keeps to
        // themselves, and the command is written to no file.
        let first = agent.command.as_str().split_whitespace().next();
        let worker_command = first.unwrap_or_default().to_string();
        said.keep(me, me.now, log::What::Start { worker_command });
        said.log = said.log.filter(|log| log.file.exists());
        if said.log.is_none() {
            eprintln!(
                "warning: this run keeps no log of its ticks: `afkconvoy doctor` has none to read"
            );
        }
        said
    }

    fn keep(&self, me: &Me, at: u64, what: log::What) {
        let instance = me.instance.as_str().to_string();
        let entry = log::Entry { at, instance, host: me.host.clone(), what };
        if let Some(log) = &self.log {
            // Soft: the line is on stdout too, and the next one is tried again.
            let _ = log.append(&entry);
        }
    }
}

/// `afkconvoy run`: tick after tick, sleeping in between — a sleep a worker's
/// `afkconvoy wake` ends early, and that is skipped after a tick that left the
/// next one a worker to start (`Pass::follow_up_due`) or that failed on a read
/// GitHub could not be reached for. Told to stop — SIGTERM, SIGINT — it
/// finishes the step it is in and exits: no claim is released on the way out,
/// and the next start on this host takes them back (docs/adr/0003); holding
/// none, it removes its heartbeat. A start removes the heartbeats left by the
/// runs that could not (`at_start`). With a `panel_port` it serves the panel there, a new page
/// after every tick. It keeps a line of each tick in its log (`log::Log`),
/// which nothing here ever reads.
pub fn run(
    me: &mut Me,
    agent: &Agent,
    pacing: &Pacing,
    panel_port: Option<u16>,
    clock: impl Fn() -> u64,
) -> Result<Value> {
    let mut shown = Panel::empty(&Fleet::open(me.now)?, Some(me));
    let served = match panel_port {
        Some(port) => {
            let served = Served::open(port, panel::render(&shown))?;
            println!("panel: http://127.0.0.1:{}/", served.port);
            Some(served)
        }
        None => None,
    };
    let mut streak = 0;
    let mut skipped = 0;
    let mut count = 0;
    let mut started = false;
    let wakes = Listener::open(&Repo::here()?);
    let stop = Stop::listen();
    let said = Said::open(me, agent);
    loop {
        count += 1;
        me.now = clock();
        // Once: a start takes every claim stamped with this host (docs/adr/0003).
        if !started {
            match Fleet::open(me.now).and_then(|mut fleet| at_start(&mut fleet, me)) {
                Ok(took) => {
                    started = true;
                    if let Some(line) = start_line(&took) {
                        println!("start: {line}");
                    }
                }
                Err(error) => println!("start: FAILED to take this host's claims: {error:#}"),
            }
        }
        // A tick that could not even rebuild is a line, not the end of the loop.
        let ticked = turn(
            || wakes.forget(),
            || {
                let (ticked, fleet) = tick(me, agent, &stop)?;
                let panel = served.as_ref().map(|_| ticked.panel(&fleet, me));
                Ok::<_, anyhow::Error>((ticked, panel))
            },
        );
        let (line, sleep, logged) = match ticked {
            Ok((ticked, panel)) => {
                if let Some(panel) = panel {
                    shown = panel;
                }
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
        match sleep {
            0 => println!("tick {count}: {line} · next now"),
            _ => println!("tick {count}: {line} · next in {sleep}s"),
        }
        let (bound, answer, error) = logged;
        let tick = log::Tick { count, sleep, line: line.clone(), bound, answer, error };
        said.keep(me, clock(), log::What::Tick(tick));
        if let Some(served) = &served {
            // A tick that failed leaves the page as the last one found it.
            shown.tick = Some(panel::Tick { count, line, ended: clock(), sleep });
            served.show(panel::render(&shown));
        }
        if pacing.ticks.is_some_and(|ticks| count >= ticks) {
            return Ok(json!({"stopped": true, "ticks": count, "instance": me.instance.as_str()}));
        }
        rest(&wakes, &stop, pacing.second * sleep as u32);
        if stop.told() {
            println!("stop: {}", stop_line(me));
            return Ok(json!({"stopped": true, "ticks": count, "instance": me.instance.as_str()}));
        }
    }
}

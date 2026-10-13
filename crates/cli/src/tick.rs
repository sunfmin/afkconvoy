//! The tick's effect half: carries out the steps the core's pass hands it, and
//! the loop that runs one tick after another.

use std::time::Duration;

use afkconvoy_core::lock::named;
use afkconvoy_core::plan::WorkingSet;
use afkconvoy_core::takeover::run_said;
use afkconvoy_core::tick::{Outcome, Pass, TickStep, empty_streak, pace, run_pass, sleeps_skipped};
use afkconvoy_core::wake::turn;
use anyhow::Result;
use serde_json::{Value, json};

use crate::advance::{advance, hand_to_human};
use crate::board::show;
use crate::claims::{Me, beat};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::judge::judge;
use crate::log::{self, Log};
use crate::panel::{self, Panel};
use crate::patrol;
use crate::precedent;
use crate::serve::Served;
use crate::settle::settle;
use crate::stage::Flows;
use crate::takeover::{at_start, reclaim, unlock};
use crate::tend::tend_train;
use crate::wake::Listener;
use crate::watch::watch;

/// One step that failed, and why.
struct Failure {
    step: TickStep,
    error: String,
}

/// What one tick came to.
pub struct Ticked {
    pub pass: Pass,
    failures: Vec<Failure>,
    /// The claims the pass could hold (`WorkingSet::bound`).
    bound: usize,
    /// What its rebuild read: the pass was made over these.
    set: WorkingSet,
    flows: Flows,
}

/// `afkconvoy tick`: one reconciliation pass, start to finish. It never waits
/// for the workers it started, and leaves nothing on this host a later tick
/// needs: the next one rebuilds from GitHub and the remote.
pub fn tick(fleet: &mut Fleet, me: &Me, agent: &Agent) -> Result<Ticked> {
    let (set, flows) = fleet.rebuild(Some(&me.instance))?;
    let bound = set.bound(fleet.config.concurrency());
    let mut failures = vec![];
    let read = Read { set: &set, flows: &flows };
    let pass = run_pass(&set, bound, set.held(), |step| {
        // Whatever a step raised, the rest of the tick is still owed.
        carry_out(fleet, me, agent, &read, step).unwrap_or_else(|error| {
            failures.push(Failure { step, error: format!("{error:#}") });
            Outcome::Failed
        })
    });
    Ok(Ticked { pass, failures, bound, set, flows })
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
                Some(calling) => {
                    let rounds = read.flows.rounds(issue);
                    hand_to_human(fleet, me, issue, &calling.reason, rounds.as_deref())
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

    pub fn json(&self) -> Value {
        let did: Vec<Value> = self
            .pass
            .did
            .iter()
            .map(|(step, outcome)| {
                let outcome = match outcome {
                    Outcome::Done => "done",
                    Outcome::Nudged => "nudged",
                    Outcome::Continued => "continued",
                    Outcome::Restarted => "restarted",
                    Outcome::Abandoned => "abandoned",
                    Outcome::Nothing => "nothing",
                    Outcome::Lost => "lost",
                    Outcome::Failed => "failed",
                };
                let mut did = self.named(*step);
                did["outcome"] = json!(outcome);
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
        for (verb, wanted) in [
            ("settled", "settle"),
            ("ended", "judge"),
            ("escalated", "watch"),
            ("advanced", "advance"),
            ("handed to a human", "escalate"),
            ("unlocked", "unlock"),
            ("reclaimed", "reclaim"),
            ("dispatched", "dispatch"),
        ] {
            let issues: Vec<String> = self
                .pass
                .did
                .iter()
                .filter(|(step, outcome)| words(*step).0 == wanted && *outcome == Outcome::Done)
                .filter_map(|(step, _)| words(*step).1.map(|issue| format!("#{issue}")))
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
        parts.push(format!("holding {}/{}", self.pass.held, self.bound));
        parts.push(format!("frontier {}", self.pass.frontier_remaining));
        for failure in &self.failures {
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
    let taken = [issues("taken"), runs("taken_runs")].concat();
    let lost = [issues("lost"), runs("lost_runs")].concat();
    let mut parts = vec![];
    if !taken.is_empty() {
        parts.push(format!("took {} stamped with this host", taken.join(", ")));
    }
    if !lost.is_empty() {
        parts.push(format!("lost {} to a peer", lost.join(", ")));
    }
    if parts.is_empty() { None } else { Some(parts.join(" · ")) }
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
/// next one a worker to start (`Pass::follow_up_due`). Stopping it is just exiting: no claim is
/// released on the way out, and the next start on this host takes them back
/// (docs/adr/0003). With a `panel_port` it serves the panel there, a new page
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
                let mut fleet = Fleet::open(me.now)?;
                let ticked = tick(&mut fleet, me, agent)?;
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
                streak = empty_streak(streak, pass.did_work(), pass.held, pass.frontier_remaining);
                skipped = sleeps_skipped(skipped, pass.follow_up_due(ticked.bound));
                let sleep = if skipped > 0 { 0 } else { pace(pass.did_work(), pass.held, streak) };
                let logged = (Some(ticked.bound), Some(ticked.json()), None);
                (ticked.line(), sleep, logged)
            }
            Err(error) => {
                skipped = 0;
                let error = format!("{error:#}");
                (format!("FAILED: {error}"), pace(true, 0, 0), (None, None, Some(error)))
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
        wakes.rest(pacing.second * sleep as u32);
    }
}

//! The tick's effect half: carries out the steps the core's pass hands it, and
//! the loop that runs one tick after another.

use std::time::Duration;

use afkconvoy_core::tick::{Outcome, Pass, TickStep, empty_streak, pace, run_pass};
use anyhow::Result;
use serde_json::{Value, json};

use crate::claims::{Me, beat};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::judge::judge;
use crate::settle::settle;
use crate::takeover::{at_start, reclaim, unlock};
use crate::tend::tend_train;
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
    concurrency: usize,
}

/// `afkconvoy tick`: one reconciliation pass, start to finish. It never waits
/// for the workers it started, and leaves nothing on this host a later tick
/// needs: the next one rebuilds from GitHub and the remote.
pub fn tick(fleet: &mut Fleet, me: &Me, agent: &Agent) -> Result<Ticked> {
    let set = fleet.rebuild(Some(&me.instance))?;
    let concurrency = fleet.config.concurrency();
    let mut failures = vec![];
    let pass = run_pass(&set, concurrency, set.mine.len(), |step| {
        // Whatever a step raised, the rest of the tick is still owed.
        carry_out(fleet, me, agent, step).unwrap_or_else(|error| {
            failures.push(Failure { step, error: format!("{error:#}") });
            Outcome::Failed
        })
    });
    Ok(Ticked { pass, failures, concurrency })
}

fn carry_out(fleet: &mut Fleet, me: &Me, agent: &Agent, step: TickStep) -> Result<Outcome> {
    match step {
        TickStep::Settle { issue } => {
            settle(fleet, me, issue)?;
            Ok(Outcome::Done)
        }
        TickStep::TendTrain => tend_train(fleet, me, agent),
        TickStep::Judge { issue } => judge(fleet, me, agent, issue),
        TickStep::Watch { issue } => watch(fleet, me, agent, issue),
        TickStep::Unlock { issue } => unlock(fleet, me, issue),
        TickStep::Reclaim { issue } => reclaim(fleet, me, agent, issue),
        TickStep::Dispatch { issue } => {
            let started = dispatch(fleet, me, agent, issue, StartFrom::Auto)?;
            Ok(if started["started"] == true { Outcome::Done } else { Outcome::Lost })
        }
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
        TickStep::Unlock { issue } => ("unlock", Some(issue)),
        TickStep::Reclaim { issue } => ("reclaim", Some(issue)),
        TickStep::Dispatch { issue } => ("dispatch", Some(issue)),
        TickStep::Heartbeat => ("heartbeat", None),
    }
}

impl Ticked {
    pub fn json(&self) -> Value {
        let did: Vec<Value> = self
            .pass
            .did
            .iter()
            .map(|(step, outcome)| {
                let (word, issue) = words(*step);
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
                json!({"do": word, "issue": issue, "outcome": outcome})
            })
            .collect();
        let errors: Vec<Value> = self
            .failures
            .iter()
            .map(|failure| {
                let (word, issue) = words(failure.step);
                json!({"do": word, "issue": issue, "error": failure.error})
            })
            .collect();
        json!({"did": did, "errors": errors, "held": self.pass.held,
            "frontier_remaining": self.pass.frontier_remaining})
    }

    /// One line for whoever watches the loop: what the tick did, and where the
    /// fleet stands.
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = vec![];
        for (verb, wanted) in [
            ("settled", "settle"),
            ("ended", "judge"),
            ("escalated", "watch"),
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
        for (outcome, said) in [
            (Outcome::Done, "tended the train"),
            (Outcome::Nudged, "nudged the train worker"),
            (Outcome::Abandoned, "abandoned the train"),
        ] {
            if self.pass.did.contains(&(TickStep::TendTrain, outcome)) {
                parts.push(said.to_string());
            }
        }
        if parts.is_empty() {
            parts.push("nothing to do".to_string());
        }
        parts.push(format!("holding {}/{}", self.pass.held, self.concurrency));
        parts.push(format!("frontier {}", self.pass.frontier_remaining));
        for failure in &self.failures {
            let (word, issue) = words(failure.step);
            let issue = issue.map(|issue| format!(" #{issue}")).unwrap_or_default();
            parts.push(format!("FAILED {word}{issue}: {}", failure.error));
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
    let (taken, lost) = (issues("taken"), issues("lost"));
    let mut parts = vec![];
    if !taken.is_empty() {
        parts.push(format!("took {} stamped with this host", taken.join(", ")));
    }
    if !lost.is_empty() {
        parts.push(format!("lost {} to a peer", lost.join(", ")));
    }
    if parts.is_empty() { None } else { Some(parts.join(" · ")) }
}

/// `afkconvoy run`: tick after tick, sleeping in between. Stopping it is just
/// exiting: no claim is released on the way out, and the next start on this
/// host takes them back (docs/adr/0003).
pub fn run(me: &mut Me, agent: &Agent, pacing: &Pacing, clock: impl Fn() -> u64) -> Result<Value> {
    let mut streak = 0;
    let mut count = 0;
    let mut started = false;
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
        let (line, sleep) = match Fleet::open(me.now)
            .and_then(|mut fleet| tick(&mut fleet, me, agent))
        {
            Ok(ticked) => {
                let pass = &ticked.pass;
                streak = empty_streak(streak, pass.did_work(), pass.held, pass.frontier_remaining);
                (ticked.line(), pace(pass.did_work(), pass.held, streak))
            }
            Err(error) => (format!("FAILED: {error:#}"), pace(true, 0, 0)),
        };
        println!("tick {count}: {line} · next in {sleep}s");
        if pacing.ticks.is_some_and(|ticks| count >= ticks) {
            return Ok(json!({"stopped": true, "ticks": count, "instance": me.instance.as_str()}));
        }
        std::thread::sleep(pacing.second * sleep as u32);
    }
}

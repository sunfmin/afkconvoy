//! The tick's effect half: carries out the steps the core's pass hands it, and
//! the loop that runs one tick after another.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use afkconvoy_core::before::{Before, Lost};
use afkconvoy_core::host_sleep::{self, IDLE_WAIT, Standing};
use afkconvoy_core::hot::Limits;
use afkconvoy_core::lease::stamp_age;
use afkconvoy_core::lock::named;
use afkconvoy_core::plan::{Status, WorkingSet, handed_by};
use afkconvoy_core::takeover::run_said;
use afkconvoy_core::tick::{Outcome, Pass, TickStep, empty_streak, pace, run_pass, sleeps_skipped};
use afkconvoy_core::train::Built;
use afkconvoy_core::wake::turn;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::admission::Admission;
use crate::advance::{advance, hand_to_human, run_again_unclaimed};
use crate::board::show;
use crate::claims::{Me, beat, goodbye};
use crate::dispatch::{Agent, StartFrom, dispatch};
use crate::fleet::Fleet;
use crate::forget::forget;
use crate::gh::{Gone, unreached};
use crate::git::Repo;
use crate::host_sleep::{Checked, HostSleep};
use crate::hot::{self, Hot};
use crate::inspection::{self, Inspector};
use crate::judge::judge;
use crate::log::{self, Cause, Log};
use crate::measure;
use crate::opening;
use crate::panel::{self, Panel};
use crate::patrol;
use crate::precedent;
use crate::serve::{self, Live};
use crate::settle::settle;
use crate::slow_gate;
use crate::stage::Flows;
use crate::stop::Stop;
use crate::takeover::{at_start, reclaim, unlock};
use crate::tend::{cut_train, tend_train};
use crate::topic::{self, See};
use crate::wake::{During, Listener, Waker};
use crate::watch::watch;

/// What failed: a step, or the rebuild's read of one claimed issue.
#[derive(Clone, Copy)]
enum Failed {
    Step(TickStep),
    Read(u64),
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
fn waiting_since(since: u64, now: u64) -> String {
    let days = since / 86_400;
    let (hour, minute) = (since % 86_400 / 3600, since % 3600 / 60);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's `civil_from_days`).
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let of_era = shifted % 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    let ago = match stamp_age(since, now) {
        Some(ago) if ago >= 60 => format!("{}m ago", ago / 60),
        Some(ago) => format!("{ago}s ago"),
        None => "just now".to_string(),
    };
    format!("since {year}-{month:02}-{day:02} {hour:02}:{minute:02} UTC, {ago}")
}

/// What one tick came to.
pub struct Ticked {
    pub pass: Pass,
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
    /// The closed claimed issues, each with who closed it (`Fleet::closers`).
    closers: Vec<(u64, String)>,
    /// What the train's worktree held when the pass cut it again
    /// (`Fleet::cut_train`).
    cut_train: Option<Built>,
    /// The open issues, each with the label that hands it to the fleet
    /// (`plan::handed_by`).
    handed: Vec<(u64, String)>,
    /// What this tick saw as it ended, but for its count: the next one's
    /// `before`.
    saw: Before,
    /// The config's `concurrency`.
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

/// `afkconvoy tick`: one reconciliation pass, start to finish. It never waits
/// for the workers it started, and leaves nothing on this host a later tick
/// needs: the next one rebuilds from GitHub and the remote. Told to `stop`, it
/// finishes the step it is in and makes none of those left, which fail. What
/// the tick `before` saw, when there was one, words its line and decides
/// nothing.
pub fn tick(
    me: &Me,
    agent: &Agent,
    stop: &Stop,
    before: Option<&Before>,
) -> Result<(Ticked, Fleet)> {
    let (mut fleet, mut open) = Fleet::read(me.now)?;
    let (mut set, mut flows) = fleet.rebuild(&open, Some(&me.instance))?;
    // What no consult saw goes to one first (docs/adr/0018), and is read
    // again as the fleet's. A team that does not load puts nothing: the
    // rebuild says why of each issue.
    let put = crate::stage::put_to_consults(&fleet, &flows, &open.issues).unwrap_or_default();
    if !put.is_empty() {
        (fleet, open) = Fleet::read(me.now)?;
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
    let mut blocked = vec![];
    let read = Read { set: &set, flows: &flows };
    let pass = run_pass(&set, bound, set.held(), |step| {
        let made = match stop.told() {
            true => Err(anyhow!("this run was told to stop before it made the step")),
            false => carry_out(&mut fleet, (me, agent), &read, step, &mut blocked),
        };
        // Whatever a step raised, the rest of the tick is still owed.
        made.unwrap_or_else(|error| {
            failures.push(Failure::of(Failed::Step(step), &error));
            Outcome::Failed
        })
    });
    let escalated = std::mem::take(&mut fleet.escalated);
    let forgotten = std::mem::take(&mut fleet.forgotten);
    let closers = std::mem::take(&mut fleet.closers);
    let cut_train = fleet.cut_train.take();
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
        failures,
        escalated,
        blocked,
        forgotten,
        before: before.cloned(),
        lost,
        closers,
        cut_train,
        handed,
        saw,
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
    match step {
        TickStep::Settle { issue } => {
            settle(fleet, me, issue)?;
            Ok(Outcome::Done)
        }
        TickStep::Forget { issue } => forget(fleet, me, issue),
        TickStep::TendTrain => tend_train(fleet, me, agent),
        TickStep::CutTrain => match read.set.outgrown_train {
            Some(built) => cut_train(fleet, built),
            None => Ok(Outcome::Nothing),
        },
        TickStep::Judge { issue, start } => judge(fleet, me, agent, issue, start),
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
            if started["gone"].is_string() {
                return Ok(Outcome::Gone);
            }
            if let Some(blockers) = started["blocked_by"].as_array() {
                blocked.push((issue, blockers.iter().filter_map(Value::as_u64).collect()));
                return Ok(Outcome::Blocked);
            }
            Ok(if started["started"] == true { Outcome::Done } else { Outcome::Lost })
        }
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
        TickStep::TendTrain => ("tend_train", None),
        TickStep::CutTrain => ("cut_train", None),
        TickStep::Judge { issue, .. } => ("judge", Some(issue)),
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
        let mut named = json!({"do": word, "issue": issue.or(self.proposal(step))});
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
        }
    }

    /// What failed, as the line names it: `settle #3`, `patrol triage`.
    fn failed_said(&self, failed: Failed) -> String {
        let step = match failed {
            Failed::Step(step) => step,
            Failed::Read(issue) => return format!("read #{issue}"),
        };
        let (word, issue) = words(step);
        let issue = issue.or(self.proposal(step));
        let issue = issue.map(|issue| format!(" #{issue}")).unwrap_or_default();
        let patrol = self.patrol(step).map(|p| format!(" {p}")).unwrap_or_default();
        format!("{word}{issue}{patrol}")
    }

    /// The panel of `me` after this tick: what its rebuild read, with what the
    /// pass did since.
    pub fn panel(&self, fleet: &Fleet, me: &Me) -> Panel {
        let mut panel = panel::read(fleet, Some(me), &self.set, &self.flows, &self.pass.did);
        panel.declared = self.declared().min(self.pass.held);
        panel.held = self.pass.held - panel.declared;
        panel.bound = self.bound - panel.declared;
        panel
    }

    /// The claims whose worker declared a verdict that the pass left held:
    /// what its bound was raised by and still is (`Pass::bound`).
    fn declared(&self) -> usize {
        self.bound.saturating_sub(self.concurrency.saturating_add(self.set.left_alone()))
    }

    /// Whether this instance is idle after the tick, and whether it worked in
    /// it: held a claim — as its rebuild read the claims, or as its pass ended
    /// — or had something on the landing train.
    pub fn idle(&self) -> (bool, bool) {
        let on_train = self.set.mine.iter().any(|mine| mine.status == Status::Joined);
        let pass = &self.pass;
        let idle = host_sleep::idle(pass.held, pass.frontier_remaining, on_train);
        (idle, host_sleep::worked(self.set.held().max(pass.held), on_train))
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
            parts.push(format!("{declared} declared"));
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
                    Outcome::Waiting(_) => "waiting",
                    Outcome::Continued => "continued",
                    Outcome::Restarted => "restarted",
                    Outcome::Abandoned => "abandoned",
                    Outcome::Nothing => "nothing",
                    Outcome::Lost => "lost",
                    Outcome::Blocked => "blocked",
                    Outcome::Gone => "gone",
                    Outcome::Failed => "failed",
                };
                let mut did = self.named(*step);
                did["outcome"] = json!(ended);
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
                if let (TickStep::Dispatch { issue }, Outcome::Blocked) = (step, outcome)
                    && let Some(blockers) = self.blocked_by(*issue)
                {
                    did["blocked_by"] = json!(blockers);
                }
                did
            })
            .collect();
        let errors: Vec<Value> = self
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
        let mut answer = json!({"did": did, "errors": errors, "held": self.pass.held,
            "frontier_remaining": self.pass.frontier_remaining, "locks": self.set.locks});
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
        parts.extend(self.lost.iter().map(Lost::said));
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
        // A worker left as it waits on its own background work, and since when:
        // it is not silent, and the line says why it was not nudged.
        let waiting: Vec<String> = self
            .pass
            .did
            .iter()
            .filter_map(|(step, outcome)| match outcome {
                Outcome::Waiting(since) => {
                    let since = waiting_since(*since, self.now);
                    Some(match words(*step).1 {
                        Some(issue) => format!("#{issue} ({since})"),
                        None => format!("the train worker ({since})"),
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
        if let Some(cut) = self.cut_train {
            let fresh = match cut.fresh {
                Some(fresh) => format!("its fresh build {}", measure::said(fresh)),
                None => "no fresh build on record".to_string(),
            };
            parts.push(format!(
                "cut the train's worktree again (built {}, {fresh})",
                measure::said(cut.built)
            ));
        }
        if self.pass.did.contains(&(TickStep::SlowGate, Outcome::Done)) {
            parts.push("opened an issue for the slow gate".to_string());
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

/// How `run` paces itself; the knobs exist for the tests.
pub struct Pacing {
    /// Stop after this many ticks [default: never].
    pub ticks: Option<u64>,
    /// How long one second of sleep really takes.
    pub second: Duration,
    /// Whether it was told `--no-host-sleep`.
    pub no_host_sleep: bool,
    /// How long, in seconds, every fleet on this host must have been idle
    /// before it is put to sleep [default: `IDLE_WAIT`].
    pub host_sleep_after: u64,
    /// What a worker's processes may take before it turns hot.
    pub hot: Limits,
}

/// What a `run` says at its start of host sleep, when it puts its host to
/// sleep: after how long, and how not to.
fn host_sleep_line(pacing: &Pacing) -> Option<String> {
    let after = match pacing.host_sleep_after {
        IDLE_WAIT => "ten minutes".to_string(),
        seconds => format!("{seconds}s"),
    };
    (!pacing.no_host_sleep).then(|| {
        format!(
            "start: puts this host to sleep once every fleet on it has been idle {after} \
             (`--no-host-sleep` keeps it awake)"
        )
    })
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

/// The longest, in seconds, a `run` sleeps while its inspection runs or a
/// hot worker waits for one: an inspection's time limit is kept to within it.
const INSPECTING_EVERY: u64 = 30;

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
fn rest(wakes: &Listener, stop: &Stop, sleep: Duration, (said, clock): (&Said, &Clock)) -> Rested {
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

/// The clock a `run` reads, in epoch seconds: shared with the thread that
/// hears its wakes.
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The log a `run` keeps of itself (`log::Log`), and its stdout: each line is
/// printed as it is written, one at a time, whichever thread says it, so the
/// two hold the same lines in the same order. A log that cannot be written is
/// said once and costs only what `doctor` could have read of it: no tick
/// waits for its log.
struct Said {
    log: Option<Log>,
    instance: String,
    host: String,
    /// Held while a line is printed and written.
    one: Mutex<()>,
}

impl Said {
    /// Opens the log and says in it that `me` started, with `agent`.
    fn open(me: &Me, agent: &Agent) -> Said {
        let log = Repo::here().and_then(|repo| Log::of(&repo));
        let instance = me.instance.as_str().to_string();
        let mut said =
            Said { log: log.ok(), instance, host: me.host.clone(), one: Mutex::default() };
        // First words alone (`Launches::logged`): a command is written to no file.
        let (worker_command, harness_commands) = agent.launches.logged();
        said.keep(me.now, log::What::Start { worker_command, harness_commands });
        said.log = said.log.filter(|log| log.file.exists());
        if said.log.is_none() {
            eprintln!(
                "warning: this run keeps no log of its ticks: `afkconvoy doctor` has none to read"
            );
        }
        said
    }

    fn keep(&self, at: u64, what: log::What) {
        let (instance, host) = (self.instance.clone(), self.host.clone());
        let entry = log::Entry { at, instance, host, what };
        let _one = self.one.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        println!("{}", entry.said());
        if let Some(log) = &self.log {
            // Soft: the line is on stdout too, and the next one is tried again.
            let _ = log.keep(&entry);
        }
    }
}

/// Makes the check for host sleep that is due, if one is (`HostSleep::check`),
/// and says how it ended → whether the next tick is due now, the check waiting on it.
fn check_host(host: &mut HostSleep, said: &Said, stop: &Stop, clock: &Clock) -> bool {
    let Some(checked) = host.check(clock(), clock.as_ref(), stop) else { return false };
    let (slept, awake) = match checked {
        Checked::TickNow => return true,
        Checked::Stopped => return false,
        Checked::Awake(awake) => (false, awake.iter().map(|awake| awake.said()).collect()),
        Checked::Slept => (true, vec![]),
        Checked::Failed(error) => {
            (false, vec![format!("the host could not be put to sleep: {error}")])
        }
    };
    said.keep(clock(), log::What::HostSleep { slept, awake });
    false
}

/// `afkconvoy run`: tick after tick, sleeping in between — a sleep a worker's
/// `afkconvoy wake` ends early, and that is skipped after a tick that left the
/// next one a worker to start (`Pass::follow_up_due`) or that failed on a read
/// GitHub could not be reached for. Told to stop — SIGTERM, SIGINT — it
/// finishes the step it is in and exits: no claim is released on the way out,
/// and the next start on this host takes them back (docs/adr/0003); holding
/// none, it removes its heartbeat. A start removes the heartbeats left by the
/// runs that could not (`at_start`). It serves the panel once that is opened — at the start,
/// on a `panel_port`, or later by `afkconvoy panel --open` (`opening`) — a new page after
/// every tick. It keeps a line in its log (`log::Log`) as each tick starts,
/// with why, and as it ends, and one of each event that reaches it — a wake
/// heard during a tick, said as it is heard — and prints each as it is
/// written (`Said`); nothing here ever reads the log. It watches what each
/// worker's processes take, and says as one turns hot and as it stops being
/// hot (`hot::watch`, docs/adr/0019); after each tick its inspector ends the
/// inspection that is over and starts the next a hot worker waits for, and
/// it looks again within `INSPECTING_EVERY` while either is so
/// (`Inspector`). Unless told `--no-host-sleep`, it puts the
/// host to sleep once every fleet on it has been idle long enough
/// (`HostSleep`, docs/adr/0016).
pub fn run(
    me: &mut Me,
    agent: &Agent,
    pacing: &Pacing,
    panel_port: Option<u16>,
    clock: impl Fn() -> u64 + Send + Sync + 'static,
) -> Result<Value> {
    let clock: Clock = Arc::new(clock);
    let mut shown = Panel::empty(&Fleet::open(me.now)?, Some(me));
    let live = Live::new(panel::render(&shown));
    if let Some(port) = panel_port {
        println!("panel: {}", serve::address(live.open(port)?.0));
    }
    let mut streak = 0;
    let mut skipped = 0;
    let mut count = 0;
    let mut started = false;
    // What the last tick that rebuilt saw: the next one's line names what
    // changed outside this host since.
    let mut before: Option<Before> = None;
    let repo = Repo::here()?;
    let said = Arc::new(Said::open(me, agent));
    let during: During = {
        let (said, clock) = (Arc::clone(&said), Arc::clone(&clock));
        Box::new(move |during, by| said.keep(clock(), log::What::WokenDuring { during, by }))
    };
    let wakes = Listener::open(&repo, during);
    let hot = Hot::default();
    let mut inspector = Inspector::new(pacing.hot);
    {
        let said = Arc::clone(&said);
        let keep = move |at, what| said.keep(at, what);
        hot::watch(&repo, pacing.hot, pacing.second, Arc::clone(&clock), keep, Arc::clone(&hot));
    }
    opening::listen(&repo, live.clone(), me.instance.as_str());
    let stop = Stop::listen();
    let mut cause = Cause::Start;
    let mut by = None;
    if let Some(line) = host_sleep_line(pacing) {
        println!("{line}");
    }
    let standing = Standing {
        instance: me.instance.as_str().to_string(),
        repo: repo.main_checkout().map(|main| main.display().to_string()).unwrap_or_default(),
        version: crate::VERSION.to_string(),
        no_host_sleep: pacing.no_host_sleep,
        idle: false,
        since: me.now,
        worked: None,
    };
    let mut host = HostSleep::open(standing, Waker::of(&repo), pacing.host_sleep_after);
    loop {
        count += 1;
        said.keep(clock(), log::What::TickStarted { count, cause, by: by.take() });
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
        // Asked once a turn: a panel opened while the tick runs is the next
        // tick's to fill, which the wake its opening sends starts at once.
        let served = live.served();
        // A tick that could not even rebuild is a line, not the end of the loop.
        let ticked = turn(
            || wakes.forget(count),
            || {
                let (ticked, fleet) = tick(me, agent, &stop, before.as_ref())?;
                let panel = served.then(|| ticked.panel(&fleet, me));
                Ok::<_, anyhow::Error>((ticked, panel))
            },
        );
        wakes.ended();
        let idle = ticked.as_ref().ok().map(|(ticked, _)| ticked.idle());
        host.ticked(clock(), idle);
        let (line, sleep, logged) = match ticked {
            Ok((ticked, panel)) => {
                if let Some(panel) = panel {
                    shown = panel;
                }
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
        for what in inspector.step(agent, &hot_now, clock()) {
            said.keep(clock(), what);
        }
        let sleep = if inspector.busy(&hot_now) { sleep.min(INSPECTING_EVERY) } else { sleep };
        let (bound, answer, error) = logged;
        let tick = log::Tick { count, sleep, line: line.clone(), bound, answer, error };
        said.keep(clock(), log::What::Tick(tick));
        if served {
            // A tick that failed leaves the page as the last one found it.
            shown.tick = Some(panel::Tick { count, line, ended: clock(), sleep });
            shown.hot = hot_now;
            shown.inspection = inspection::on_host();
            live.show(panel::render(&shown));
        }
        let mut due_now = check_host(&mut host, &said, &stop, &clock);
        if pacing.ticks.is_some_and(|ticks| count >= ticks) {
            return Ok(json!({"stopped": true, "ticks": count, "instance": me.instance.as_str()}));
        }
        // A check that falls due while it sleeps is made then, whether a
        // tick is due or not.
        let mut left = pacing.second * sleep as u32;
        cause = if sleep == 0 { Cause::FollowUp } else { Cause::Timer };
        while !due_now {
            let until = host.due().map(|due| Duration::from_secs(due.saturating_sub(clock())));
            let nap = until.map_or(left, |until| until.min(left));
            let rested = rest(&wakes, &stop, nap, (&said, &clock));
            let slept = rested == Rested::Slept;
            if let Rested::Woken(woken) = rested
                && sleep > 0
            {
                (cause, by) = (Cause::Woken, woken);
            }
            if !slept || nap == left {
                break;
            }
            left -= nap;
            due_now = check_host(&mut host, &said, &stop, &clock);
        }
        if stop.told() {
            said.keep(clock(), log::What::Stop { line: stop_line(me) });
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
}

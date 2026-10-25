//! Inspections, as `run` starts and ends them (docs/adr/0026): once what
//! shows a worker hot is taken down (`evidence`), a worker in the role of its
//! stage's consult — the stock consult where the stage names none, and in an
//! unbound repo — is started in a checkout of its own as the operator on this
//! host, so that the fleet's work goes on: it finds what holds the work back,
//! fixes it, and says what it did with `afkconvoy inspected`. The fleet logs
//! each thing it did, leaves it on the issue, and ends it. Who inspects,
//! which hot worker is next and what it is told are the core's
//! (`afkconvoy_core::inspection`); what holds it to its bounds is the guard's.
//!
//! When admission has held back new workers for one reason past its span, the
//! host is inspected the same way (`afkconvoy_core::host_inspection`), once an
//! episode: the stock consult, given what holds the disk, the load or the
//! memory (`host_evidence`). What it did is kept in the cache every fleet of
//! the host shares, for `doctor` and the panel to show beside admission's
//! reading. No inspection opens an issue, nor hands anything to a human: a
//! change of the repo it makes itself, and puts on the landing train with
//! `afkconvoy land --inspection` (`land`), on no issue.
//!
//! One inspection runs on the host at a time, of either kind, across its
//! fleets: the host's turn to inspect is held by its host runner (`Turn`),
//! which gives it to the fleet instance whose inspection runs, and to an
//! inspection of the host only while no fleet instance has a hot worker
//! waiting for one. The turn goes with the inspection that holds it — ended,
//! or dropped with a fleet instance that ends. An inspection takes no slot of
//! the concurrency.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use afkconvoy_core::config::{Bound, Config};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::host_inspection::{self, Due, Reason, Trigger, Watching, covered};
use afkconvoy_core::inspection::{
    self, CHECKOUT, Fields, Finding, Hot, Inspected, Inspector as Who, Kind, Operator, SAID,
    inspector, over,
};
use afkconvoy_core::role::{Facts, On, Start, workflow_of};
use afkconvoy_core::stage_record::history;
use afkconvoy_core::stock;
use afkconvoy_core::train::Of;
use afkconvoy_core::worker::WorkerState;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::admission::Admission;
use crate::copies::{Copies, Copy, cache};
use crate::dispatch::Agent;
use crate::evidence;
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::host_evidence::{self, Own};
use crate::hot::{Hot as HotWorkers, HotNow, held_said};
use crate::log::What;
use crate::orca::{Source, Worktree};
use crate::role::ToldRules;
use crate::stage::records_of;
use crate::team::keep;

/// What the last host inspection did, in the cache every fleet of the host
/// shares.
const LAST: &str = "host-inspection/last.json";

/// macOS's pressure signal is independent of admission's available-memory
/// share. A host that cannot report it is under no observed pressure.
fn pressure_level() -> Option<u64> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    crate::host_sleep::output("sysctl", &["-n", "kern.memorystatus_vm_pressure_level"])
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn under_pressure() -> bool {
    pressure_level().is_some_and(|level| level >= 2)
}

/// A current snapshot, taken after any previous inspection was ended.
fn hot_now(hot: &HotWorkers) -> Vec<HotNow> {
    hot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

fn still_hot(hot: &HotWorkers, episode: &HotNow) -> bool {
    hot_now(hot)
        .iter()
        .any(|now| now.path == episode.path && now.episode.since == episode.episode.since)
}

/// What a host inspection's turn names as the worker it inspects.
pub const THE_HOST: &str = "the host";

/// An inspection running on the host, as its turn says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    /// The main checkout of the repo whose fleet runs it.
    pub repo: String,
    /// The hot worker it inspects, and the issue that worker is on.
    pub worker: String,
    pub issue: Option<u64>,
    /// The role it was started in.
    pub by: String,
    /// When it started, in epoch seconds.
    pub since: u64,
    /// Of a host inspection: what admission held back for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_back_by: Option<Reason>,
}

/// The host's one turn to inspect, held by its host runner for every fleet
/// instance it holds: the inspection running, if one is, and the fleet
/// instances with a hot worker waiting for one, which go before an
/// inspection of the host. A clone is the same.
#[derive(Clone, Default)]
pub struct Turn {
    held: Arc<Mutex<Turns>>,
}

#[derive(Default)]
struct Turns {
    running: Option<Running>,
    /// The places of the inspectors with a hot worker waiting, by key.
    waiting: BTreeSet<u64>,
    /// The key the next place takes.
    next: u64,
}

impl Turn {
    fn held(&self) -> MutexGuard<'_, Turns> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The inspection running on the host now, across its fleets.
    pub fn running(&self) -> Option<Running> {
        self.held().running.clone()
    }

    /// Whether the turn can be taken now for an inspection of the host,
    /// `of_host`, or of a hot worker: not while another inspection holds it,
    /// nor — of the host — while a hot worker of any fleet instance waits for
    /// one.
    fn free(&self, of_host: bool) -> bool {
        Turn::free_in(&self.held(), of_host)
    }

    fn free_in(held: &Turns, of_host: bool) -> bool {
        held.running.is_none() && !(of_host && !held.waiting.is_empty())
    }

    /// Takes the turn for `running` → it, held until it is dropped; `None`
    /// where it is not free (`free`).
    fn take(&self, running: Running) -> Option<Taken> {
        let mut held = self.held();
        if !Turn::free_in(&held, running.held_back_by.is_some()) {
            return None;
        }
        held.running = Some(running);
        Some(Taken { turn: self.clone() })
    }

    /// A place at the turn for one fleet instance's inspector.
    fn place(&self) -> Place {
        let mut held = self.held();
        let key = held.next;
        held.next += 1;
        Place { turn: self.clone(), key }
    }
}

/// The host's turn, taken: it is free again once this is dropped.
struct Taken {
    turn: Turn,
}

impl Drop for Taken {
    fn drop(&mut self) {
        self.turn.held().running = None;
    }
}

/// One inspector's place at the host's turn: whether a hot worker of its
/// waits for an inspection. It waits no more once this is dropped.
struct Place {
    turn: Turn,
    key: u64,
}

impl Place {
    /// Says whether a hot worker of this inspector's waits for an inspection.
    fn waits(&self, waiting: bool) {
        let mut held = self.turn.held();
        match waiting {
            true => held.waiting.insert(self.key),
            false => held.waiting.remove(&self.key),
        };
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.waits(false);
    }
}

/// The inspection running on this host now, across its fleets, as its host
/// runner says it; none where none runs, or no host runner can say.
pub fn on_host() -> Option<Running> {
    crate::host_runner::shared().ok().flatten()?.inspection
}

/// `running`, as `doctor` and the panel show it.
pub fn json(running: &Running) -> Value {
    let kind = if running.held_back_by.is_some() { Kind::Host } else { Kind::Worker };
    json!({"kind": kind.word(), "repo": running.repo, "worker": running.worker,
        "issue": running.issue, "by": running.by, "since": running.since,
        "held_back_by": running.held_back_by.map(Reason::word)})
}

/// What the last host inspection on this host did, whichever of its fleets
/// started it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Last {
    /// The main checkout of the repo whose fleet started it.
    pub repo: String,
    pub reason: Reason,
    /// What admission read as it started.
    pub reading: String,
    pub by: String,
    pub started: u64,
    pub ended: u64,
    /// How it ended: `said` where it said what it did, `nothing` where it
    /// said nothing in time.
    pub found: String,
    /// What held the work back, in its words.
    pub said: String,
    /// Each thing it did, and the PRs it put on the landing train.
    #[serde(default)]
    pub did: Vec<String>,
    #[serde(default)]
    pub prs: Vec<u64>,
}

impl Last {
    /// What it said and did, and its PRs on the landing train, in one line,
    /// as the log keeps it.
    pub fn line(&self) -> String {
        Finding { said: self.said.clone(), did: self.did.clone(), prs: self.prs.clone() }.line()
    }
}

/// What the last host inspection on this host did; none where none ended.
pub fn last_on_host() -> Option<Last> {
    last_inspections().pop()
}

/// One completed inspection per reason, newest last. Read the former single
/// record too, so an upgrade keeps its coverage of a current episode.
fn last_inspections() -> Vec<Last> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Kept {
        One(Last),
        Each(Vec<Last>),
    }
    let read = || -> Option<Kept> {
        let said = std::fs::read_to_string(cache().ok()?.join(LAST)).ok()?;
        serde_json::from_str(&said).ok()
    };
    match read() {
        Some(Kept::One(last)) => vec![last],
        Some(Kept::Each(last)) => last,
        None => vec![],
    }
}

fn last_for(reason: Reason) -> Option<(Reason, u64)> {
    last_inspections()
        .iter()
        .rev()
        .find(|last| last.reason == reason)
        .map(|last| (last.reason, last.started))
}

/// Keeps `last` as what the last host inspection on this host found.
fn keep_last(last: &Last) -> Result<()> {
    let file = cache()?.join(LAST);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot make {}", dir.display()))?;
    }
    // A load or disk inspection must not erase a memory inspection's
    // coverage while a different fleet still observes that pressure episode.
    let mut kept = last_inspections();
    kept.retain(|old| old.reason != last.reason);
    kept.push(last.clone());
    std::fs::write(&file, serde_json::to_vec_pretty(&kept)?)
        .with_context(|| format!("cannot write {}", file.display()))
}

/// `last`, as `doctor` shows it beside admission's reading.
pub fn last_json(last: &Last) -> Value {
    serde_json::to_value(last).unwrap_or(Value::Null)
}

/// What an inspection this `run` started is of.
enum Subject {
    /// A hot worker, and `why` it turned hot.
    Worker { hot: HotNow, why: String },
    /// The host, on which admission held back for `reason`, having read
    /// `reading`.
    Host { reason: Reason, reading: String },
}

/// An inspection this `run` started, while it runs.
struct Inspection {
    on: Subject,
    checkout: Worktree,
    started: u64,
    by: String,
    /// The host's turn, held until it ends.
    _turn: Taken,
}

/// What a start came to.
enum Started {
    Running(Box<Inspection>),
    /// Nobody inspects the worker, for this reason.
    Nobody(String),
    /// Another inspection holds the host's turn.
    Waits,
}

/// Who an inspection is started as, and how.
enum As {
    /// The stock consult of an unbound repo, started with the worker launch
    /// command of the harness the host kit names for it, on its model: its
    /// instruction is in its brief, and nothing guards it.
    Unbound { harness: Harness, model: Option<String> },
    /// A role's consult, or the stock consult, of a bound repo: started in
    /// its role, guarded as a consult.
    Bound {
        harness: Harness,
        /// The stock consult's model, as the host kit names it: none of a
        /// role's consult.
        model: Option<String>,
        role: String,
        skill: String,
        role_copy: Copy,
        team_copy: Copy,
        team_rules: Option<String>,
        host_user: bool,
        dsh_profile: afkconvoy_core::team::DshProfile,
        escalate_label: String,
    },
}

impl As {
    fn role(&self) -> &str {
        match self {
            As::Unbound { .. } => stock::ROLE,
            As::Bound { role, .. } => role,
        }
    }

    fn harness(&self) -> Harness {
        match self {
            As::Unbound { harness, .. } | As::Bound { harness, .. } => *harness,
        }
    }
}

/// The stock consult of an unbound repo, on what the host kit names for it.
fn unbound_as(fleet: &Fleet) -> Result<As> {
    let stock = fleet.stock_consult()?;
    Ok(As::Unbound { harness: stock.harness, model: stock.model })
}

/// The stock consult of the repo `bound` to a team, which inspects the host:
/// on what the host kit names for it.
fn stock_as(fleet: &Fleet, bound: &Bound) -> Result<As> {
    let copies = Copies::of_host()?;
    let kept = keep(&fleet.gh, &copies, &bound.team, &fleet.config.labels())?;
    let stock = fleet.stock_consult()?;
    Ok(As::Bound {
        harness: stock.harness,
        model: stock.model,
        role: stock::ROLE.to_string(),
        skill: stock::SKILL.to_string(),
        role_copy: copies.stock()?,
        team_copy: kept.copy.clone(),
        team_rules: kept.rules(),
        host_user: kept.team.host_user,
        dsh_profile: kept.team.dsh_profile,
        escalate_label: kept.team.escalate_label.clone(),
    })
}

/// Who inspects the hot worker on `issue` — none of a patrol's — in a repo
/// `bound` to a team: its stage's consult, as the issue's records have it now
/// (`inspection::inspector`). `None`: nobody does.
fn bound_as(fleet: &Fleet, bound: &Bound, issue: Option<u64>) -> Result<Option<As>> {
    let copies = Copies::of_host()?;
    let kept = keep(&fleet.gh, &copies, &bound.team, &fleet.config.labels())?;
    let read = match issue {
        Some(issue) => Some((fleet.gh.issue(issue)?.labels, records_of(&fleet.gh, issue)?.0)),
        None => None,
    };
    let workflow = read
        .as_ref()
        .and_then(|(labels, records)| Some((workflow_of(&kept.team, labels).ok()?, records)));
    let place = workflow.map(|(workflow, records)| history(workflow, records).place);
    let stage = place.as_ref().and_then(|place| place.position.stage());
    let (harness, model, role, skill, role_copy, host_user, dsh_profile) = match inspector(stage) {
        None => return Ok(None),
        // A consult a stage names runs its role's range's first.
        Some(Who::Role(work)) => {
            let role = kept.team.role_of(work);
            let cast = fleet.cast_in(role, &[])?;
            let copy = kept.role(role, cast.harness)?;
            (
                cast.harness,
                cast.model,
                role.name.clone(),
                work.skill.clone(),
                copy,
                role.host_user,
                role.dsh_profile,
            )
        }
        // The stock consult runs what the host kit names for it.
        Some(Who::Stock) => {
            let stock = fleet.stock_consult()?;
            (
                stock.harness,
                stock.model,
                stock::ROLE.to_string(),
                stock::SKILL.to_string(),
                copies.stock()?,
                kept.team.host_user,
                kept.team.dsh_profile,
            )
        }
    };
    Ok(Some(As::Bound {
        harness,
        model,
        role,
        skill,
        role_copy,
        team_copy: kept.copy.clone(),
        team_rules: kept.rules(),
        host_user,
        dsh_profile,
        escalate_label: kept.team.escalate_label.clone(),
    }))
}

/// What a `run` keeps of the inspections it starts.
pub struct Inspector {
    /// The checkout of the fleet whose `run` it is.
    checkout: Repo,
    /// Its place at the host's turn to inspect.
    place: Place,
    inspected: Inspected,
    running: Option<Box<Inspection>>,
    /// Admission, tick after tick: the host is inspected once it held back
    /// for one reason past `span`.
    watching: Watching,
    span: u64,
    /// The main checkout of the repo, and each of its issues with its stage,
    /// as the last tick saw them: what a host inspection lists its worktrees
    /// with.
    repo: PathBuf,
    stages: Vec<(u64, String)>,
}

impl Inspector {
    /// The inspector of the `run` of the checkout `checkout`, inspecting in
    /// turn with every other of the host's, `turn`.
    pub fn new(checkout: &Repo, turn: &Turn) -> Inspector {
        Inspector {
            checkout: checkout.clone(),
            place: turn.place(),
            inspected: Inspected::default(),
            running: None,
            watching: Watching::default(),
            span: host_inspection::SPAN,
            repo: PathBuf::new(),
            stages: vec![],
        }
    }

    /// What a tick at `now` found: `admission` holding back or admitting, in
    /// the repo whose main checkout is `repo`, its issues at `stages`.
    pub fn ticked(
        &mut self,
        now: u64,
        admission: &Admission,
        repo: PathBuf,
        stages: Vec<(u64, String)>,
    ) {
        self.watching.ticked(now, &admission.reasons(), under_pressure());
        self.span = admission.span;
        (self.repo, self.stages) = (repo, stages);
    }

    /// The host inspection due at `now`, if one is.
    fn host_due(&self, now: u64) -> Option<(Trigger, Due)> {
        self.watching.due(now, self.span)
    }

    /// The hot workers `hot`, as the core's rules see them.
    fn seen(hot: &[HotNow]) -> Vec<Hot> {
        let seen = hot.iter().map(|hot| Hot {
            worker: hot.path.display().to_string(),
            since: hot.episode.since,
            evidence: evidence::of(&hot.worktree(), &hot.worker, hot.episode.since).is_some(),
        });
        seen.collect()
    }

    /// Whether it is to look again soon: its inspection runs, or a hot worker
    /// of `hot` waits for one.
    pub fn busy(&self, hot: &[HotNow], now: u64) -> bool {
        self.running.is_some()
            || self.inspected.next(&Inspector::seen(hot)).is_some()
            || self.host_due(now).is_some()
    }

    /// One look, at `now`, with `hot` the workers hot now: ends the
    /// inspection that is over, and starts the next a hot worker waits for —
    /// or, none waiting, the host's where one is due — once the host's turn
    /// is free → what to log of it.
    pub fn step(&mut self, agent: &Agent, workers: &HotWorkers, now: u64) -> Vec<What> {
        let mut said = vec![];
        if let Some(running) = &self.running {
            let gone = matches!(running.checkout.state(now), Ok(WorkerState::Gone));
            let finding = running.checkout.inspected().ok().flatten();
            if !over(running.started, finding.is_some(), gone, now) {
                return said;
            }
            let running = self.running.take().expect("running above");
            said.extend(end(&self.checkout, running, finding, now));
        }
        // Ending an inspection can take time: a snapshot taken before it
        // ended may name a worker whose episode has already cooled.
        let hot = hot_now(workers);
        let seen = Inspector::seen(&hot);
        self.inspected.keep_to(&seen);
        let next = self.inspected.next(&seen);
        // A hot worker waiting goes before the host, whichever fleet's it is.
        self.place.waits(next.is_some());
        let Some(next) = next else {
            said.extend(self.step_host(agent, now));
            return said;
        };
        let found = hot.iter().find(|hot| {
            hot.path.display().to_string() == next.worker && hot.episode.since == next.since
        });
        let Some(hot) = found else { return said };
        let worker = hot.worker.clone();
        match self.start(agent, hot, workers, now) {
            Ok(Started::Waits) => {}
            Ok(Started::Nobody(why)) => {
                self.inspected.record(next);
                said.push(What::Inspected { worker, found: "none".into(), said: why });
            }
            Ok(Started::Running(inspection)) => {
                self.inspected.record(next);
                said.push(What::Inspecting { worker, by: inspection.by.clone() });
                self.running = Some(inspection);
            }
            Err(error) => {
                self.inspected.record(next);
                let why = format!("it could not be started: {error:#}");
                said.push(What::Inspected { worker, found: "failed".into(), said: why });
            }
        }
        said
    }

    /// Starts the host's inspection at `now`, where one is due and no fleet
    /// of the host inspected it in this episode → what to log of it.
    fn step_host(&mut self, agent: &Agent, now: u64) -> Option<What> {
        let (trigger, due) = self.host_due(now)?;
        let reason = due.reason.word().to_string();
        let last = last_for(due.reason);
        if covered(last, &due) {
            self.watching.record(trigger);
            let said = "another fleet of this host inspected it in this episode".to_string();
            return Some(What::HostInspected { reason, found: "covered".into(), said });
        }
        match self.start_host(agent, trigger, &due, now) {
            Ok(Started::Waits) => None,
            Ok(Started::Nobody(said)) => {
                self.watching.record(trigger);
                Some(What::HostInspected { reason, found: "none".into(), said })
            }
            Ok(Started::Running(inspection)) => {
                self.watching.record(trigger);
                let by = inspection.by.clone();
                let reading = match &inspection.on {
                    Subject::Host { reading, .. } => reading.clone(),
                    Subject::Worker { .. } => String::new(),
                };
                self.running = Some(inspection);
                Some(What::HostInspecting { reason, by, reading })
            }
            Err(error) => {
                self.watching.record(trigger);
                let said = format!("it could not be started: {error:#}");
                Some(What::HostInspected { reason, found: "failed".into(), said })
            }
        }
    }

    /// Starts the host's inspection for `due` at `now`: the stock consult,
    /// in a checkout of the base branch of its own, once the host's turn is
    /// taken and what it is given is taken down.
    fn start_host(
        &mut self,
        agent: &Agent,
        trigger: Trigger,
        due: &Due,
        now: u64,
    ) -> Result<Started> {
        if !self.place.turn.free(true) {
            return Ok(Started::Waits);
        }
        let fleet = Fleet::open(&self.checkout, now)?;
        let who = match &fleet.config.bound {
            Some(bound) => stock_as(&fleet, bound)?,
            None => unbound_as(&fleet)?,
        };
        let harness = who.harness();
        let launch = agent.command(harness)?;
        let running = Running {
            repo: fleet.repo.main_checkout()?.display().to_string(),
            worker: THE_HOST.to_string(),
            issue: None,
            by: who.role().to_string(),
            since: now,
            held_back_by: Some(due.reason),
        };
        let Some(turn) = self.place.turn.take(running) else { return Ok(Started::Waits) };
        // Another fleet may have ended its inspection while this one read
        // GitHub. Check again while holding the turn, before taking evidence.
        if covered(last_for(due.reason), due) {
            return Ok(Started::Nobody(
                "another fleet of this host inspected it in this episode".into(),
            ));
        }
        // Read again as it starts: what it is given is what holds back now.
        let admission = Admission::read(&fleet.repo.dir)?;
        let pressure = pressure_level();
        self.watching.ticked(now, &admission.reasons(), pressure.is_some_and(|level| level >= 2));
        if !self.watching.still_due(trigger, due, now, self.span) {
            return Ok(Started::Waits);
        }
        let reading = match trigger {
            Trigger::Pressure => {
                format!("the host is under memory pressure (level {})", pressure.unwrap_or(0))
            }
            Trigger::Admission => admission
                .holds()
                .iter()
                .filter(|hold| Reason::of(hold) == due.reason)
                .map(|hold| admission.said(hold))
                .collect::<Vec<_>>()
                .join("; "),
        };
        let own = Own { repo: &self.repo, stages: &self.stages };
        let evidence = host_evidence::take(&admission, due.reason, &reading, &own)?;
        let source = Source::of(&fleet.repo)?;
        if let Some(left) = Worktree::named(&source, CHECKOUT)? {
            left.remove()?;
        }
        let checkout = Worktree::cut(&source, CHECKOUT, &fleet.base_tip()?, None)?;
        let started = (|| {
            checkout.record_inspection_kind(Kind::Host)?;
            let path = checkout.path.display().to_string();
            let fields = host_inspection::Fields {
                trigger,
                reason: due.reason,
                reading: &reading,
                minutes: now.saturating_sub(due.since) / 60,
                evidence: &evidence.display().to_string(),
                role: who.role(),
                skill: match &who {
                    As::Unbound { .. } => stock::SKILL,
                    As::Bound { skill, .. } => skill,
                },
                instruction: matches!(who, As::Unbound { .. }).then_some(stock::INSTRUCTION),
                operator: Operator {
                    checkout: &path,
                    branch: &checkout.branch,
                    base_branch: &fleet.config.base_branch,
                    afkconvoy: &crate::own_command_in(&path).own(),
                },
            };
            let brief = host_inspection::brief(&fields).map_err(|e| anyhow!(e))?;
            let brief = checkout.write_brief(&brief)?;
            let command = command(&who, launch.as_str(), &checkout, &fleet, THE_HOST)?;
            let admission = Admission::read(&fleet.repo.dir)?;
            self.watching.ticked(now, &admission.reasons(), under_pressure());
            if !self.watching.still_due(trigger, due, now, self.span) {
                return Ok(false);
            }
            let handle = agent.start(&checkout, harness, &command, now)?;
            checkout.await_ready(&handle, agent.ready_timeout)?;
            checkout.deliver(&handle, &brief)?;
            Ok(true)
        })();
        match started {
            Ok(true) => {}
            Ok(false) => {
                checkout.remove()?;
                return Ok(Started::Waits);
            }
            Err(error) => {
                let _ = checkout.remove();
                return Err(error);
            }
        }
        Ok(Started::Running(Box::new(Inspection {
            on: Subject::Host { reason: due.reason, reading },
            checkout,
            started: now,
            by: who.role().to_string(),
            _turn: turn,
        })))
    }

    /// Starts the inspection of `hot` at `now`, in a checkout of the base
    /// branch of its own, once the host's turn is taken.
    fn start(
        &self,
        agent: &Agent,
        hot: &HotNow,
        workers: &HotWorkers,
        now: u64,
    ) -> Result<Started> {
        // Asked first: a worker waiting its turn reads nothing of GitHub.
        if !self.place.turn.free(false) {
            return Ok(Started::Waits);
        }
        let fleet = Fleet::open(&self.checkout, now)?;
        let worktree = hot.worktree();
        let evidence = evidence::of(&worktree, &hot.worker, hot.episode.since)
            .ok_or_else(|| anyhow!("what shows {} hot is not taken down", hot.worker))?;
        let who = match &fleet.config.bound {
            Some(bound) => bound_as(&fleet, bound, hot.issue)?,
            None => fleet.config.stock_consult().then(|| unbound_as(&fleet)).transpose()?,
        };
        let Some(who) = who else {
            let why = match fleet.config.bound {
                Some(_) => "nobody inspects it: its stage says `human`",
                None => "nobody inspects it: the repo says `escalate_to: human`",
            };
            return Ok(Started::Nobody(why.to_string()));
        };
        let harness = who.harness();
        let launch = agent.command(harness)?;
        let running = Running {
            repo: fleet.repo.main_checkout()?.display().to_string(),
            worker: hot.worker.clone(),
            issue: hot.issue,
            by: who.role().to_string(),
            since: now,
            held_back_by: None,
        };
        let Some(turn) = self.place.turn.take(running) else { return Ok(Started::Waits) };
        if !still_hot(workers, hot) {
            return Ok(Started::Waits);
        }
        let source = Source::of(&fleet.repo)?;
        if let Some(left) = Worktree::named(&source, CHECKOUT)? {
            left.remove()?;
        }
        let checkout = Worktree::cut(&source, CHECKOUT, &fleet.base_tip()?, None)?;
        let why = hot.why.clone();
        let terminal = worktree.terminal()?;
        let started = (|| {
            let path = checkout.path.display().to_string();
            let fields = Fields {
                worker: &hot.worker,
                issue: hot.issue,
                why: &why,
                held: &held_said(&hot.episode),
                worktree: &hot.path.display().to_string(),
                terminal: terminal.as_deref().unwrap_or("(it has none)"),
                evidence: &evidence.display().to_string(),
                role: who.role(),
                skill: match &who {
                    As::Unbound { .. } => stock::SKILL,
                    As::Bound { skill, .. } => skill,
                },
                instruction: matches!(who, As::Unbound { .. }).then_some(stock::INSTRUCTION),
                operator: Operator {
                    checkout: &path,
                    branch: &checkout.branch,
                    base_branch: &fleet.config.base_branch,
                    afkconvoy: &crate::own_command_in(&path).own(),
                },
            };
            let brief =
                checkout.write_brief(&inspection::brief(&fields).map_err(|e| anyhow!(e))?)?;
            let command = command(&who, launch.as_str(), &checkout, &fleet, &hot.worker)?;
            if !still_hot(workers, hot) {
                return Ok(false);
            }
            let handle = agent.start(&checkout, harness, &command, now)?;
            checkout.await_ready(&handle, agent.ready_timeout)?;
            checkout.deliver(&handle, &brief)?;
            Ok(true)
        })();
        match started {
            Ok(true) => {}
            Ok(false) => {
                checkout.remove()?;
                return Ok(Started::Waits);
            }
            Err(error) => {
                let _ = checkout.remove();
                return Err(error);
            }
        }
        Ok(Started::Running(Box::new(Inspection {
            on: Subject::Worker { hot: hot.clone(), why },
            checkout,
            started: now,
            by: who.role().to_string(),
            _turn: turn,
        })))
    }
}

/// The line the inspection's terminal in `checkout` runs, `launch` being the
/// worker launch command of its harness: in a bound repo, with what starts it
/// in its role and holds it, as the operator on this host, to its bounds.
fn command(
    who: &As,
    launch: &str,
    checkout: &Worktree,
    fleet: &Fleet,
    worker: &str,
) -> Result<String> {
    if let As::Unbound { harness, model: Some(model) } = who {
        return Ok(harness.with_model(launch, model));
    }
    let As::Bound {
        harness,
        model,
        role,
        skill,
        role_copy,
        team_copy,
        team_rules,
        host_user,
        dsh_profile,
        escalate_label,
    } = who
    else {
        return Ok(launch.to_string());
    };
    let told = ToldRules::read(*harness, &checkout.path, team_rules.as_deref())?;
    let (kept, shadowed) = crate::kept::for_worker(
        *harness,
        *host_user,
        &role_copy.dir,
        &told.told(),
        &crate::own_command().fixed(" guard").own(),
    )?;
    let start = Start {
        role_dir: &role_copy.dir.display().to_string(),
        worktree: &checkout.path.display().to_string(),
        team_dir: &team_copy.dir.display().to_string(),
        model: model.as_deref(),
        guard: &crate::own_command().fixed(" guard").own(),
        kept: &kept,
        host_user: *host_user,
        dsh_profile: *dsh_profile,
        shadowed: &shadowed,
        told: told.told(),
        facts: Facts {
            on: On::Inspection(worker),
            role,
            skill,
            writes: true,
            consult: false,
            base_branch: &fleet.config.base_branch,
            escalate_label,
            lock_label: &fleet.config.lock_label,
            role_commit: &role_copy.commit,
            team_commit: &team_copy.commit,
        },
    };
    Ok(harness.in_role(launch, &start))
}

/// Ends `inspection` of the fleet of the checkout `repo`, which said
/// `finding` — at `now` — and removes its checkout, its terminal with it →
/// what to log of it: a line for each thing it did, then how it ended, with
/// the PRs it put on the landing train. What cannot be done of it is said,
/// and the rest done all the same.
fn end(repo: &Repo, inspection: Box<Inspection>, finding: Option<Finding>, now: u64) -> Vec<What> {
    // As its checkout says them last: one put on the train after it said.
    let prs = inspection.checkout.inspection_prs().unwrap_or_default();
    let finding = finding.map(|finding| Finding { prs, ..finding });
    let worker = match &inspection.on {
        Subject::Worker { hot, .. } => hot.worker.clone(),
        Subject::Host { .. } => THE_HOST.to_string(),
    };
    let did = finding.iter().flat_map(|finding| &finding.did);
    let mut said: Vec<What> = did
        .map(|did| What::InspectionDid { worker: worker.clone(), did: inspection::one_line(did) })
        .collect();
    let mut ended = match &inspection.on {
        Subject::Worker { hot, why } => end_worker(repo, hot, why, &inspection.by, finding),
        Subject::Host { reason, reading } => {
            end_host(repo, (*reason, reading.as_str()), &inspection, finding, now)
        }
    };
    if let Err(error) = inspection.checkout.remove() {
        let removed = format!(" · its checkout was not removed: {error:#}");
        match &mut ended {
            What::Inspected { said, .. } | What::HostInspected { said, .. } => {
                said.push_str(&removed)
            }
            _ => {}
        }
    }
    said.push(ended);
    said
}

/// What an inspection said, as the log says it, or that it said nothing in
/// time.
fn said_of(finding: Option<&Finding>) -> String {
    match finding {
        Some(finding) => finding.line(),
        None => format!("it said nothing in {} minutes", inspection::TIME_LIMIT / 60),
    }
}

/// Ends the host's inspection, which the fleet of the checkout `repo`
/// started, of what admission held back for — `reason`, having read
/// `reading` — which said `finding` at `now`: keeps it for `doctor` and the
/// panel, beside admission's reading.
fn end_host(
    repo: &Repo,
    (reason, reading): (Reason, &str),
    inspection: &Inspection,
    finding: Option<Finding>,
    now: u64,
) -> What {
    let mut said = said_of(finding.as_ref());
    let found = finding.as_ref().map_or("nothing", |_| SAID);
    let (did, prs) = finding
        .as_ref()
        .map(|finding| (finding.did.clone(), finding.prs.clone()))
        .unwrap_or_default();
    let last = Last {
        repo: repo.main_checkout().map(|main| main.display().to_string()).unwrap_or_default(),
        reason,
        reading: reading.to_string(),
        by: inspection.by.clone(),
        started: inspection.started,
        ended: now,
        found: found.to_string(),
        said: finding.as_ref().map_or_else(|| said.clone(), |finding| finding.said.clone()),
        did,
        prs,
    };
    if let Err(error) = keep_last(&last) {
        said.push_str(&format!(" · what it did was not kept: {error:#}"));
    }
    What::HostInspected { reason: reason.word().to_string(), found: found.to_string(), said }
}

/// Ends the inspection of the hot worker `hot`, `why` it turned hot, by
/// `by`, which said `finding`: leaves it on the issue, in the tracker of the
/// checkout `repo`.
fn end_worker(repo: &Repo, hot: &HotNow, why: &str, by: &str, finding: Option<Finding>) -> What {
    let worker = hot.worker.clone();
    let mut said = said_of(finding.as_ref());
    if let Some(issue) = hot.issue {
        let body = inspection::comment(&worker, why, by, finding.as_ref());
        if let Err(error) = GitHub::of(repo).and_then(|gh| gh.comment(issue, &body)) {
            said.push_str(&format!(" · no comment was left on #{issue}: {error:#}"));
        }
    }
    let found = finding.as_ref().map_or("nothing", |_| SAID);
    What::Inspected { worker, found: found.to_string(), said }
}

/// The checkout `repo` is in, which must be an inspection's: only an
/// inspection says what it did, or lands a change, from the checkout it was
/// started in.
fn inspection_checkout(repo: &Repo) -> Result<Worktree> {
    let checkout = Worktree::of(repo)?;
    let named = checkout.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if !afkconvoy_core::worker::is_worktree_named(named, CHECKOUT) {
        anyhow::bail!(
            "{} is no inspection's checkout: only an inspection says what it did, or lands a \
             change of its own, from the checkout it was started in",
            checkout.path.display()
        );
    }
    Ok(checkout)
}

/// `afkconvoy inspected`: keeps what the inspection in the checkout
/// `checkout` said — what held the work back, and each thing it `did` — with
/// the PRs it put on the landing train, for its `run` to log, leave on the
/// issue or keep beside admission's reading, and to end it.
pub fn inspected(checkout: &Repo, said: &str, did: &[String]) -> Result<Value> {
    if said.trim().is_empty() {
        anyhow::bail!("say what held the work back, where and why: the text is empty");
    }
    let did: Vec<String> = did.iter().map(|item| item.trim().to_string()).collect();
    if did.iter().any(String::is_empty) {
        anyhow::bail!("a --did is empty: say what was done");
    }
    let checkout = inspection_checkout(checkout)?;
    let kind = checkout.inspection_kind()?;
    let finding = Finding { said: said.trim().to_string(), did, prs: checkout.inspection_prs()? };
    checkout.record_inspected(&finding)?;
    Ok(json!({"inspected": true, "kind": kind.word(), "found": SAID, "did": finding.did,
        "prs": finding.prs}))
}

/// `afkconvoy land --inspection`: the inspection in the checkout `repo` puts
/// the PR of its branch there on the landing train — a change of the repo it
/// made itself, on no issue — as a worker's joins it (`train::join`), its
/// join commit naming the inspection on `host` where a worker's names its
/// issue. A PR that joined, or that the base branch already holds, is kept
/// in the checkout's notes for the inspection's record. With a fleet lock
/// standing it joins nothing → `locked`: an inspection's is no lock issue's.
pub fn land(repo: &Repo, config: &Config, gh: &GitHub, host: &str, now: u64) -> Result<Value> {
    let checkout = inspection_checkout(repo)?;
    let Some(branch) = checkout.branch_now()? else {
        anyhow::bail!(
            "{} is on no branch: commit the change on the branch of this checkout, push it and \
             open its PR, then run this again",
            checkout.path.display()
        );
    };
    let prs = gh.open_prs()?;
    let Some(pr) = prs.iter().find(|pr| pr.head_branch == branch) else {
        anyhow::bail!(
            "no open PR has this checkout's branch {branch:?} for its head — push it \
             (`git push origin HEAD`) and open one (`gh pr create --base {} --head {branch}`), \
             then run this again",
            config.base_branch
        );
    };
    let answer = json!({"inspection": host, "pr": pr.number});
    let locks = crate::lock::standing_locks(gh, config)?;
    if !locks.is_empty() {
        return Ok(crate::train::locked(answer, &locks));
    }
    let of = Of::Inspection(host.to_string());
    let answer = crate::train::join(repo, config, &gh.tracker, pr, &of, answer, now)?;
    if matches!(answer["outcome"].as_str(), Some("joined" | "merged")) {
        checkout.record_inspection_pr(pr.number)?;
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_says_which_kind_of_inspection_holds_it() {
        // As a `run` of a version before host inspections wrote it.
        let worker: Running = serde_json::from_str(
            r##"{"repo": "/src/widgets", "worker": "#7", "issue": 7, "by": "stock-consult", "since": 700}"##,
        )
        .unwrap();
        assert_eq!(json(&worker)["kind"], "worker");
        let host = Running {
            worker: THE_HOST.into(),
            issue: None,
            held_back_by: Some(Reason::Disk),
            ..worker
        };
        let said = json(&host);
        assert_eq!(
            (said["kind"].as_str(), said["held_back_by"].as_str()),
            (Some("host"), Some("disk"))
        );
        let again: Running = serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
        assert_eq!(again, host);
    }

    #[test]
    fn one_inspection_holds_the_turn_and_a_hot_worker_waiting_goes_before_the_host() {
        let worker = Running {
            repo: "/src/widgets".into(),
            worker: "#7".into(),
            issue: Some(7),
            by: "stock-consult".into(),
            since: 700,
            held_back_by: None,
        };
        let host = Running {
            worker: THE_HOST.into(),
            issue: None,
            held_back_by: Some(Reason::Load),
            ..worker.clone()
        };
        let turn = Turn::default();
        let (alpha, beta) = (turn.place(), turn.place());
        let taken = turn.take(worker.clone()).expect("free");
        assert_eq!(turn.running(), Some(worker.clone()));
        assert!(turn.take(host.clone()).is_none(), "one at a time, of either kind");
        drop(taken);
        assert_eq!(turn.running(), None);

        // a hot worker of beta's waits: the host waits behind it
        beta.waits(true);
        assert!(!turn.free(true) && turn.free(false));
        assert!(turn.take(host.clone()).is_none());
        alpha.waits(false);
        assert!(turn.take(host.clone()).is_none(), "another's place says nothing of beta's");
        drop(beta);
        let taken = turn.take(host.clone()).expect("no hot worker waits once its place is gone");
        assert_eq!(turn.running(), Some(host));
        drop(taken);
    }
}

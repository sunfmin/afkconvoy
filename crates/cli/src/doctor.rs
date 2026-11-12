//! `afkconvoy doctor`: the state of the fleet on this host, and what is wrong
//! with it — each finding with the line to run, or the file and key to edit.
//!
//! `read` is the half that asks: this host, the log of its `run`, GitHub and
//! the remote, orca after each worker; and whether the tracker is public,
//! which `tracker::findings` asks and judges. It writes nothing anywhere but the
//! fleet's own cache, and makes no startup probe: it is run beside a live
//! fleet, often. While a startup probe that checked the kit's current launch
//! commands still stands, it starts no harness; otherwise it starts a ready
//! harness once, and asks a logged-out one once more. `findings` is the half that judges what was read, and asks
//! nothing. Nothing here fixes anything: the fleet's own commands do.
//!
//! What a run's log says is read by `run_log`. Whether a claim's health
//! observation is overdue, its worker stopped or its worker disputed is
//! judged by health (`health::Pace`), the tick (`judge::silenced`) and the
//! worker (`worker::disputed_past_grace`), each from what this look read
//! once: doctor makes their judgments findings, and keeps no rule of theirs.

use std::path::{Path, PathBuf};

use afkconvoy_core::casting;
use afkconvoy_core::claims::{InstanceId, Owner, claim_ref};
use afkconvoy_core::config::{Config, SaidBy};
use afkconvoy_core::cut;
use afkconvoy_core::gate::GateCommand;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::lease::{LEASE, stamp_age, stamp_stale};
use afkconvoy_core::left::{Kept, KeptBecause};
use afkconvoy_core::lock::keeps_off;
use afkconvoy_core::patrol::Standing;
use afkconvoy_core::plan::{LabelBar, NOT_OURS, Status, WorkingSet, closing_pr};
use afkconvoy_core::slow_gate;
use afkconvoy_core::team::Fault;
use afkconvoy_core::tools::{Found, START_ORCA, Tool, add_checkout, harness_missing};
use afkconvoy_core::tracker::Tracker;
use afkconvoy_core::waiting::{WAITED_LONG, Waiting};
use afkconvoy_core::worker::{self, WorkerState};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::admission::Readings;
use crate::checkout_list::{List, repo_of};
use crate::claims::Me;
use crate::config;
use crate::fleet::Fleet;
use crate::fleet_report::FleetReport;
use crate::gate::head_tree;
use crate::git::{REMOTE, Repo};
use crate::host_runner;
use crate::judge::{Silenced, leaving_at_open};
use crate::launch::Launches;
use crate::log::{self, Entry, Failed, Log, Tick, What, by_cause};
use crate::orca::{Source, Under, Worktree};
use crate::panel::lasted;
use crate::role::{self, Checked};
use crate::run_log::{self, BrokenRead, Event, HotRead};
use crate::stage::Stood;
use crate::team::{self, Refused};
use crate::tools;
use crate::topic::See;

pub mod watching;

/// How many ticks in a row a step fails in before its finding is critical.
const FAILED_TICKS: usize = 3;
/// How many failed steps the answer lists, newest first.
pub(crate) const FAILED_SHOWN: usize = 10;
/// How long an issue is at a stage before its stay is held against the
/// stages before it.
const STAGE_LONG: u64 = 900;

/// What the answer says of a host whose `run` left no log.
const NO_LOG: &str = "no log";

/// How much a finding matters, most first: `doctor --help` lists it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, agent_help::Help,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The fleet does not work, or will not, until it is fixed.
    Critical,
    /// One issue or one patrol is held up, or is about to be.
    Warning,
    /// Nothing is held up: said so that a surprise is explained.
    Info,
}

/// One thing that is wrong, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// What it is a finding of: the watch acts on it by this, never by its
    /// words.
    pub kind: Kind,
    pub severity: Severity,
    pub what: String,
    /// What was read that says so.
    pub evidence: String,
    /// A line to run, or a file and the key in it to edit: never prose alone.
    pub fix: String,
    /// The command that prints the topic explaining it.
    pub see: See,
    /// Whether it is the host's (`Kind::of_host`), which `doctor --all`
    /// says once.
    #[serde(skip)]
    pub of_host: bool,
    /// The condition's identity, made where it is judged and the same while
    /// it lasts, however its words change: `checkout:` one of this checkout,
    /// `host:` one of the host, a resource's its episode. Doctor answers it
    /// scoped as the watch acts on it (`answered`).
    pub watch_id: String,
}

/// Names each kind of finding there is, with how much it matters and its
/// symptom: the words its `what` always holds.
macro_rules! kinds {
    ($($kind:ident: $severity:ident, $symptom:literal;)+) => {
        /// One kind of thing `doctor` finds wrong: what a finding is made as.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Kind {
            $($kind,)+
        }

        impl Kind {
            /// Every kind there is.
            pub const ALL: &[Kind] = &[$(Kind::$kind,)+];

            /// The words the `what` of every finding of this kind holds: what
            /// the help topic `troubleshooting` heads its row with.
            pub fn symptom(self) -> &'static str {
                match self {
                    $(Kind::$kind => $symptom,)+
                }
            }

            fn severity(self) -> Severity {
                match self {
                    $(Kind::$kind => Severity::$severity,)+
                }
            }

            /// The topic that explains a finding of this kind: `humans`
            /// where what it reports waits for a human, else the one that
            /// has a row for each.
            fn see(self) -> See {
                match self {
                    Kind::LockUnworked | Kind::PatrolFailed | Kind::CutFailed | Kind::WaitedLong => {
                        See::Humans
                    }
                    _ => See::Troubleshooting,
                }
            }
        }
    };
}

kinds! {
    ToolMissing: Critical, "is not installed";
    ToolLoggedOut: Critical, "is not logged in";
    NoProvider: Critical, "holds credentials for no provider";
    OrcaUnasked: Critical, "cannot be asked about this checkout";
    OrcaNotRunning: Critical, "is not running";
    NoCheckout: Critical, "does not know this checkout";
    LaunchStartsNone: Critical, "cannot start a worker";
    LaunchTakesNoRole: Critical, "cannot start a worker in its role";
    NoRun: Critical, "no `afkconvoy run` is alive";
    Broken: Critical, "is broken";
    PublicTracker: Critical, "is public: its fleet instance works nothing";
    RepoUnworked: Warning, "no fleet has taken";
    RepoUnreadable: Warning, "remote could not be read";
    StepFailed: Critical, "failed in each of the last";
    StepUnrecovered: Warning, "failed in tick";
    HarnessDown: Critical, "harness is down";
    HarnessPaused: Warning, "is paused on this host";
    HotWorker: Warning, "hot worker is past its limits";
    AdmissionHeld: Warning, "admission has held back new workers";
    MemoryPressure: Warning, "the host has been under memory pressure";
    PressureElsewhere: Info, "under memory pressure from another account";
    KitUnplaced: Warning, "has unplaced models in the host kit";
    AgentSetupDrifted: Warning, "differs from the agent setup";
    TmuxMissing: Warning, "Guild machine has no tmux";
    WatchUnavailable: Critical, "no harness can start this host's watch";
    WatchBusy: Critical, "the resident watch is busy; its round remains undelivered";
    NotifyFailed: Warning, "human decision sending has failed repeatedly";
    DaybookUnwritten: Warning, "a watch round's daybook entry is not written";
    NotMoved: Warning, "the host runner did not move onto";
    NotUpgraded: Warning, "is published, newer than the host runner's";
    PathStale: Warning, "is first on PATH";
    RosterModelUnmatched: Warning, "names no model the host kit holds";
    TeamUnread: Critical, "the team this repo names cannot be read";
    TeamFaulty: Critical, "has a fault at its head";
    NothingSafe: Warning, "has no harness on this host it holds limits for";
    NamedHarness: Warning, "names a harness no worker of this checkout starts on";
    RosterUnmatched: Warning, "matches nothing in the host kit";
    LockUnworked: Critical, "stands and cannot be worked";
    StateUnread: Critical, "the fleet's state cannot be read";
    NoWorker: Warning, "has no worker";
    WorkerStopped: Warning, "has been stopped for";
    WorkerDisputed: Warning, "its session says its worker works, and orca says it is done";
    SettleWaiting: Warning, "settling has waited for its worker";
    WorkerHealth: Warning, "worker health is waiting for the operator";
    HealthOverdue: Warning, "its health observation is overdue";
    NotAlive: Warning, "its fleet instance is not alive";
    TickFailed: Warning, "a tick failed since the last look";
    GoalMisbehaved: Warning, "worker's goal misbehaved";
    NothingStarted: Warning, "the last tick started no worker";
    NoWorkflow: Warning, "follows no workflow of the team";
    BlockerUnworked: Warning, "waits on a blocker no fleet works";
    LongAtStage: Warning, "longer than its baseline";
    NearBreaker: Warning, "is one rework from a breaker";
    PatrolFailed: Warning, "does not run: its last run failed";
    PatrolFull: Warning, "have reached its `max_open`";
    CutFailed: Warning, "cuts no version: its last cut failed";
    CutNeed: Warning, "not cutting here";
    CutWaiting: Info, "wait on the next cut";
    NeedInstallFailed: Warning, "install failed on this host";
    NeedInstalled: Info, "was installed on this host";
    WaitedLong: Warning, "has waited for a human for";
    WaitTaken: Info, "human wait is taken by";
    NotOurs: Warning, "is held off by a PR not the fleet's";
    ClaimOnGone: Warning, "is claimed but GitHub has it no more";
    SlowGate: Warning, "the gate is slow";
    ExclusiveBound: Warning, "ended at the exclusive bound";
    Moved: Info, "moved since the stage";
    RetiredConcurrency: Info, "which nothing reads any more";
    KeptWorktree: Info, "is kept with no claim";
    TakenWorktree: Info, "is kept after its claim was taken";
    NoGateLimit: Info, "declares no gate limit";
    OtherAccount: Info, "works on this machine under another account";
}

impl Kind {
    /// Whether what a finding of this kind says is wrong is the host's, the
    /// same whichever checkout `doctor` is asked in: a command missing or
    /// logged out, orca not running, a harness paused, the host runner not
    /// moved onto a newer afkconvoy.
    fn of_host(self) -> bool {
        matches!(
            self,
            Kind::ToolMissing
                | Kind::ToolLoggedOut
                | Kind::NoProvider
                | Kind::OrcaNotRunning
                | Kind::HarnessPaused
                | Kind::HotWorker
                | Kind::AdmissionHeld
                | Kind::MemoryPressure
                | Kind::PressureElsewhere
                | Kind::KitUnplaced
                | Kind::AgentSetupDrifted
                | Kind::TmuxMissing
                | Kind::WatchUnavailable
                | Kind::WatchBusy
                | Kind::NotifyFailed
                | Kind::DaybookUnwritten
                | Kind::NotMoved
                | Kind::NotUpgraded
                | Kind::PathStale
                | Kind::OtherAccount
                | Kind::RepoUnworked
                | Kind::RepoUnreadable
                | Kind::WorkerHealth
                | Kind::HealthOverdue
        )
    }

    /// Whether the host's watch answers a finding of this kind: not one that
    /// says the watch itself cannot start or go on, nor the PATH of the shell
    /// `doctor` was run in, which are the operator's, nor a release published
    /// since, which the host runner installs itself and a `brew` of the
    /// watch's would race on the host that cuts it.
    pub fn watched(self) -> bool {
        !matches!(
            self,
            Kind::WatchUnavailable
                | Kind::WatchBusy
                | Kind::NotifyFailed
                | Kind::DaybookUnwritten
                | Kind::PathStale
                | Kind::NotUpgraded
        )
    }
}

/// A finding of `kind`, of the condition `identity`: what tells it from every
/// other of its kind, made where it is judged and never from its words.
pub(crate) fn finding(
    kind: Kind,
    identity: Value,
    what: String,
    evidence: String,
    fix: String,
) -> Finding {
    debug_assert!(what.contains(kind.symptom()), "{kind:?} is not said in: {what}");
    let (severity, see, of_host) = (kind.severity(), kind.see(), kind.of_host());
    let watch_id = unscoped(kind, &identity);
    Finding { kind, severity, what, evidence, fix, see, of_host, watch_id }
}

/// A finding's `watch_id` as it is judged: `identity`, said to be the host's
/// or its checkout's by its `kind`.
fn unscoped(kind: Kind, identity: &Value) -> String {
    let scope = if kind.of_host() { "host" } else { "checkout" };
    format!("{scope}:{identity}")
}

/// A human question's source and answer, independent of its displayed age
/// and issue title. Both doctor representations use this same condition.
#[derive(serde::Deserialize, Serialize)]
struct HumanWait {
    issue: u64,
    since: Option<u64>,
    url: Option<String>,
    why: String,
    answer: String,
}

impl From<&Waiting> for HumanWait {
    fn from(waiting: &Waiting) -> Self {
        Self {
            issue: waiting.issue,
            since: waiting.since,
            url: waiting.url.clone(),
            why: waiting.why.clone(),
            answer: waiting.answer.clone(),
        }
    }
}

impl HumanWait {
    /// The question's identity, which its finding and its place among those
    /// waiting on a human share.
    fn identity(&self) -> Value {
        json!(["human", self])
    }
}

/// The identity the watch acts on a finding of `kind` by, made of its
/// `watch_id`, `id`, where it was found, `at`: a checkout's scoped to that
/// checkout, a host's to the host, and a kit's unplaced models to their
/// research. A resource's holds its scope already.
pub(crate) fn scoped(at: &str, kind: Kind, id: &str) -> String {
    if kind == Kind::KitUnplaced {
        format!("{}{id}", afkconvoy_core::host_watch::RESEARCH_ITEM)
    } else if let Some(identity) = id.strip_prefix("checkout:") {
        format!("finding:{at}:{identity}")
    } else if let Some(identity) = id.strip_prefix("host:") {
        format!("finding:host:{identity}")
    } else {
        id.to_string()
    }
}

/// `found` as doctor answers it of `at`, the checkout it was found in, or
/// `host`: each `watch_id` scoped as the watch acts on it, which the watch
/// reads as given.
pub(crate) fn answered(at: &str, found: Vec<Finding>) -> Result<Value> {
    let found: Vec<Finding> = found
        .into_iter()
        .map(|finding| Finding { watch_id: scoped(at, finding.kind, &finding.watch_id), ..finding })
        .collect();
    Ok(serde_json::to_value(found)?)
}

/// The identity the watch acts on `waiting`, an issue of the checkout `at`
/// in doctor's `waiting_on_human`, by: the one its finding is answered with
/// once it has waited long.
pub(crate) fn waiting_id(at: &str, waiting: &Value) -> Result<String> {
    let wait: HumanWait = serde_json::from_value(waiting.clone())?;
    let kind = Kind::WaitedLong;
    Ok(scoped(at, kind, &unscoped(kind, &wait.identity())))
}

/// An answer with a finding of the highest severity: printed as it is, with
/// a non-zero exit, so that a script can gate on it.
#[derive(Debug)]
pub struct Unwell(pub Value);

impl std::fmt::Display for Unwell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the fleet has a critical finding")
    }
}

impl std::error::Error for Unwell {}

/// The worker launch command of one harness the fleet could be asked for,
/// as far as this host can say what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The harness, what calls for it, its command and that command's
    /// faults, each said with the harness and what calls for it.
    pub checked: Checked,
}

/// What this host has of what the fleet needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub tools: Vec<(Tool, Found)>,
    /// The host `gh` was asked about.
    pub gh_host: Option<String>,
    /// Why an orca that is running could not be asked about this checkout
    /// (`tools::Looked`).
    pub orca_unasked: Option<String>,
    /// One for each harness the fleet could be asked for — in an unbound
    /// repo, its one command's. A harness is left out on a custom provider
    /// with no `run` to say its command: a start would ask.
    pub launches: Vec<Launch>,
}

/// The heartbeats found on the remote: a start sweeps the dead ones that hold
/// nothing, so more than a few is fleets that were never started again.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Heartbeats {
    pub found: usize,
    /// The instances whose heartbeat is within the lease.
    pub live: Vec<String>,
}

/// The `run` of this host for this repo.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Whether this host's runner holds a fleet instance of this repo: it is
    /// alive.
    pub alive: bool,
    /// Its fleet instance: the one its log names, else the one the claims
    /// stamped with this host name. `None`: this host never ran one.
    pub instance: Option<String>,
    pub host: String,
    /// How old its heartbeat is; `None`: it has none on the remote.
    pub heartbeat_age: Option<u64>,
    /// Every heartbeat the remote has, whoever stamped it.
    pub heartbeats: Heartbeats,
    /// Where its log is; `None`: this host has no cache directory.
    pub log_file: Option<PathBuf>,
    /// The ticks its log has of that instance, oldest first, each with when
    /// it ended; `None` where there is no log.
    pub ticks: Option<Vec<(u64, Tick)>>,
    /// The seconds its last tick took, from its start line to its end;
    /// `None` where the log has no start of it.
    pub last_took: Option<u64>,
    /// When its tick in progress began; `None` once it ended or where the
    /// log has no start of it.
    pub tick_started: Option<u64>,
    /// The newest events that reached this host's `run`s, newest first.
    pub events: Vec<Event>,
    /// The workers its log says are hot: each turned hot, and has not
    /// stopped being hot since.
    pub hot: Vec<HotRead>,
    /// Its fleet instance broken, as its log has it: the host runner starts
    /// it again after a backoff.
    pub broken: Option<BrokenRead>,
}

impl Run {
    fn last_tick(&self) -> Option<&(u64, Tick)> {
        self.ticks.as_ref().and_then(|ticks| ticks.last())
    }

    /// A claim is left to the tick that is starting it, however long that
    /// tick takes. A later completed tick proves it had its chance;
    /// otherwise it gets one sleep plus the last tick's duration.
    /// Unknown stamps and missing log evidence keep the existing finding.
    fn dispatching(&self, held_for: Option<u64>, now: u64) -> bool {
        let Some(age) = held_for else { return false };
        let stamped = now.saturating_sub(age);
        if self.tick_started.is_some_and(|started| started <= stamped) {
            return true;
        }
        if let (Some((ended, _)), Some(took)) = (self.last_tick(), self.last_took)
            && *ended <= now
            && ended.saturating_sub(took) > stamped
        {
            return false;
        }
        let sleep = self.last_tick().map(|(_, tick)| tick.sleep).unwrap_or(0);
        age < sleep.saturating_add(self.last_took.unwrap_or(0))
    }
}

/// A role or the team a stage in flight started under, that has moved since:
/// its directory holds something else at the head of its repo than at the
/// commit the stage started under. A commit that differs moves nothing by
/// itself: a push to a repo of several roles gives each a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// `the team`, or the role by its name.
    pub which: String,
    /// The commit the stage started under.
    pub started: String,
    pub head: String,
}

/// One claim of this host's fleet instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub issue: u64,
    pub title: Option<String>,
    pub status: Status,
    pub pr: Option<u64>,
    pub stage: Option<String>,
    pub role: Option<String>,
    /// The seconds since the claim was stamped.
    pub held_for: Option<u64>,
    /// `None`: orca could not say.
    pub worker: Option<WorkerState>,
    /// Since when orca has reported its worker's agent `done`, in epoch
    /// seconds; `None` when it reports anything else or nothing.
    pub orca_done: Option<u64>,
    /// Its worker is silent, by the read the tick routes it from
    /// (`judge::silenced`), with the command the tick runs next for it.
    /// `None` when it is not, or that could not be read.
    pub silent: Option<Silenced>,
    pub worktree: Option<PathBuf>,
    /// What its worker was started under, in a bound repo.
    pub under: Option<Under>,
    /// What of that has moved since: the team, its role, or both.
    pub moved: Vec<Moved>,
    /// When the green gate run on record for the tree its worktree is at
    /// finished; `None`: none stands.
    pub gate: Option<u64>,
}

impl Held {
    /// Whether a worker is still owed on it: it is not on the train, landed
    /// or closed.
    fn at_work(&self) -> bool {
        matches!(self.status, Status::NoPr | Status::Landing | Status::AtStage)
    }
}

/// The team a bound repo names, as it stands at its head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamRead {
    Unbound,
    Loaded {
        /// The flow label of each workflow it has.
        flow_labels: Vec<String>,
        /// Each role no worker of which can be started on this host, with
        /// why (`casting::nothing_safe`).
        nothing_safe: Vec<(String, String)>,
        unmatched_models: Vec<casting::UnmatchedModel>,
        /// Each role whose roster entry matches nothing in the host kit, with
        /// what it allows and the host kit holds (`casting::unmatched_entries`).
        unmatched: Vec<(String, String)>,
    },
    Faulty {
        name: String,
        faults: Vec<Fault>,
    },
    /// It could not be read at all, and why.
    Unread(String),
}

/// What GitHub, the remote and orca say of the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The live hosts holding human waits, from this look's remote scan.
    pub wait_holders: std::collections::BTreeMap<u64, String>,
    pub set: WorkingSet,
    pub targets: Value,
    pub refusals: Value,
    pub held: Vec<Held>,
    pub stood: Vec<Stood>,
    pub stage_baselines: std::collections::BTreeMap<String, (u64, usize)>,
    pub team: TeamRead,
    pub ready_label: String,
    /// The fleet's tracker: a fix that runs `gh issue` names it.
    pub tracker: Tracker,
    /// The worktrees on this host whose issue holds no claim.
    pub unclaimed: Vec<Unclaimed>,
    /// The open ready issues held off, with no claim, only by an open PR
    /// that is not the fleet's (`plan::NOT_OURS`).
    pub not_ours: Vec<NotOurs>,
    /// The claims on issues GitHub says were deleted, transferred or never
    /// issued.
    pub on_gone: Vec<OnGone>,
    /// Candidate state from the same remote listing as the working set.
    pub candidates: Value,
    /// Jira deliveries from the same remote listing as the working set.
    pub deliveries: Vec<Value>,
}

/// A claim on an issue GitHub has no more: deleted, transferred to another
/// repo, or never issued. Its holder's next tick lets it go; until
/// then it holds a slot of that fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnGone {
    pub issue: u64,
    /// The claim's ref on the remote.
    pub claim_ref: String,
    /// The fleet instance that holds it, as its record names it.
    pub holder: Option<String>,
}

/// An open ready issue no claim holds, kept off the frontier by an open PR
/// from a branch the fleet never recorded on it: no worker is started on it,
/// and nothing else moves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotOurs {
    pub issue: u64,
    pub pr: u64,
    pub branch: String,
}

/// A worktree on this host with no local claim: one a park or an escalation
/// kept, or whose claim another host took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unclaimed {
    pub issue: u64,
    pub path: PathBuf,
    pub branch: String,
    /// Why it is kept; `None` when it holds nothing the remote does not.
    pub kept: Option<Kept>,
    /// The other host's holder; absent when the issue holds no claim.
    pub taken: Option<Owner>,
}

/// Everything `doctor` read.
pub struct Seen<'a> {
    pub now: u64,
    pub host: &'a Host,
    /// The checkout `doctor` is asked in.
    pub checkout: &'a Path,
    /// The repo's config, with the concurrency the checkout says on this
    /// host laid over it (`concurrency::config_of`); `None` where it cannot
    /// be read.
    pub config: Option<&'a Config>,
    pub run: &'a Run,
    /// The fleet's state, or why it could not be read.
    pub state: Result<&'a State, &'a str>,
}

/// The first complete backtick-quoted line in the fleet's error context.
/// Only its first line can name a fix: subsequent lines hold screens and
/// command output. Inline delivery receipts and the cause after `failed:`
/// are external evidence too, even when they quote a plausible fleet command.
/// Keep that evidence in the finding, but never take a line to run from it.
fn named_line(error: &str) -> Option<String> {
    let context = error.split(['\n', '\r']).next().unwrap_or_default();
    let context = context.split("delivery receipt:").next().unwrap_or_default();
    let context = context.split(" failed:").next().unwrap_or_default();
    let mut remaining = context;
    loop {
        let (_, quoted) = remaining.split_once('`')?;
        // Require the closing backtick before the evidence boundary too.
        let (line, rest) = quoted.split_once('`')?;
        let line = line.trim();
        if line.contains(' ') {
            return Some(line.to_string());
        }
        remaining = rest;
    }
}

fn on(command: &str, issue: u64, instance: &str) -> String {
    format!("afkconvoy {command} --issue {issue} --instance {instance}")
}

fn numbered(issues: &[u64]) -> String {
    issues.iter().map(|issue| format!("#{issue}")).collect::<Vec<_>>().join(", ")
}

impl Host {
    /// Every tool looked for on this host, each with what calls for it: the
    /// ones every fleet needs and the default harness, which nothing names,
    /// then each other harness a stage or a patrol names.
    fn looked_for(&self) -> impl Iterator<Item = (&(Tool, Found), &[String])> {
        let named = self.launches.iter().filter_map(|launch| {
            launch.checked.tool.as_ref().map(|tool| (tool, &launch.checked.callers[..]))
        });
        self.tools.iter().map(|tool| (tool, &[][..])).chain(named)
    }
}

fn host_findings(host: &Host) -> Vec<Finding> {
    let mut found = vec![];
    let gh_host = host.gh_host.as_deref();
    for ((tool, state), callers) in host.looked_for() {
        let name = tool.name();
        let called = match callers {
            [] => String::new(),
            callers => format!("; it is called for by {}", callers.join(", ")),
        };
        let (kind, what, evidence, fix) = match state {
            Found::Ready => continue,
            Found::Missing => (
                Kind::ToolMissing,
                format!("`{name}` is not installed: the fleet cannot work without it"),
                format!("no `{name}` was found on this host{called}"),
                tool.when_missing(),
            ),
            Found::LoggedOut(answer) => (
                if *tool == Tool::Harness(Harness::Pi) {
                    Kind::NoProvider
                } else {
                    Kind::ToolLoggedOut
                },
                format!("`{name}` {}", tool.logged_out()),
                format!(
                    "`{}` says so, in a login shell{called}\n{}",
                    tool.login_question(gh_host).unwrap_or_default(),
                    answer.evidence()
                ),
                tool.login(gh_host),
            ),
            Found::NotRunning => match &host.orca_unasked {
                Some(why) => (
                    Kind::OrcaUnasked,
                    format!(
                        "`{name}` cannot be asked about this checkout: no worker can be started"
                    ),
                    why.clone(),
                    "orca repo list".to_string(),
                ),
                None => (
                    Kind::OrcaNotRunning,
                    format!("`{name}` is not running: no worker can be started or asked after"),
                    "`orca status` does not say its runtime is ready".to_string(),
                    START_ORCA.to_string(),
                ),
            },
            Found::KnowsNoCheckout(path) => (
                Kind::NoCheckout,
                format!("`{name}` does not know this checkout: no worker can be started"),
                format!("`orca repo list` names no repo at {path}"),
                add_checkout(path),
            ),
        };
        found.push(finding(kind, json!(["tool", name]), what, evidence, fix));
    }
    for Launch { checked, .. } in &host.launches {
        // A harness that is missing is one finding: its command's are the same.
        if checked.missing() {
            continue;
        }
        let default_missing = harness_missing(&host.tools);
        let faults = [
            (checked.fault.as_ref().filter(|_| !default_missing), Kind::LaunchStartsNone),
            (checked.flags_fault.as_ref(), Kind::LaunchTakesNoRole),
        ];
        for (why, kind) in faults {
            let Some(why) = why else { continue };
            found.push(finding(
                kind,
                json!(["launch", checked.harness, kind]),
                format!("the worker launch command `{}` {}", checked.command, kind.symptom()),
                why.clone(),
                "afkconvoy kit".to_string(),
            ));
        }
    }
    found
}

/// The fleet instance of this checkout is broken: why, since when, and when
/// the host runner starts it again.
fn broken_finding(run: &Run) -> Option<Finding> {
    let broken = run.broken.as_ref()?;
    let since = format!("it broke at {} UTC", log::utc(broken.since));
    let evidence = match broken.next {
        Some(next) => format!("{since}; the host runner starts it again at {} UTC", log::utc(next)),
        None => format!("{since}, and no host runner runs on this host to start it again"),
    };
    Some(finding(
        Kind::Broken,
        json!(["broken", broken.since]),
        format!("the fleet instance of this checkout is broken: {}", broken.why),
        evidence,
        named_line(&broken.why).unwrap_or_else(|| "afkconvoy run".to_string()),
    ))
}

/// No `run` is alive, and claims are held: a broken fleet instance is said
/// as one (`broken_finding`).
fn run_findings(run: &Run, held: &[Held]) -> Vec<Finding> {
    let alive = run.alive || run.broken.is_some();
    let Some(instance) = run.instance.as_ref().filter(|_| !alive && !held.is_empty()) else {
        return vec![];
    };
    let issues: Vec<u64> = held.iter().map(|held| held.issue).collect();
    let beat = match run.heartbeat_age {
        Some(age) => {
            format!("its heartbeat is {} old, and the lease is {}", lasted(age), lasted(LEASE))
        }
        None => "it has no heartbeat on the remote".to_string(),
    };
    vec![finding(
        Kind::NoRun,
        json!(["not-alive"]),
        format!(
            "no `afkconvoy run` is alive on {} for this repo, and its fleet instance {instance} \
             holds {}: nothing works them",
            run.host,
            numbered(&issues)
        ),
        format!("this host's runner holds no fleet instance of this repo; {beat}"),
        "afkconvoy run".to_string(),
    )]
}

/// A claim with no worker, a worker stopped for long, and a role or a team
/// that moved since a stage in flight started.
fn settle_wait_findings(run: &Run, held: &[Held], now: u64) -> Vec<Finding> {
    held.iter().filter_map(|claim| {
        if claim.at_work() { return None; }
        let path = claim.worktree.as_ref()?;
        let worktree = Worktree::at(path.clone(), String::new());
        let since: u64 = std::fs::read_to_string(worktree.note("convoy-settle-wait").ok()?).ok()?.trim().parse().ok()?;
        if !worker::past_grace(since, now) { return None; }
        let processes = crate::hot::working_in(path).ok()?;
        // A session still there, working or still ending, keeps the wait.
        let session = matches!(claim.worker, Some(WorkerState::Busy | WorkerState::Idle { .. }));
        if !session && processes.is_empty() { return None; }
        let state = match claim.worker {
            Some(WorkerState::Busy) => "busy",
            Some(WorkerState::Idle { .. }) => "idle",
            Some(WorkerState::Gone) => "gone",
            None => "unknown",
        };
        Some(finding(Kind::SettleWaiting, json!(["settle-waiting", claim.issue, path]),
            format!("settling has waited for its worker of #{} in {}", claim.issue, path.display()),
            format!("worker state: {state}; processes in the worktree: {}; tick-observed wait since {} UTC",
                if processes.is_empty() { "none".to_string() } else { processes.join(", ") }, crate::log::utc(since)),
            format!("afkconvoy tick --instance {}", run.instance.as_deref().unwrap_or_default())))
    }).collect()
}

fn claim_findings(run: &Run, held: &[Held], now: u64, locks: &[u64]) -> Vec<Finding> {
    let mut found = vec![];
    let instance = run.instance.as_deref().unwrap_or_default();
    for claim in held.iter().filter(|claim| claim.at_work()) {
        let issue = claim.issue;
        let place = match &claim.worktree {
            Some(worktree) => worktree.display().to_string(),
            None => "no worktree".to_string(),
        };
        match claim.worker {
            _ if keeps_off(locks, issue) => {}
            // The tick routes by the session, and leaves a worker it says is
            // at work alone: a session misread holds the claim for good.
            Some(WorkerState::Busy)
                if worker::disputed_past_grace(claim.worker, claim.orca_done, now) =>
            {
                let since = claim.orca_done.unwrap_or_default();
                let carry_on = match claim.status {
                    Status::Landing => format!(
                        "Your last turn stopped. Carry on: run afkconvoy land --issue {issue} again"
                    ),
                    _ => "Your last turn stopped. Carry on with your brief from where you stopped"
                        .to_string(),
                };
                found.push(finding(
                    Kind::WorkerDisputed,
                    json!(["worker-disputed", issue, claim.worktree]),
                    format!(
                        "the worker of #{issue}: its session says its worker works, and orca says \
                         it is done"
                    ),
                    format!(
                        "its session records a turn still open, in {place}; orca has reported \
                         its agent done since {} UTC. The tick goes by the session, and leaves \
                         the claim alone while it says so",
                        crate::log::utc(since)
                    ),
                    format!("afkconvoy tell --issue {issue} --text '{carry_on}'"),
                ));
            }
            // With no `run` alive nothing is about to start one: that is the finding.
            Some(WorkerState::Gone) if run.alive && !run.dispatching(claim.held_for, now) => {
                let (evidence, fix) = match (&claim.worktree, claim.status) {
                    (None, _) => (format!("orca links no worktree to #{issue}"), "dispatch"),
                    (Some(_), Status::Landing) => {
                        (format!("orca has no live terminal in {place}"), "restart")
                    }
                    (Some(_), _) => (format!("orca has no live terminal in {place}"), "dispatch"),
                };
                let held_for =
                    claim.held_for.map(lasted).unwrap_or_else(|| "an unknown time".into());
                found.push(finding(
                    Kind::NoWorker,
                    json!(["no-worker", issue]),
                    format!("the claim on #{issue} has no worker"),
                    format!(
                        "{evidence}; the claim was stamped {held_for} ago. A tick starts one on \
                         its own: the fix is for a claim the next tick leaves so"
                    ),
                    on(fix, issue, instance),
                ));
            }
            // Silent by the tick's own judgment, and no clock of doctor's:
            // waiting on its own background work is no silence until that
            // runs past the gate limit, which is one at once.
            Some(WorkerState::Idle { quiet }) if let Some(silent) = claim.silent => {
                let fix = match silent.command {
                    "tick" => format!("afkconvoy tick --instance {instance}"),
                    command => on(command, issue, instance),
                };
                let why = match silent.waited {
                    None if silent.declared => {
                        "it declared its verdict, and no tick has carried it out".to_string()
                    }
                    Some(waited) => format!(
                        "it waited {} on background work of its own, past the gate limit",
                        lasted(waited)
                    ),
                    None if silent.nudged => "it stayed silent after its nudge".to_string(),
                    None => "it declared nothing and showed no sign of life for the grace \
                             period"
                        .to_string(),
                };
                let stopped = quiet.map(lasted).unwrap_or_else(|| "an unknown time".into());
                found.push(finding(
                    Kind::WorkerStopped,
                    json!(["worker-stopped", issue, claim.worktree]),
                    format!(
                        "the worker of #{issue} has been stopped for {stopped}, and its claim is \
                         still at work"
                    ),
                    format!(
                        "{why}; orca reports it stopped, in {place}: look at its terminal there"
                    ),
                    fix,
                ));
            }
            _ => {}
        }
        let Some(under) = &claim.under else { continue };
        for Moved { which, started, head } in &claim.moved {
            found.push(finding(
                Kind::Moved,
                json!(["moved", issue, which, started]),
                format!(
                    "{which} moved since the stage `{}` of #{issue} started: its worker goes by \
                     the older one, and the next stage by the head",
                    under.stage
                ),
                format!("it started under {started}; the head is {head}"),
                "afkconvoy check".to_string(),
            ));
        }
    }
    found
}

/// What to do about the step `failed`: the line its error names, the same
/// transition by hand where there is one, or the plan that shows it.
fn fix_of(failed: &Failed, instance: &str) -> String {
    named_line(&failed.error).unwrap_or_else(|| match (failed.step.as_str(), failed.issue) {
        ("dispatch", Some(issue)) => on("dispatch", issue, instance),
        ("slow_gate", _) => format!("afkconvoy tick --instance {instance}"),
        _ => format!("afkconvoy plan --instance {instance}"),
    })
}

/// Failures stay visible across ticks that do not try their step. A later
/// successful attempt clears one, as does progress on the same issue by a
/// step that can continue or end its claim. An advance with nothing to do
/// clears an earlier dispatch failure: the claim has a worker to advance.
/// Boards and labels clear none.
fn recovered(failed: &Failed, step: &Value) -> bool {
    if step["outcome"] == "failed" {
        return false;
    }
    let same = step["do"] == failed.step
        && step["issue"].as_u64() == failed.issue
        && step["patrol"].as_str() == failed.patrol.as_deref();
    let same = same && step["comment"].as_str() == failed.comment.as_deref();
    let progressed = failed.issue.is_some()
        && step["issue"].as_u64() == failed.issue
        && matches!(
            step["do"].as_str(),
            Some(
                "judge"
                    | "watch"
                    | "advance"
                    | "settle"
                    | "forget"
                    | "reclaim"
                    | "dispatch"
                    | "escalate"
            )
        )
        && matches!(step["outcome"].as_str(), Some("done" | "continued" | "restarted" | "gone"));
    let started = failed.step == "dispatch"
        && step["do"] == "advance"
        && step["issue"].as_u64() == failed.issue
        && step["outcome"] == "nothing";
    same || progressed || started
}

/// Each step that failed and has not since recovered, with its latest error.
/// Three consecutive failed ticks still make its finding critical.
fn log_findings(run: &Run) -> Vec<Finding> {
    let ticks = run.ticks.as_deref().unwrap_or_default();
    let instance = run.instance.as_deref().unwrap_or_default();
    let mut pending: Vec<(u64, Failed)> = vec![];
    for (_, tick) in ticks {
        let did = tick.answer.as_ref().and_then(|answer| answer["did"].as_array());
        pending.retain(|(_, failed)| {
            !(failed.step == "tick" && tick.error.is_none()
                || did.into_iter().flatten().any(|step| recovered(failed, step)))
        });
        for failed in tick.failed() {
            pending.retain(|(_, previous)| !previous.same_step(&failed));
            pending.push((tick.count, failed));
        }
    }
    let recent = ticks.len().checked_sub(FAILED_TICKS).map(|from| &ticks[from..]);
    pending
        .into_iter()
        .map(|(count, failed)| {
            let identity = if let Some(comment) = &failed.comment {
                json!(["step", failed.step, failed.issue, failed.patrol, comment])
            } else {
                json!(["step", failed.step, failed.issue, failed.patrol])
            };
            let repeated = recent.filter(|last| {
                last.iter().all(|(_, tick)| tick.failed().iter().any(|was| was.same_step(&failed)))
            });
            match repeated {
                Some(last) => {
                    let (first, latest) = (last[0].1.count, last[FAILED_TICKS - 1].1.count);
                    finding(
                        Kind::StepFailed,
                        identity,
                        format!(
                            "`{}` failed in each of the last {FAILED_TICKS} ticks ({first} to {latest})",
                            failed.named()
                        ),
                        failed.error.clone(),
                        fix_of(&failed, instance),
                    )
                }
                None => finding(
                    Kind::StepUnrecovered,
                    identity,
                    format!("`{}` failed in tick {count} and has not recovered", failed.named()),
                    failed.error.clone(),
                    fix_of(&failed, instance),
                ),
            }
        })
        .collect()
}

/// An issue long at one stage, one rework from a breaker, or under a flow
/// label that names no workflow.
fn issue_findings(
    stood: &[Stood],
    team: &TeamRead,
    baselines: &std::collections::BTreeMap<String, (u64, usize)>,
    now: u64,
) -> Vec<Finding> {
    let mut found = vec![];
    for issue in stood {
        let number = issue.number;
        let show = format!("afkconvoy show --issue {number}");
        if let Err(why) = &issue.workflow {
            let has = match team {
                TeamRead::Loaded { flow_labels, .. } => flow_labels.join(", "),
                _ => String::new(),
            };
            found.push(finding(
                Kind::NoWorkflow,
                json!(["no-workflow", number]),
                format!("#{number} follows no workflow of the team: {why}"),
                format!(
                    "it carries {}; the team's workflows take {has}",
                    issue.flow_labels.join(", ")
                ),
                "afkconvoy flows".to_string(),
            ));
            continue;
        }
        // Only where a worker is owed: a human takes the time they take.
        let (Some(stage), Some(_)) = (&issue.stage, &issue.role) else { continue };
        let fallback = issue
            .recorded
            .windows(2)
            .map(|pair| pair[1].saturating_sub(pair[0]))
            .filter(|seconds| *seconds >= 60)
            .max();
        let baseline = baselines.get(stage).map(|(seconds, count)| (*seconds, format!("the repo p95 for `{stage}` from {count} completed stage performances in closed issues’ work accounts")))
            .or_else(|| fallback.map(|seconds| (seconds, "the longest between two earlier records on this issue, excluding gaps under a minute".to_string())));
        let here = issue.recorded.last().and_then(|last| stamp_age(*last, now));
        if let Some(((took, source), here)) =
            baseline.zip(here).filter(|((took, _), here)| here > took && *here >= STAGE_LONG)
        {
            found.push(finding(
                Kind::LongAtStage,
                json!(["long-stage", number, stage, issue.recorded.last()]),
                format!(
                    "#{number} has been at the stage `{stage}` for {}, longer than its baseline",
                    lasted(here)
                ),
                format!(
                    "its last stage record is {} old; {source} is {}",
                    lasted(here),
                    lasted(took)
                ),
                show.clone(),
            ));
        }
        let mut near = vec![];
        if let (had @ 1.., 0) = issue.rework {
            near.push(format!(
                "it had {had} rework in total, all its workflow's `max_rework` allows"
            ));
        }
        for (at, had, left) in &issue.in_a_row {
            if *had >= 1 && *left == 0 {
                near.push(format!(
                    "the stage `{at}` rejected it {had} time(s) in a row, all its workflow's \
                     `max_consecutive_rework` allows"
                ));
            }
        }
        if !near.is_empty() {
            found.push(finding(
                Kind::NearBreaker,
                json!(["near-breaker", number]),
                format!("#{number} is one rework from a breaker: the next one escalates it"),
                near.join("; "),
                show,
            ));
        }
    }
    found
}

/// A frontier nothing is dispatched from, a lock that cannot be worked, and a
/// patrol that does not run.
fn plan_findings(run: &Run, state: &State) -> Vec<Finding> {
    let mut found = vec![];
    let set = &state.set;
    let instance = run.instance.as_deref().unwrap_or_default();
    if let Some((_, tick)) = run.last_tick().filter(|_| run.alive)
        && let Some(first) = set.frontier.first()
        && set.free_slots > 0
        && !tick.dispatched()
        // A busy host starting nothing is admission at work: `host` says why.
        && !tick.held_back()
    {
        let failed = tick.failed();
        let start = failed.iter().find(|failed| failed.step == "dispatch" || failed.step == "tick");
        let left = tick
            .stood()
            .zip(tick.bound)
            .is_some_and(|((held, frontier), bound)| frontier > 0 && held < bound as u64);
        if start.is_some() || left {
            let frontier: Vec<u64> = set.frontier.iter().map(|issue| issue.number).collect();
            found.push(finding(
                Kind::NothingStarted,
                json!(["nothing-started", frontier]),
                format!(
                    "the frontier holds {}, {} slot(s) are free, and the last tick started no worker",
                    numbered(&frontier),
                    set.free_slots
                ),
                format!("tick {}: {}", tick.count, tick.line),
                match start {
                    Some(failed) => fix_of(failed, instance),
                    None => on("dispatch", first.number, instance),
                },
            ));
        }
    }
    if let Some(slow) = &set.slow_gate {
        let (stands, fix) = match slow.issue {
            Some(issue) => {
                (format!("#{issue} is open for it"), format!("afkconvoy show --issue {issue}"))
            }
            None => (
                "no issue says so yet, and the next tick opens one".to_string(),
                "afkconvoy tick --help".to_string(),
            ),
        };
        found.push(finding(
            Kind::SlowGate,
            json!(["slow-gate"]),
            format!("the gate is slow: {stands}"),
            slow_gate::said(slow),
            fix,
        ));
    }
    for lock in &set.locks {
        let claimed = set
            .mine
            .iter()
            .map(|mine| mine.number)
            .chain(set.peer_live.iter().chain(&set.stale).map(|peer| peer.number));
        let excluded = set.excluded.iter().find(|excluded| excluded.number == *lock);
        let Some(excluded) = excluded.filter(|_| !claimed.clone().any(|n| n == *lock)) else {
            continue;
        };
        let label = |label: &str| {
            format!("{} --add-label {label}", state.tracker.gh_issue("edit", *lock).own())
        };
        let fix = if excluded.reason == LabelBar::NotReady(state.ready_label.clone()).reason() {
            label(&state.ready_label)
        } else if excluded.reason == LabelBar::NoFlow.reason() {
            match &state.team {
                TeamRead::Loaded { flow_labels, .. } if flow_labels.len() == 1 => {
                    label(&flow_labels[0])
                }
                _ => "afkconvoy flows".to_string(),
            }
        } else {
            state.tracker.gh_issue("view", *lock).own()
        };
        found.push(finding(
            Kind::LockUnworked,
            json!(["lock", lock]),
            format!(
                "the fleet lock #{lock} stands and cannot be worked: the fleet works nothing \
                 until it closes"
            ),
            format!("the plan leaves it off the frontier: {}", excluded.reason),
            fix,
        ));
    }
    for patrol in &set.patrols {
        let name = &patrol.name;
        let opened: Vec<u64> = state
            .stood
            .iter()
            .filter(|issue| issue.opened_by.as_deref() == Some(name))
            .map(|issue| issue.number)
            .collect();
        let (kind, why, evidence, fix) = match (patrol.standing, patrol.failure, opened.first()) {
            (Standing::Failed, Some(failure), _) => (
                Kind::PatrolFailed,
                "its last run failed",
                format!("#{failure} reports the failure and is still open"),
                state.tracker.gh_issue("close", failure).own(),
            ),
            (Standing::Full, _, Some(oldest)) => (
                Kind::PatrolFull,
                "the open issues it opened have reached its `max_open`",
                format!("{} open of {}: {}", patrol.open, patrol.max_open, numbered(&opened)),
                format!("afkconvoy show --issue {oldest}"),
            ),
            _ => continue,
        };
        found.push(finding(
            kind,
            json!(["patrol", name]),
            format!("the patrol `{name}` does not run: {why}"),
            evidence,
            fix,
        ));
    }
    if let Some(cut) = set.cut.as_ref()
        && let Some(failed) = cut.not_cutting_here.as_ref()
    {
        // One held cut is one condition, whichever of its needs fails: it is
        // of the version the cut is due for, past its newest tag or the
        // declared initial one (sunfmin/control#570).
        found.push(finding(
            Kind::CutNeed,
            json!(["cut-need", cut.newest_tag, cut.initial]),
            failed.said(),
            failed.evidence(),
            "read its stderr file on this host, then check the repo's cut-needs declaration"
                .to_string(),
        ));
    }
    if let Some(cut) = set.cut.as_ref().filter(|cut| !cut.waiting.is_empty()) {
        found.push(finding(
            Kind::CutWaiting,
            json!(["cut-waiting"]),
            format!("{} wait on the next cut", numbered(&cut.waiting)),
            format!(
                "their stages need a version not installed yet; the cut is {}, and once one has \
                 published and installed its version they return to the frontier at the stage \
                 they left",
                cut.standing.word()
            ),
            "afkconvoy plan".to_string(),
        ));
    }
    if let Some(cut) = set.cut.as_ref().filter(|cut| cut.standing == cut::Standing::Failed)
        && let Some(failure) = cut.failure
    {
        found.push(finding(
            Kind::CutFailed,
            json!(["cut-failed", failure]),
            "the fleet cuts no version: its last cut failed".to_string(),
            format!(
                "#{failure} says which step failed, why, and the line that goes on from there, \
                 and is still open"
            ),
            format!("gh issue view {failure}"),
        ));
    }
    found
}

/// An issue has waited for a human for long: nothing in the fleet moves it.
pub(crate) fn waiting_findings(waiting: &[Waiting], now: u64) -> Vec<Finding> {
    let long = |waiting: &Waiting| {
        let waited = waiting.waited_long(now)?;
        let finding = finding(
            Kind::WaitedLong,
            HumanWait::from(waiting).identity(),
            format!(
                "#{} has waited for a human for {}: {}",
                waiting.issue,
                lasted(waited),
                waiting.why
            ),
            format!(
                "nobody holds it and no tick moves it; it is the fleet's again once it is \
                 answered, and it has waited longer than {}",
                lasted(WAITED_LONG)
            ),
            waiting.answer.clone(),
        );
        Some(finding)
    };
    waiting.iter().filter_map(long).collect()
}

/// A worktree kept on this host with no claim on its issue: what it holds
/// that the remote does not, and the line that removes it — pushing first
/// whatever only it holds.
fn unclaimed_finding(unclaimed: &Unclaimed) -> Finding {
    let (path, branch) = (unclaimed.path.display(), &unclaimed.branch);
    let remove = format!("orca worktree rm --worktree path:{path} --force");
    if let Some(owner) = &unclaimed.taken {
        let holds = unclaimed.kept.as_ref().map(Kept::holds).unwrap_or_default();
        return finding(
            Kind::TakenWorktree,
            json!(["taken", unclaimed.issue, unclaimed.path]),
            format!(
                "the worktree of #{} is kept after its claim was taken: it holds {holds}",
                unclaimed.issue
            ),
            format!(
                "{path}, on the branch {branch}; held by instance {} on host {} since {}; local work was not pushed",
                owner.instance, owner.host, owner.ts
            ),
            format!("git -C {path} log --oneline HEAD --not --remotes"),
        );
    }
    let push = format!("git -C {path} push origin HEAD:{branch}");
    let (holds, fix) = match &unclaimed.kept {
        None => ("nothing the remote does not".to_string(), remove),
        Some(kept) => {
            let fix = match kept.because {
                KeptBecause::Dirty => format!(
                    "git -C {path} add -A && git -C {path} commit -m 'Work kept from {path}' && \
                     {push} && {remove}"
                ),
                KeptBecause::Unpushed | KeptBecause::Unread => format!("{push} && {remove}"),
            };
            (kept.holds(), fix)
        }
    };
    finding(
        Kind::KeptWorktree,
        json!(["kept", unclaimed.issue, unclaimed.path]),
        format!("the worktree of #{} is kept with no claim: it holds {holds}", unclaimed.issue),
        format!("{path}, on the branch {branch}"),
        fix,
    )
}

/// An open ready issue held off by a PR not the fleet's: the
/// fleet never starts a second PR, and takes up only one of its own.
fn not_ours_finding(held_off: &NotOurs) -> Finding {
    let NotOurs { issue, pr, branch } = held_off;
    finding(
        Kind::NotOurs,
        json!(["not-ours", issue, pr]),
        format!("#{issue} is held off by a PR not the fleet's: #{pr} is open and closes it"),
        format!(
            "no claim holds it, and #{pr}'s branch {branch} is not one the fleet recorded on it"
        ),
        format!("gh pr close {pr}"),
    )
}

/// A claim on an issue that is gone: the line that deletes it, for a holder
/// whose ticks do not.
fn on_gone_finding(on: &OnGone) -> Finding {
    let OnGone { issue, claim_ref, holder } = on;
    let holder = holder.as_deref().unwrap_or("no fleet instance it names");
    finding(
        Kind::ClaimOnGone,
        json!(["gone", issue]),
        format!(
            "#{issue} is claimed but GitHub has it no more: it was deleted or transferred, or there is no such issue"
        ),
        format!(
            "GitHub answers 410 or 404 for #{issue}; {claim_ref} is held by {holder}, whose next \
             tick releases it"
        ),
        format!("git push {REMOTE} :{claim_ref}"),
    )
}

/// The team, or a role of it, has a fault at its head.
fn team_findings(team: &TeamRead) -> Vec<Finding> {
    match team {
        TeamRead::Unbound => vec![],
        TeamRead::Loaded { nothing_safe, unmatched, unmatched_models, .. } => {
            let mut found = vec![];
            found.extend(unmatched_models.iter().map(unmatched_finding));
            for (role, why) in nothing_safe {
                found.push(finding(
                    Kind::NothingSafe,
                    json!(["nothing-safe", role]),
                    format!(
                        "{role} has no harness on this host it holds limits for: its stages wait"
                    ),
                    why.clone(),
                    "afkconvoy kit".to_string(),
                ));
            }
            for (role, said) in unmatched {
                found.push(finding(
                    Kind::RosterUnmatched,
                    json!(["roster-unmatched", role]),
                    format!(
                        "the roster's entry for the role `{role}` matches nothing in the host \
                         kit: it is not followed"
                    ),
                    said.clone(),
                    format!("afkconvoy kit, or the team's roster: roster.{role}"),
                ));
            }
            found
        }
        TeamRead::Unread(why) => vec![finding(
            Kind::TeamUnread,
            json!(["team-unread"]),
            "the team this repo names cannot be read: no stage can be started".to_string(),
            why.clone(),
            "afkconvoy check".to_string(),
        )],
        TeamRead::Faulty { name, faults } => faults
            .iter()
            .map(|fault| {
                let key =
                    if fault.key.is_empty() { String::new() } else { format!(": {}", fault.key) };
                finding(
                    Kind::TeamFaulty,
                    json!(["team-fault", name, fault.file, fault.key]),
                    format!("the team {name} has a fault at its head: no stage can be started"),
                    fault.problem.clone(),
                    format!("{}{key}", fault.file),
                )
            })
            .collect(),
    }
}

/// A roster entry that names a model the host kit holds by no name: its role
/// runs no worker on it here. The team's own fleet is to correct it: a tick
/// opens an issue for it in the team's repo (`roster_issue`).
fn unmatched_finding(entry: &casting::UnmatchedModel) -> Finding {
    finding(
        Kind::RosterModelUnmatched,
        json!(["roster-model", entry.role, entry.model, entry.harness]),
        format!(
            "the role `{}` has a roster entry that names no model the host kit holds: {} on {}",
            entry.role,
            entry.model,
            entry.harness.key()
        ),
        entry.line(),
        format!("{}: roster", afkconvoy_core::team::TEAM_FILE),
    )
}

/// A config that declares no gate limit: the gate has the default's hour, and
/// nothing ends a run that has become many times slower before that.
fn config_finding(config: &Config) -> Option<Finding> {
    if config.gate_limit_declared {
        return None;
    }
    let seconds = config.gate_seconds();
    Some(finding(
        Kind::NoGateLimit,
        json!(["no-gate-limit"]),
        format!(
            "this repo's config declares no gate limit: the gate may run for {seconds}s before \
             anything ends it"
        ),
        format!("{} sets no `gate_seconds`, so the gate limit is {seconds}s", config::FILE),
        format!("{}: gate_seconds", config::FILE),
    ))
}

/// The installs this host last ran for the needs `config` declares
/// (`probe::installs`): when, how long, and their result, never their output.
fn install_findings(checkout: &Path, config: &Config) -> Vec<Finding> {
    let repo = crate::git::Repo::new(checkout.to_path_buf());
    crate::probe::installs(&repo, config)
        .into_iter()
        .map(|(kind, installed)| {
            let name = &installed.check.name;
            let (kind_of, what) = if installed.passed {
                (Kind::NeedInstalled, format!("the {kind} check {name} was installed on this host"))
            } else {
                (
                    Kind::NeedInstallFailed,
                    format!("the {kind} check {name}: its install failed on this host"),
                )
            };
            finding(
                kind_of,
                json!(["need-install", kind, name]),
                what,
                installed.said().trim_start_matches("; ").to_string(),
                format!(
                    "{}: {kind} `install` of {name}; run it in {}: {}",
                    config::FILE,
                    checkout.display(),
                    installed.check.install.as_deref().unwrap_or_default()
                ),
            )
        })
        .collect()
}

/// A repo config that still names a `concurrency`, which nothing reads any
/// more (docs/adr/0025): the fleet instance of `checkout` goes by its own on
/// this host, or the default. Nothing is held up. Where it says none of its
/// own and the config names a different number, the line that keeps that
/// number on this host comes before deleting the key.
fn retired_concurrency_finding(checkout: &Path, config: &Config) -> Option<Finding> {
    let named = config.retired_concurrency?;
    let in_force = config.concurrency();
    let said = match config.concurrency_said_by() {
        SaidBy::Checkout => format!("this checkout says {in_force} on this host"),
        SaidBy::Default => {
            format!("this checkout says none on this host, so the default, {in_force}, holds")
        }
    };
    let fix = if config.concurrency_said_by() == SaidBy::Default && named.get() != in_force {
        format!(
            "afkconvoy run {} --concurrency {named}, then delete `concurrency` from {}",
            checkout.display(),
            config::FILE
        )
    } else {
        format!("{}: concurrency", config::FILE)
    };
    Some(finding(
        Kind::RetiredConcurrency,
        json!(["retired-concurrency"]),
        format!(
            "this repo's config names `concurrency: {named}`, which nothing reads any more: this \
             checkout's fleet instance holds up to {in_force} claims at work at once on this host"
        ),
        format!("{} names `concurrency: {named}`; {said}", config::FILE),
        fix,
    ))
}

/// Each worker of the host whose worker health waits for the operator: past
/// its bound, with no class a rule or Jev settled. Without a key command the
/// fix names one; with it, the worker's screen is the operator's to read.
fn health_findings() -> Vec<Finding> {
    crate::health::waiting_for_operator()
        .into_iter()
        .map(|waiting| {
            finding(
                Kind::WorkerHealth,
                json!(["worker-health", waiting.worker]),
                format!(
                    "the worker in {}: its worker health is waiting for the operator",
                    waiting.worker.display()
                ),
                format!("{}; its screen ends: {}", waiting.reason, waiting.screen_line),
                waiting.fix,
            )
        })
        .collect()
}

/// Everything that is wrong in what `seen` holds, most severe first.
pub fn findings(seen: &Seen) -> Vec<Finding> {
    let mut found = host_findings(seen.host);
    found.extend(health_findings());
    found.extend(seen.config.and_then(config_finding));
    found.extend(seen.config.and_then(|config| retired_concurrency_finding(seen.checkout, config)));
    found.extend(
        seen.config.map(|config| install_findings(seen.checkout, config)).unwrap_or_default(),
    );
    found.extend(crate::named_harness::findings(seen.checkout, seen.now));
    found.extend(log_findings(seen.run));
    found.extend(broken_finding(seen.run));
    match seen.state {
        Ok(state) => {
            found.extend(run_findings(seen.run, &state.held));
            found.extend(team_findings(&state.team));
            found.extend(plan_findings(seen.run, state));
            found.extend(claim_findings(seen.run, &state.held, seen.now, &state.set.locks));
            found.extend(settle_wait_findings(seen.run, &state.held, seen.now));
            found.extend(issue_findings(
                &state.stood,
                &state.team,
                &state.stage_baselines,
                seen.now,
            ));
            let local: Vec<_> = state
                .set
                .waiting_on_human
                .iter()
                .filter(|wait| {
                    state.wait_holders.get(&wait.issue).is_none_or(|host| host == &seen.run.host)
                })
                .cloned()
                .collect();
            found.extend(waiting_findings(&local, seen.now));
            for wait in &state.set.waiting_on_human {
                if let Some(host) =
                    state.wait_holders.get(&wait.issue).filter(|host| *host != &seen.run.host)
                {
                    found.push(finding(
                        Kind::WaitTaken,
                        json!(["wait-taken", wait.issue]),
                        format!("#{} human wait is taken by {host}'s watch", wait.issue),
                        format!(
                            "the claim holder's heartbeat is within the {} second lease",
                            LEASE
                        ),
                        format!("leave this wait to {host}'s watch"),
                    ));
                }
            }
            found.extend(crate::blockers::findings(&state.set.unworked_blockers));
            found.extend(state.unclaimed.iter().map(unclaimed_finding));
            found.extend(state.not_ours.iter().map(not_ours_finding));
            found.extend(state.on_gone.iter().map(on_gone_finding));
        }
        Err(why) => found.push(finding(
            Kind::StateUnread,
            json!(["state-unread"]),
            "the fleet's state cannot be read: GitHub, the remote or the config".to_string(),
            why.to_string(),
            named_line(why).unwrap_or_else(|| "afkconvoy scan".to_string()),
        )),
    }
    found.sort_by_key(|finding| finding.severity);
    found
}

/// What this host has of what the fleet of the checkout `repo`, under
/// `config`, needs, read from its current host kit. When `trust` is set, a
/// startup probe that still stands already checked every harness: none is
/// started again.
fn read_host(
    repo: &Repo,
    config: Option<&Config>,
    trust: bool,
    notes: &[crate::probe::NotedHarness],
) -> Host {
    let launches = Launches::foreseen(repo);
    let default = launches.default();
    let launch = if trust { None } else { launches.of(default).ok().flatten() };
    let called = role::called_on(repo, config, default);
    let checked = if trust && !notes.is_empty() {
        // The probe already asked. Repeating it would start the harness.
        noted(&called, notes)
    } else if trust {
        trusted(&called)
    } else {
        role::check(&launches, &called).unwrap_or_default()
    };
    // Flags are this doctor's one start of the default harness when no probe
    // stands. The login look follows and does not start that program again.
    let looked = tools::look(repo, launch.as_ref());
    let launch = |checked: Checked| Launch { checked };
    Host {
        tools: looked.found,
        gh_host: looked.host,
        orca_unasked: looked.orca_unasked,
        launches: checked.into_iter().map(launch).collect(),
    }
}

/// A harness as the standing probe recorded it (`probe::NotedHarness`).
fn noted(called: &[(Harness, Vec<String>)], notes: &[crate::probe::NotedHarness]) -> Vec<Checked> {
    let kit = crate::kit::load().ok();
    called
        .iter()
        .map(|(harness, callers)| {
            let command = command_of(&kit, *harness);
            let noted = notes.iter().find(|note| note.harness() == harness.key());
            let found = match noted.map(crate::probe::NotedHarness::state).unwrap_or("ready") {
                "missing" => Some(Found::Missing),
                "logged_out" => Some(Found::LoggedOut(
                    noted.and_then(crate::probe::NotedHarness::answer).unwrap_or_default(),
                )),
                "absent" => None,
                _ => Some(Found::Ready),
            };
            Checked {
                harness: *harness,
                callers: callers.clone(),
                command: command.to_string(),
                tool: found
                    .filter(|_| harness.runs_itself(command))
                    .map(|found| (Tool::Harness(*harness), found)),
                fault: noted.and_then(crate::probe::NotedHarness::fault).map(str::to_string),
                flags_fault: noted
                    .and_then(crate::probe::NotedHarness::flags_fault)
                    .map(str::to_string),
            }
        })
        .collect()
}

/// The host kit's command for `harness`, or its stock command.
fn command_of(kit: &Option<crate::kit::Kit>, harness: Harness) -> &str {
    kit.as_ref()
        .and_then(|kit| kit.command(harness))
        .filter(|command| !command.trim().is_empty())
        .unwrap_or_else(|| harness.stock())
}

/// The host kit's commands, as a standing startup probe already checked them.
/// A note from before harness results were recorded has nothing else to say.
fn trusted(called: &[(Harness, Vec<String>)]) -> Vec<Checked> {
    let kit = crate::kit::load().ok();
    called
        .iter()
        .map(|(harness, callers)| {
            let command = command_of(&kit, *harness);
            Checked {
                harness: *harness,
                callers: callers.clone(),
                command: command.to_string(),
                // The standing probe already found this command able to start.
                // Ready is that result; asking it again would start the harness.
                tool: harness
                    .runs_itself(command)
                    .then_some((Tool::Harness(*harness), Found::Ready)),
                fault: None,
                flags_fault: None,
            }
        })
        .collect()
}

/// The team the config of the checkout `repo` names, where it is `bound`,
/// as it stands at its head → with the team itself, where it loaded: what a
/// stage in flight is held against. What its roles make of the host kit is
/// the fleet report's `roles`: none where it read none.
fn read_team(
    repo: &Repo,
    bound: bool,
    roles: Option<&crate::fleet_report::Roles>,
) -> (TeamRead, Option<team::Loaded>) {
    if !bound {
        return (TeamRead::Unbound, None);
    }
    match team::load(team::From::Config, Some(repo)) {
        Ok(loaded) => {
            let workflows = loaded.team.workflows.iter();
            let flow_labels = workflows.map(|workflow| workflow.flow_label().to_string()).collect();
            // A host kit that cannot be read is a finding of its own.
            let roles = roles.cloned();
            (
                TeamRead::Loaded {
                    flow_labels,
                    nothing_safe: roles
                        .as_ref()
                        .map(|r| r.nothing_safe.clone())
                        .unwrap_or_default(),
                    unmatched: roles.as_ref().map(|r| r.unmatched.clone()).unwrap_or_default(),
                    unmatched_models: roles.map(|r| r.unmatched_models).unwrap_or_default(),
                },
                Some(loaded),
            )
        }
        Err(error) => {
            let read = match error.downcast::<Refused>() {
                Ok(refused) => TeamRead::Faulty { name: refused.team, faults: refused.faults },
                Err(error) => TeamRead::Unread(format!("{error:#}")),
            };
            (read, None)
        }
    }
}

/// What of the team and the role a stage started `under` has moved since, in
/// `team` as it stands at its head: the team first. A role the team no longer
/// names has no head to have moved to.
fn read_moved(team: &team::Loaded, under: &Under) -> Vec<Moved> {
    let mut moved = vec![];
    if let Some(head) = team.commit.as_ref().filter(|_| team.moved(&under.team_commit)) {
        let started = under.team_commit.clone();
        moved.push(Moved { which: "the team".to_string(), started, head: head.clone() });
    }
    let role = team.team.roles.iter().find(|role| role.name == under.role);
    if let Some(role) = role.filter(|role| team.role_moved(role, &under.role_commit)) {
        moved.push(Moved {
            which: format!("the role `{}`", role.name),
            started: under.role_commit.clone(),
            head: role.commit.clone(),
        });
    }
    moved
}

/// The claims of `instance` in `set`, each with what orca and the remote say
/// of its worker. Whatever cannot be asked is left unsaid: one claim that
/// cannot be read does not cost the answer. `team` is the repo's, where it
/// loaded: what each stage in flight started under is held against it.
fn read_held(
    fleet: &Fleet,
    set: &WorkingSet,
    stood: &[Stood],
    team: Option<&team::Loaded>,
) -> Vec<Held> {
    let source = Source::of(&fleet.repo).ok();
    let gate = GateCommand::new(&fleet.config.gate_command);
    let read = |mine: &afkconvoy_core::plan::Mine| {
        let asked = source
            .as_ref()
            .and_then(|source| Worktree::worker_of(source, mine.number, fleet.now).ok());
        let (worktree, worker) = match asked {
            Some((worktree, worker)) => (worktree, Some(worker)),
            None => (None, None),
        };
        let under = worktree.as_ref().and_then(|worktree| worktree.under().ok().flatten());
        let orca_done =
            worktree.as_ref().and_then(|worktree| worktree.agent_report().ok()??.over_since());
        let green = worktree.as_ref().and_then(|worktree| {
            let tree = head_tree(&Repo::new(worktree.path.clone())).ok()?;
            fleet.recorded_gate(&tree, &gate)
        });
        let at = stood.iter().find(|issue| issue.number == mine.number);
        let stamped = fleet.scan.claim_of(mine.number).and_then(|claim| claim.owner.as_ref());
        // The one read the tick routes a stopped worker from: no clock of
        // doctor's own decides its silence.
        let silent = match (worker, stamped) {
            (Some(WorkerState::Idle { .. }), Some(owner)) => {
                InstanceId::parse(&owner.instance).ok().and_then(|instance| {
                    let me = Me { instance, host: owner.host.clone(), now: fleet.now };
                    crate::judge::silenced(fleet, &me, mine.number, mine.status).ok().flatten()
                })
            }
            _ => None,
        };
        Held {
            issue: mine.number,
            title: mine.title.clone(),
            status: mine.status,
            pr: mine.pr,
            stage: under
                .as_ref()
                .map(|under| under.stage.clone())
                .or(at.and_then(|at| at.stage.clone())),
            role: under
                .as_ref()
                .map(|under| under.role.clone())
                .or(at.and_then(|at| at.role.clone())),
            held_for: stamped.and_then(|owner| stamp_age(owner.ts, fleet.now)),
            worker,
            orca_done,
            silent,
            worktree: worktree.map(|worktree| worktree.path),
            moved: match (&under, team) {
                (Some(under), Some(team)) => read_moved(team, under),
                _ => vec![],
            },
            under,
            gate: green.map(|green| green.at()),
        }
    };
    set.mine.iter().map(read).collect()
}

/// The fleet instance the claims stamped with `host` name: the one stamped
/// last, for a host whose `run` left no log.
fn stamped_instance(fleet: &Fleet, host: &str) -> Option<String> {
    let on_issues = fleet.scan.claims.iter().filter_map(|claim| claim.owner.as_ref());
    let on_runs = fleet.scan.runs.iter().filter_map(|run| run.owner.as_ref());
    let owners = on_issues.chain(on_runs);
    let newest = owners.filter(|owner| owner.host == host).max_by_key(|owner| owner.ts);
    newest.map(|owner| owner.instance.clone())
}

/// What the fleet view holds, once it is open. The open runs beside the host
/// check: neither waits for the other.
fn finish_state(
    repo: &Repo,
    now: u64,
    run: &mut Run,
    trust: bool,
    opened: (Fleet, crate::gh::Open),
    readings: &Readings,
) -> Result<(State, FleetReport)> {
    let (mut fleet, open) = opened;
    if trust {
        fleet.trust_launches();
    }
    if run.instance.is_none() {
        run.instance = stamped_instance(&fleet, &run.host);
    }
    let instance = run.instance.as_deref().and_then(|instance| InstanceId::parse(instance).ok());
    let beat = run.instance.as_ref().and_then(|instance| fleet.scan.last_beat(instance));
    run.heartbeat_age = beat.and_then(|beat| stamp_age(beat, now));
    let beats = fleet.scan.beats();
    let live = beats.iter().filter(|(_, beat)| !stamp_stale(*beat, now));
    run.heartbeats = Heartbeats {
        found: beats.len(),
        live: live.map(|(instance, _)| instance.clone()).collect(),
    };
    let (mut set, flows, _) = fleet.rebuild(&open, instance.as_ref())?;
    // Tracker-wide reports have their own opening-host finding. Treating
    // them as ordinary escalations would wake every host serving this repo.
    set.waiting_on_human.retain(|wait| {
        open.issues.body(wait.issue).is_none_or(|body| !crate::unworked::is_report(&body.body))
    });
    let stood = flows.stood();
    let report = FleetReport::read(&fleet, &flows, &set, readings);
    let (team, loaded) = read_team(repo, fleet.config.bound.is_some(), report.roles.as_ref());
    let held = read_held(&fleet, &set, &stood, loaded.as_ref());
    let unclaimed = read_unclaimed(&fleet);
    let ready_label = fleet.config.ready_label.clone();
    let tracker = fleet.gh.tracker.clone();
    let not_ours = set
        .excluded
        .iter()
        .filter(|excluded| excluded.reason == NOT_OURS)
        .filter_map(|excluded| {
            let pr = closing_pr(&open.prs, excluded.number)?;
            Some(NotOurs { issue: excluded.number, pr: pr.number, branch: pr.head_branch.clone() })
        })
        .collect();
    let on_gone = set
        .gone
        .iter()
        .map(|&issue| OnGone {
            issue,
            claim_ref: claim_ref(fleet.repo.layout(), &fleet.scan.tracker, issue),
            holder: fleet
                .scan
                .claim_of(issue)
                .and_then(|claim| claim.owner.as_ref())
                .map(|owner| owner.instance.clone()),
        })
        .collect();
    let prefix = fleet.repo.layout().under("candidates/");
    let candidates = crate::candidates::view(
        &fleet.repo,
        &fleet.config,
        &fleet.records_at_open(&format!("{prefix}*")),
    )
    .unwrap_or_else(|error| json!({"error": error.to_string()}));
    let deliveries = report.deliveries_json();
    let state = State {
        wait_holders: crate::human_wait::holders(&fleet.scan, now),
        refusals: fleet.selection()["refusals"].clone(),
        targets: fleet.targets(&open.issues)?,
        set,
        held,
        stage_baselines: if stood.iter().any(|issue| issue.stage.is_some() && issue.role.is_some())
        {
            afkconvoy_core::accounts::stage_baselines(&fleet.gh.accounts()?)
        } else {
            Default::default()
        },
        stood,
        team,
        ready_label,
        tracker,
        unclaimed,
        not_ours,
        on_gone,
        candidates,
        deliveries,
    };
    Ok((state, report))
}

/// The worktrees on this host whose issue holds no claim, each with what it
/// holds that the remote does not. One that cannot be read is left out.
fn read_unclaimed(fleet: &Fleet) -> Vec<Unclaimed> {
    let Some(linked) = Source::of(&fleet.repo).and_then(|source| Worktree::linked(&source)).ok()
    else {
        return vec![];
    };
    let host = crate::host_settings::host_name();
    let unclaimed = linked.into_iter().filter(|(issue, _)| {
        fleet
            .scan
            .claim_of(*issue)
            .is_none_or(|claim| claim.owner.as_ref().is_some_and(|owner| owner.host != host))
    });
    unclaimed
        .filter_map(|(issue, worktree)| {
            let left = leaving_at_open(fleet, issue, &worktree).ok()?.left;
            let taken = fleet.scan.claim_of(issue).and_then(|claim| claim.owner.clone());
            if taken.is_some() && left.kept().is_none() {
                return None;
            }
            Some(Unclaimed {
                issue,
                kept: left.kept().copied(),
                taken,
                path: worktree.path,
                branch: worktree.branch,
            })
        })
        .collect()
}

fn host_json(host: &Host) -> Value {
    let tool = |(tool, found): &(Tool, Found)| {
        let state = match found {
            Found::Ready => "ready",
            Found::Missing => "missing",
            Found::LoggedOut(_) => "logged_out",
            Found::NotRunning if host.orca_unasked.is_some() => "unasked",
            Found::NotRunning => "not_running",
            Found::KnowsNoCheckout(_) => "knows_no_checkout",
        };
        let mut said = json!({"name": tool.name(), "state": state});
        // Which orca the fleet runs, and where: its calls all go there.
        if *tool == Tool::Orca && *found != Found::Missing {
            let place = crate::tools::orca_place();
            said["path"] = json!(place.as_ref().map(|(path, _)| path));
            said["from"] = json!(place.map(|(_, from)| from));
        }
        said
    };
    json!({
        "tools": host.looked_for().map(|(looked, _)| tool(looked)).collect::<Vec<_>>(),
        "worker_commands": host.launches.iter().map(|Launch { checked }| json!({
            // An unbound repo knows of no harness: nothing calls for one.
            "harness": (!checked.callers.is_empty()).then_some(checked.harness),
            "called_for_by": checked.callers,
            "command": checked.command,
            "from": "host_kit",
            "starts_a_worker": checked.fault.is_none(),
            "takes_role_flags": checked.flags_fault.is_none(),
        })).collect::<Vec<_>>(),
    })
}

fn fleet_json(run: &Run) -> Value {
    let last_tick = run.last_tick().map(|(ended, tick)| {
        let answer = tick.answer.as_ref();
        json!({"count": tick.count, "ended": ended, "next_due": ended + tick.sleep,
            "sleep": tick.sleep, "took": run.last_took, "line": tick.line, "did": answer.map(|answer| &answer["did"]),
            "error": tick.error})
    });
    let mut failed = vec![];
    for (ended, tick) in run.ticks.iter().flatten().rev() {
        let steps = tick.failed();
        for (step, count) in by_cause(&steps, |step| &step.error) {
            failed.push(json!({"tick": tick.count, "ended": ended, "do": step.step,
                "issue": step.issue, "patrol": step.patrol, "error": step.error,
                "count": count}));
        }
    }
    failed.truncate(FAILED_SHOWN);
    json!({
        "alive": run.alive,
        "instance": run.instance,
        "host": run.host,
        "heartbeat_age": run.heartbeat_age,
        "heartbeats": run.heartbeats,
        // Said in words: an agent that finds no ticks here should not look for a file.
        "log": match run.ticks.as_ref().and(run.log_file.as_ref()) {
            Some(file) => json!(file),
            None => json!(NO_LOG),
        },
        "last_tick": last_tick,
        "failed_steps": failed,
        "events": run.events,
        // Only a `run` that is alive watches its workers.
        "hot_workers": if run.alive { json!(run.hot) } else { json!([]) },
        "broken": run.broken,
    })
}

fn held_json(held: &Held, locks: &[u64]) -> Value {
    if held.at_work() && keeps_off(locks, held.issue) {
        return json!({
            "issue": held.issue, "title": held.title, "status": held.status, "pr": held.pr,
            "stage": held.stage, "role": held.role, "held_for": held.held_for,
            "worker": {"state": "waiting_on_lock", "locks": locks}, "worktree": held.worktree,
            "gate": held.gate.map(|at| json!({"green_at": at})),
        });
    }
    let disputed = worker::disputed(held.worker, held.orca_done);
    let mut worker = held.worker.map(|worker| match worker {
        WorkerState::Gone => json!({"state": "gone"}),
        WorkerState::Busy if disputed => json!({"state": "disputed", "session": "busy"}),
        WorkerState::Busy => json!({"state": "busy"}),
        WorkerState::Idle { quiet } => json!({"state": "stopped", "quiet": quiet}),
    });
    if let (Some(worker), Some(path)) = (&mut worker, &held.worktree)
        && let Ok(Some(harness)) = Worktree::at(path.clone(), String::new()).harness()
    {
        worker["harness"] = json!(harness);
        let worktree = Worktree::at(path.clone(), String::new());
        worker["orca"] = json!(worktree.agent_report().ok().flatten());
        let recorded = worktree.recorded_harness().ok().flatten();
        worker["recorded_harness"] = json!(recorded);
        worker["harness_mismatch"] = json!(recorded.is_some_and(|h| h != harness));
        if let Some(settings) =
            crate::codex_launch::recorded(&Worktree::at(path.clone(), String::new()))
        {
            worker["codex"] = json!(settings);
        }
    }
    json!({
        "issue": held.issue, "title": held.title, "status": held.status, "pr": held.pr,
        "stage": held.stage, "role": held.role, "held_for": held.held_for,
        "worker": worker, "worktree": held.worktree,
        "gate": held.gate.map(|at| json!({"green_at": at})),
    })
}

fn issues_json(set: &WorkingSet) -> Result<Value> {
    // As `plan` answers them: the same working set, said the same way.
    let plan = serde_json::to_value(set)?;
    let mut issues = json!({});
    for key in ["frontier", "excluded", "escalating", "locks", "free_slots", "patrols", "casting"] {
        issues[key] = plan.get(key).cloned().unwrap_or_else(|| json!([]));
    }
    Ok(issues)
}

/// The queue observation `afkconvoy queue` answers, from what this look
/// read of the host runner: `None` where none listens.
fn exclusive_json(said: Result<crate::queue::Said>, now: u64) -> Value {
    let observed = said.and_then(|said| {
        let lanes = crate::host_settings::of_host()?.lanes();
        Ok(crate::queue::observation(said, lanes, now))
    });
    observed.unwrap_or_else(|error| json!({"error": format!("{error:#}")}))
}

/// `afkconvoy doctor`, for the fleet of `host` in the checkout `repo`:
///
///   {"host", "fleet", "claims", "issues", "waiting_on_human": [{"issue", "title",
///    "why", "url", "since", "answer"}], "candidates", "deliveries", "findings": [{"severity", "what",
///    "evidence", "fix"}]}
///
/// An answer with a `critical` finding is an `Unwell` error: the same object,
/// and a non-zero exit. It reads only — but for the fleet's own cache, where
/// reading a team keeps its copies.
pub fn doctor(repo: &Repo, now: u64, host: &str) -> Result<Value> {
    doctor_with_proxy(repo, now, host, &[])
}

/// Adds the network settings of a start the host is considering.
pub fn doctor_with_proxy(repo: &Repo, now: u64, host: &str, proxy: &[String]) -> Result<Value> {
    doctor_with_proxy_rounds(repo, now, host, proxy, false)
}

/// Adds earlier watch rounds when `rounds` is true.
pub fn doctor_with_proxy_rounds(
    repo: &Repo,
    now: u64,
    host: &str,
    proxy: &[String],
    rounds: bool,
) -> Result<Value> {
    let _budget = crate::launch::budget_once();
    let look = Look::new(now, rounds);
    let Examined { mut answer, mut found, pace, .. } = examined(repo, now, host, proxy, &look)?;
    found.extend(overdue_findings(&[pace], now));
    found.sort_by_key(|finding| finding.severity);
    // A host finding is said here with this checkout. `doctor --all` says
    // each once, so `examined` leaves those unnamed.
    name_fixes(&repo.dir, &mut found);
    let unwell = critical(&found);
    answer["findings"] = answered(checkout_at(&repo.dir), found)?;
    if unwell {
        return Err(Unwell(answer).into());
    }
    Ok(answer)
}

/// Whether any of `found` is critical: an answer that holds one exits
/// non-zero.
fn critical(found: &[Finding]) -> bool {
    found.iter().any(|finding| finding.severity == Severity::Critical)
}

/// Name `checkout` on each afkconvoy command in `found`.
fn name_fixes(checkout: &Path, found: &mut [Finding]) {
    for finding in found {
        finding.fix = crate::checkout::about_checkout(checkout, &finding.fix);
    }
}

/// What `doctor` makes of one checkout (`examined`): its answer but for
/// `findings`, and what it found, most severe first.
struct Examined {
    answer: Value,
    found: Vec<Finding>,
    /// Its fleet instance's pace, by which a health observation is overdue.
    pace: Pace,
    /// Its fleet instance not alive, where nothing else found says so: a
    /// finding only of a checkout on the checkout list.
    not_alive: Option<Finding>,
}

/// How a fleet instance ticks (`health::Pace`), with the checkout it is of.
struct Pace {
    checkout: PathBuf,
    pace: crate::health::Pace,
}

/// The pace of `run`'s fleet instance of `checkout`, holding the claims
/// `state` read, if it read them.
fn pace_of(checkout: &Path, run: &Run, state: Option<&State>) -> Pace {
    let held = state.map(|state| state.held.as_slice()).unwrap_or_default();
    let claims = held.iter().filter_map(|held| Some((held.issue, held.worktree.clone()?)));
    let sleep = run.last_tick().map(|(_, tick)| tick.sleep).unwrap_or(0);
    let set = state.map(|state| &state.set);
    let pace = crate::health::Pace::new(sleep, run.last_took.unwrap_or(0), set, claims);
    Pace { checkout: checkout.to_path_buf(), pace }
}

/// Each worker health observation past its time and past a tick's chance to
/// make it, by the pace of every fleet instance `paces` says. A worker of a
/// checkout that has left the checkout list is none: no fleet ticks for it,
/// and the next tick of any fleet retires it once its harness has ended.
fn overdue_findings(paces: &[Pace], now: u64) -> Vec<Finding> {
    let Ok(mut health) = crate::health::show() else {
        return vec![];
    };
    let asked: Vec<&Path> = paces.iter().map(|pace| pace.checkout.as_path()).collect();
    if let Ok(listed) = crate::health::Listed::of_host(&asked) {
        health.workers.retain(|worker| !listed.left(&worker.observed.path));
        health.observations_failed.retain(|failed| !listed.left(&failed.path));
    }
    overdue_of(&health, paces, now)
}

fn overdue_of(health: &crate::health::Health, paces: &[Pace], now: u64) -> Vec<Finding> {
    use crate::health::Overdue;
    let paces: Vec<_> = paces.iter().map(|pace| pace.pace.clone()).collect();
    health
        .overdue(&paces, now)
        .into_iter()
        .map(|overdue| {
            let (path, evidence) = match overdue {
                Overdue::Waiting { path, due, error } => {
                    let error = error.map(|error| format!(": {error}")).unwrap_or_default();
                    let said =
                        format!("it was due at {} UTC and is still waiting{error}", log::utc(due));
                    (path, said)
                }
                Overdue::Retry { path, error } => {
                    (path, format!("it failed, and its retry is overdue: {error}"))
                }
            };
            finding(
                Kind::HealthOverdue,
                json!(["health-overdue", path]),
                format!("the worker in {}: its health observation is overdue", path.display()),
                format!("{evidence}; no tick of its fleet instance has made it in time"),
                "afkconvoy kit".to_string(),
            )
        })
        .collect()
}

/// A checkout on the checkout list whose fleet instance this host's runner
/// does not hold, said by no finding of a broken instance or of claims that
/// nothing works.
fn not_alive_finding(run: &Run, found: &[Finding]) -> Option<Finding> {
    let said = found.iter().any(|finding| matches!(finding.kind, Kind::Broken | Kind::NoRun));
    if run.alive || run.broken.is_some() || said {
        return None;
    }
    Some(finding(
        Kind::NotAlive,
        json!(["not-alive"]),
        "this checkout is on the checkout list, and its fleet instance is not alive: nothing \
         works its issues"
            .to_string(),
        format!("this host's runner holds no fleet instance of this repo on {}", run.host),
        "afkconvoy run".to_string(),
    ))
}

/// The host's one reading for a look, shared by every checkout.
struct Look {
    readings: Readings,
    host: Map<String, Value>,
    findings: Vec<Finding>,
    trackers: crate::tracker::Public,
}

/// What `doctor` reads of the fleet of `host` in the checkout `repo`, under
/// the network settings `proxy` names and the look's shared host reading.
fn examined(repo: &Repo, now: u64, host: &str, proxy: &[String], look: &Look) -> Result<Examined> {
    let _observation = crate::orca::Observation::begin();
    let launches = Launches::foreseen(repo).with_no_proxy(proxy)?;

    let log = Log::of(repo).ok();
    let entries = log.as_ref().and_then(Log::read);
    let bounded = exclusive_findings(entries.as_deref().unwrap_or_default(), now);
    let lines = entries.as_deref().unwrap_or_default();
    let (instance, _started_with, ticks) = run_log::run(lines, host);
    let mut run = Run {
        alive: host_runner::holds(repo),
        instance,
        host: host.to_string(),
        heartbeat_age: None,
        heartbeats: Heartbeats::default(),
        log_file: log.map(|log| log.file),
        last_took: run_log::last_took(lines, host),
        tick_started: run_log::tick_started(lines, host),
        hot: run_log::hot(lines, host),
        broken: run_log::broken(lines, host, crate::host_runner::runs()),
        events: run_log::events(lines, host),
        // A log only commands wrote to is no `run`'s.
        ticks: entries.filter(|entries| entries.iter().any(Entry::of_run)).map(|_| ticks),
    };
    let config = crate::concurrency::config_of(repo);
    // The host check and the fleet's remote read do not depend on each other.
    // Asking them together is what keeps one doctor inside its time.
    let (trust, seen_host, opened) = std::thread::scope(|scope| {
        let opened = scope.spawn(|| Fleet::read(repo, now));
        let trust = crate::probe::stands(repo, now);
        let notes = if trust { crate::probe::noted_harnesses(repo, now) } else { vec![] };
        let seen_host = read_host(repo, config.as_ref().ok(), trust, &notes);
        (trust, seen_host, opened.join().expect("fleet read"))
    });
    // A fleet that cannot be opened is the unread state, not a refusal of
    // the look: the host is still answered, and the failure is the finding.
    let read = opened
        .and_then(|opened| finish_state(repo, now, &mut run, trust, opened, &look.readings))
        .map_err(|error| format!("{error:#}"));
    let (state, report) = match read {
        Ok((state, report)) => (Ok(state), report),
        Err(error) => (Err(error), FleetReport::unopened(repo, &look.readings)),
    };
    let seen = Seen {
        now,
        host: &seen_host,
        checkout: &repo.dir,
        config: config.as_ref().ok(),
        run: &run,
        state: state.as_ref().map_err(String::as_str),
    };
    let mut found = findings(&seen);
    let pace = pace_of(&repo.dir, &run, state.as_ref().ok());
    let mut not_alive = not_alive_finding(&run, &found);
    if let Some(finding) = &mut not_alive {
        finding.fix = crate::checkout::about_checkout(&repo.dir, &finding.fix);
    }
    found.extend(bounded);
    found.extend(look.trackers.findings(repo));
    found.extend(crate::paused::findings(now));
    found.extend(host_runner::findings());
    found.extend(crate::host_watch::finding());
    found.extend(look.findings.clone());
    // A finding of this checkout names it. A host finding stays as it was
    // read, so `doctor --all` can say the same one once.
    for finding in found.iter_mut().filter(|finding| !finding.of_host) {
        finding.fix = crate::checkout::about_checkout(&repo.dir, &finding.fix);
    }
    found.sort_by_key(|finding| finding.severity);
    let (claims, issues, waiting) = match &state {
        Ok(state) => (
            state.held.iter().map(|held| held_json(held, &state.set.locks)).collect(),
            issues_json(&state.set)?,
            Value::Array(
                state
                    .set
                    .waiting_on_human
                    .iter()
                    .map(|wait| {
                        let mut value = json!(wait);
                        if let Some(host) =
                            state.wait_holders.get(&wait.issue).filter(|host| *host != &run.host)
                        {
                            value["taken_by"] = json!(host);
                        }
                        value
                    })
                    .collect(),
            ),
        ),
        Err(_) => (vec![], Value::Null, Value::Null),
    };
    let mut host = host_json(&seen_host);
    host["worker_no_proxy"] = launches.proxy_json();
    host["admission"] = report.admission_json();
    host["settings"] = match List::of_host() {
        Ok(list) => crate::host_settings::json(&list, &repo.dir, &look.readings),
        Err(error) => json!({"error": format!("{error:#}")}),
    };
    if let Some(host) = host.as_object_mut() {
        host.extend(look.host.clone());
    }
    let mut fleet = fleet_json(&run);
    fleet["concurrency"] = report.concurrency.clone();
    fleet["harness"] = report.harness.clone();
    fleet["github_api"] = match crate::gh::GitHub::of(repo).and_then(|gh| gh.quota_status()) {
        Ok(status) => status,
        Err(error) => json!({"error":format!("{error:#}")}),
    };
    fleet["tracker"] = report.tracker.clone();
    let deliveries = match &state {
        Ok(state) => state.deliveries.clone(),
        Err(error) => vec![json!({ "error": error })],
    };
    let answer = json!({
        "host": host,
        "fleet": fleet,
        "claims": Value::Array(claims),
        "codex_workers": crate::codex_launch::workers(repo),
        "deliveries": deliveries,
        "issues": issues,
        "targets": state.as_ref().map(|state| state.targets.clone())
            .unwrap_or_else(|error| json!({"error": error})),
        "refusals": state.as_ref().ok().map(|state| &state.refusals),
        "waiting_on_human": waiting,
        "candidates": state.as_ref().ok().map(|state| &state.candidates),
    });
    Ok(Examined { answer, found, pace, not_alive })
}

/// Each recent bound outcome is evidence for splitting the work or checking
/// the host setting. Read commands too: they need no live fleet instance.
fn exclusive_findings(entries: &[Entry], now: u64) -> Vec<Finding> {
    entries
        .iter()
        .filter_map(|entry| {
            let What::Command { command, outcome, .. } = &entry.what else { return None };
            if command != "exclusive"
                || !outcome.starts_with("ended at the exclusive bound")
                || now.saturating_sub(entry.at) > log::KEPT_FOR
            {
                return None;
            }
            Some(finding(
                Kind::ExclusiveBound,
                json!(["exclusive-bound", entry.at]),
                format!("an exclusive command ended at the exclusive bound at {}", entry.at),
                format!(
                    "{}: {outcome}; check `afkconvoy run --exclusive-seconds` if this was one step",
                    entry.host
                ),
                "afkconvoy exclusive -- <one step of the work>".into(),
            ))
        })
        .collect()
}

/// Watch log lines are the host's. `doctor --all` says each once.
fn lift_watch_events(answer: &mut Value, lifted: &mut Vec<Value>) {
    let Some(events) = answer.pointer_mut("/fleet/events").and_then(Value::as_array_mut) else {
        return;
    };
    let mut own = Vec::new();
    for event in events.drain(..) {
        if event.get("event").and_then(Value::as_str) == Some("watch") {
            if !lifted.contains(&event) {
                lifted.push(event);
            }
        } else {
            own.push(event);
        }
    }
    *events = own;
}

/// What of `doctor`'s `host` is the host's alone, the same whichever
/// checkout it is asked in — the host kit, the queue of `exclusive`, the
/// watch running, the newer afkconvoy the host runner moves onto, the host
/// runners of the other accounts on the machine — by
/// its key: `doctor --all` says it once. All but the host kit and the other
/// accounts, read from the process table, are as the host runner says
/// them, asked once.
fn host_once(
    now: u64,
    readings: &Readings,
    rounds: bool,
    others: &Result<Vec<crate::machine::Runner>>,
    elsewhere: Option<&afkconvoy_core::resources::Elsewhere>,
) -> Map<String, Value> {
    let (exclusive, installed, watch) = match host_runner::shared() {
        Ok(Some(said)) => {
            (Ok(Some((said.exclusive, said.exclusive_paused))), said.installed, said.watch)
        }
        Ok(None) => (Ok(None), None, None),
        Err(error) => (Err(error), None, None),
    };
    Map::from_iter([
        ("host_kit".to_string(), crate::kit::show_for_look(now)),
        (
            "resources".to_string(),
            crate::resources::json(now, readings, elsewhere)
                .unwrap_or_else(|e| json!({"error": format!("{e:#}")})),
        ),
        ("exclusive".to_string(), exclusive_json(exclusive, now)),
        ("installed".to_string(), serde_json::to_value(installed).unwrap_or_default()),
        ("watch".to_string(), crate::host_watch::json(watch.as_ref(), rounds)),
        ("asking".to_string(), crate::host_asking::json()),
        ("notify".to_string(), crate::watch_notify::json()),
        ("daybook".to_string(), crate::watch_daybook::json()),
        ("other_accounts".to_string(), crate::machine::json(others)),
    ])
}

impl Look {
    fn new(now: u64, rounds: bool) -> Look {
        let readings = Readings::new();
        let table = crate::machine::Table::read();
        let others = crate::machine::other_runners(&table);
        let elsewhere = table.as_ref().ok().and_then(crate::machine::Table::elsewhere);
        let host = host_once(now, &readings, rounds, &others, elsewhere.as_ref());
        let mut findings = crate::resources::findings(&host["resources"], now);
        findings.extend(crate::machine::findings(&others, host_runner::runs()));
        findings.extend(crate::machine::path_findings());
        findings.extend(crate::kit_research::findings());
        findings.extend(crate::agent_setup::findings());
        findings.extend(crate::tmux::findings());
        findings.extend(crate::watch_notify::findings());
        findings.extend(crate::watch_daybook::findings());
        Look { readings, host, findings, trackers: crate::tracker::Public::default() }
    }
}

/// `afkconvoy doctor --all`: `doctor` of every checkout on this host's
/// checkout list, in its order, each asked on a thread of its own, as the
/// fleet of `host` — else of the host its `run` named, else of this machine
/// — under the network settings `proxy` names:
///
///   {"host": {"host_kit", "exclusive", "watch", "installed", "other_accounts"}, "findings",
///    "checkouts": [{"checkout", "repo", "doctor"}]}
///
/// What is the host's is said once, at the top: the part of `host` that is
/// (`host_once`), and each finding that is (`Kind::of_host`), however many
/// checkouts found it. Each checkout's `doctor` is the rest of what `doctor`
/// answers there, `waiting_on_human` among it; or {"error", "see"} where it
/// could not be asked — the checkout is no git checkout any more, say. An
/// answer with a critical finding, or a checkout that could not be asked,
/// is an `Unwell` error.
pub fn all(now: u64, host: Option<&str>, proxy: &[String]) -> Result<Value> {
    all_since_rounds(now, host, proxy, None, false)
}

/// Adds the interval report when `since` is named. Every checkout uses the
/// same upper boundary, captured before any reads; a failed interval read
/// withholds that boundary so it cannot silently skip unread events.
pub fn all_since(
    now: u64,
    host: Option<&str>,
    proxy: &[String],
    since: Option<u64>,
) -> Result<Value> {
    all_since_rounds(now, host, proxy, since, false)
}

/// Adds earlier watch rounds when `rounds` is true.
pub fn all_since_rounds(
    now: u64,
    host: Option<&str>,
    proxy: &[String],
    since: Option<u64>,
    rounds: bool,
) -> Result<Value> {
    if since.is_some_and(|since| since > now) {
        anyhow::bail!("--since is after this look's time: use the previous answer's `until`");
    }
    let _budget = crate::launch::budget_once();
    let listed = List::of_host()?.read()?.checkouts;
    let look = Look::new(now, rounds);
    // Each checkout's repo, where it can be read, beside what was made of it.
    let asked: Vec<(Option<String>, Result<Examined>)> = std::thread::scope(|scope| {
        let look = &look;
        let asking: Vec<_> = listed
            .iter()
            .map(|entry| {
                let named = host.map(str::to_string).or_else(|| entry.named.host.clone());
                let host = named.unwrap_or_else(crate::this_host);
                scope.spawn(move || {
                    let repo = Repo::at(&entry.checkout);
                    let key = repo.as_ref().ok().and_then(|repo| repo_of(repo).ok());
                    let examined = repo.and_then(|repo| {
                        let mut examined = examined(&repo, now, &host, proxy, look)?;
                        if let Some(since) = since {
                            examined.answer["watching"] =
                                match watching::read(&repo, &host, since, now) {
                                    Ok(watching) => watching,
                                    Err(error) => crate::error_json(&error),
                                };
                            let ticks = watching::tick_findings(
                                &examined.answer["watching"],
                                &examined.found,
                            );
                            examined.found.extend(ticks);
                            let goals = watching::goal_findings(&examined.answer["watching"]);
                            examined.found.extend(goals);
                        }
                        Ok(examined)
                    });
                    (key, examined)
                })
            })
            .collect();
        let panicked = || (None, Err(anyhow!("its `doctor` panicked")));
        asking.into_iter().map(|asked| asked.join().unwrap_or_else(|_| panicked())).collect()
    });
    let mut once = look.host;
    let mut of_host = look.findings;
    let mut watch_events = vec![];
    let say_watch_once = once.get("watch").is_some_and(Value::is_object);
    let mut unwell = false;
    let mut complete = true;
    let mut checkouts = vec![];
    let mut paces = vec![];
    for (entry, (key, examined)) in listed.iter().zip(asked) {
        let doctor = match examined {
            Ok(Examined { mut answer, mut found, pace, not_alive }) => {
                complete &= answer["watching"]["error"].is_null();
                paces.push(pace);
                // Every checkout here is on the checkout list.
                found.extend(not_alive);
                found.sort_by_key(|finding| finding.severity);
                let (host_found, own): (Vec<Finding>, Vec<Finding>) =
                    found.into_iter().partition(|finding| finding.of_host);
                for finding in host_found {
                    if !of_host.contains(&finding) {
                        of_host.push(finding);
                    }
                }
                unwell |= critical(&own);
                if let Some(host) = answer["host"].as_object_mut() {
                    host.retain(|key, _| !once.contains_key(key));
                }
                // The watch is the host's. Its log lines are said once, on
                // `host.watch`, not again in every checkout.
                if say_watch_once {
                    lift_watch_events(&mut answer, &mut watch_events);
                }
                answer["findings"] = answered(checkout_at(&entry.checkout), own)?;
                answer
            }
            Err(error) => {
                unwell = true;
                complete = false;
                crate::error_json(&error)
            }
        };
        checkouts.push(json!({"checkout": entry.checkout, "repo": key, "doctor": doctor}));
    }
    watch_events.sort_by(|left, right| right["at"].as_u64().cmp(&left["at"].as_u64()));
    if say_watch_once && let Some(watch) = once.get_mut("watch").and_then(Value::as_object_mut) {
        watch.insert("events".to_string(), Value::Array(watch_events));
    }
    of_host.extend(overdue_findings(&paces, now));
    let unworked = match crate::unworked::read(now) {
        Ok(read) => read,
        Err(error) => crate::error_json(&error),
    };
    once.insert("unworked_repos".into(), unworked);
    of_host.extend(crate::unworked::findings(
        &once["unworked_repos"],
        host.unwrap_or(&crate::this_host()),
        now,
    ));
    // Said once, named for the first checkout on the list: the lines are
    // equal while they are gathered, and each runs from any directory.
    if let Some(checkout) = listed.first() {
        name_fixes(&checkout.checkout, &mut of_host);
    }
    of_host.sort_by_key(|finding| finding.severity);
    unwell |= critical(&of_host);
    unwell |= !complete;
    let answer = json!({
        "until": complete.then_some(now),
        "host": once,
        "findings": answered("host", of_host)?,
        "checkouts": checkouts,
    });
    if unwell {
        return Err(Unwell(answer).into());
    }
    Ok(answer)
}

/// The checkout `doctor --all` names a checkout's answer by, `checkout`: as
/// the watch reads it there.
pub(crate) fn checkout_at(checkout: &Path) -> &str {
    checkout.to_str().unwrap_or("unknown")
}

/// What `afkconvoy doctor --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When something looks wrong, and before anything else: a fleet that seems stuck, an issue nobody works, a worker that sits there — or when you are asked "is it working?". Run it before you file a bug, too, and put its answer in the report. It is one command for what `scan`, `takeover --list`, `plan`, `show` and orca each say a part of, and it is the only one that says what is wrong.

With `--all`, for every repo of this host at once: each checkout on the host's checkout list, as `doctor` in it answers. A session that looks after every repo of the host runs it, beside `afkconvoy checkouts`, and keeps no list of its own.

## Who runs it

The operator, in a checkout of the repo, on the host the fleet runs on. No `--instance` is needed: it finds the fleet instance of this host's `run` itself. With `--all`, anywhere on that host.

## What it reads and writes

It reads this host (the commands the fleet needs, in a login shell; orca), the log this host's `run` keeps of its ticks, the open issues, PRs and stage records on GitHub, whether GitHub says the tracker is public, the claims, heartbeats and recorded gate runs on the remote, and orca after the worker of each claim. It writes nothing on GitHub and nothing on the remote, and makes no startup probe: it is safe beside a live fleet, as often as you like. It fixes nothing either: each finding names the command that does.

A harness whose first login answer reads logged out is asked once more in that same login shell setting before `tools` reports `logged_out`; ready on the second answer is ready. A ready harness is still started at most once per doctor. A tool finding's evidence carries the last answer's exit status and the first 500 characters it printed, with credential-shaped text redacted.

With `--all` it reads the checkout list (checkouts.json in the cache directory) and the same of each checkout on it, all at once.
It also reads every tracker that list names once for repo-labelled work, including repos absent from this host. A repo nobody works has an open ready issue with no open blocker or claim, ready for more than an hour since the later of its dispatch label being applied and its last blocker closing, and no remote heartbeat within the 1800-second lease. Ready means the repo's ready label, or its team's flow labels. Only an old enough unblocked issue, or an existing report, asks that repo's heartbeat remote. `host.unworked_repos` holds the waiting issues and their seconds, newest heartbeat's host and time (or null), report and opening host, recovery host, and read error. An unreadable remote proves nothing about whether a fleet works it. Doctor opens and closes nothing; the host runner reconciles these reports at its look.

## What it answers

{"host", "fleet", "claims", "issues", "waiting_on_human", "refusals", "candidates", "findings", "deliveries"}

`targets` lists open tracker tasks with issue, tracker_issue, delivery (null when unattached), target_branch, configured_base and pinned; an invalid record or missing branch adds error rather than selecting the configured base. A failed listing is {error}.

`deliveries` lists each Jira delivery with its final PR or pending reason, whether recorded delivery evidence certifies the current delivery head, final-target head and what each attached task came to, and any settlement. Reading writes nothing.

- `host`: what this host has of what the fleet needs:

  - `tools`: each command needed — a harness only where a worker launch command the fleet could be asked for runs it by name: {name, state} — state is ready, missing or logged_out; of orca also not_running, knows_no_checkout (the checkout worktrees are cut from was never added to it) or unasked, and, found, `path`, the orca every call goes to, and `from`: `path`, the first on the PATH, or `remote_cli`, the CLI of Orca's SSH remote runtime at `~/.orca/cli/bin/orca`
  - `worker_commands`: the host kit commands and their callers. The first eligible kit harness is selected for each role. A worker's model is its model label where followed, otherwise that harness's user default.
  - `host_kit`: {path, refreshed, harnesses, paused, worker_health}: where the kit is kept; when it was last written, in epoch seconds — by `kit`, by `run`, or by a tick's daily refresh (`afkconvoy kit --help`); each harness in order, first first, with its full worker launch command, its models in order as {model, names}; `default_model` (it has no placed model, so a worker runs its login's default, with no model flag) and `research_pending` (it holds a model awaiting research). good_at, settled, sources and researched_at stay on the kit and on `plan`'s `host_kit` (`afkconvoy plan --help`); a look does not repeat them. `paused` is each harness paused on this host: {harness, since, why, probed, said}. `worker_health` is the book a worker-health finding is read from. {path, error} when unreadable. `afkconvoy kit` writes it again.

    {const:KIT_RESEARCH_STATES}

    A harness with `models` [] or no placed model runs on its default model with no model flag, and is no fault. Automatic command discovery, a switch, failed research or a hand-written kit can leave one. `afkconvoy kit` lists them; the watch supplies research through `afkconvoy kit --research <file>`. A tick lists an empty Codex entry without waiting for the daily refresh, with an hour between unsuccessful tries. A roster model or a model label that matches nothing placed is still reported as such.
  - `settings`: the host settings (`afkconvoy run --help`): {file, said, in_force} — each by its key, the name of the flag of `run` that names it (`hot_cores` for `--hot-cores`), in that flag's units: said is what the host says, null where it says none and the default holds; in_force what holds for this repo's fleet on this host, each default worked out for it — the panel's port {const:PANEL_PORT} where the host says none. {file, error} where the file cannot be read
  - `admission`: whether a tick would start a new worker on this host now, the thresholds in force and what the host reads: as `plan` answers it (`afkconvoy plan --help`), or {error} where a share the host says is wrong. A tick that admission held back is no "started no worker" finding.
  - `exclusive`: the host's exclusive queue as the one observation `afkconvoy queue` answers — {observed_at, status, paused, worker, repos, running, waiting} — at doctor's observation time, from the host runner this look asked once: worker capacity, each repo lane, and every request with its `lane`, durations and scheduling exactly as `afkconvoy queue --help` documents. A request not yet started is in `waiting`, with its scheduling state; it is not marked `waiting_for_capacity`, as a landing turn's answer is. Each `command` is the one line `afkconvoy queue --help` describes: the program and the arguments that say what is running. A script is not shown as the script: a `sh -c` test script is named by its `cargo nextest` line, or by its `afkconvoy exclusive` invocation. The line carries no `NAME=value`. `status` is `no_runner`, with empty arrays, where no host runner listens; `paused` is null where an older host runner does not say it. {error} when the host runner cannot say it, never an empty queue. `afkconvoy queue` reads just the local queue, without this full diagnosis
  - `resources`: resource episodes observed by the host runner, with stable ids, active conditions, since, episode, worker or host readings and evidence. Admission and pressure retain their identities during five minutes of recovery. A resource finding's watch_id is that episode identity, covered by a running or reported watch. Pressure also says `elsewhere`, the other user account holding the most resident memory, more than this one, from the process table — {account, uid, memory_bytes, processes}, null where this account holds the most; `workers`, how many workers this host's fleets' samplers watch; `left`, the other account a reported watch left pressure with the operator for — {account, uid, episode, watch} — until they settle its question or the largest holder changes; and `unwatched`: why it starts no watch — `no_worker` while another account holds the most and no worker runs, `left_with_operator` while it is that same left account's — or null. Pressure that starts no watch is an `info` finding naming the account and its memory.
  - `watch`: this host's watch (`afkconvoy run --help`, WATCH): {directory, stock, running, last, unavailable, association}. directory is where the next watch runs and stock whether it uses afkconvoy's instruction. running and last are each {id, directory, stock, items, started, harness, model, terminal, transcript, did, reported, report, attempts, ended, outcome}. items says why it started; transcript holds the session's exit status, not its printed output. reported is whether it called `watched`; report is its complete {held_back, did, verified, human, pending} record, null if it did not report; did is that report's actions. attempts holds each session whose harness gave out: {harness, model, terminal, why, did, screen}; screen is terminal evidence for continuation, never a report. outcome is ended (reported), not_reported (exited without reporting), closed, time_limit or no_harness. running and last are null with none. unavailable says why no harness could start or continue the last requested watch, null where one could. Earlier rounds are left out; `--rounds` adds `rounds`, each the same shape as `last`. `last` keeps the last round's report. `doctor --all` says each watch log line once, as `events` on this object — {at, event, cause, instance}, newest first — and leaves those lines out of every checkout's `fleet.events`.
  - `asking`: this host's resident asking session (`afkconvoy run --help`, ASKING SESSION): {session, status, current, pending, unavailable, association}. session is {id, directory, harness, terminal, status}, null when no question has started one; status is absent, standby, asking, unavailable or uncertain. current is the question being delivered or discussed, with its condition episode, watch, human question and answer, and its delivered, withdrawn and put-aside flags; pending holds those waiting, including questions put aside. association shows the titled Terminal's launch and delivery proof. The session is durable across host runner restarts and upgrades.
  - `installed`: the release installed where the host runner was started from, where the host runner moves onto it or did not: {path, version, from, since, wait_expired, held_back, failed} — the file; the release it says, null where it says none; the host runner's own release; when that file answered, in epoch seconds; whether the {const:MOVE_WAIT_SECONDS}s wait expired; what holds the move back, each in words ("/Users/me/Developments/hkj is in a tick", "1 exclusive command(s) running"), [] once nothing does, or the last blockers when the wait expired; and why the move failed — it did not start, or does not say its version — null until one did. null with none: nothing newer is installed there, or no host runner runs (`afkconvoy run --help`)
  - `published`: a release newer than the host runner's, published to the Homebrew tap it was installed from, which it has not moved onto yet: {formula, version, installed, from, since, failed} — the formula with its tap (`sunfmin/tap/afkconvoy`); the newest release the tap publishes; the release Homebrew has installed where the host runner was started from; the host runner's own; when a look first found it, in epoch seconds; and why the last `brew upgrade` failed, null until one did. While it stands, a finding names the installed release and the published one. null with none: the host runner runs the newest, it was installed from no tap, or no host runner runs (`afkconvoy run --help`)
  - `other_accounts`: the host runner of every other account on this machine, each the host runner of a host of its own: [{account, uid, pid, program, cores, memory_bytes, processes}] — the account and its uid; the host runner's pid and the file it was started from; and what every process of that account uses of the machine now: cores, their %CPU summed over 100; memory_bytes, resident; and how many processes. Read from the process table alone, which any account may read: nothing of the other account's files, socket or checkouts. [] where there is none; {error} where `ps` could not say. Where this account has no host runner of its own, each is also a finding beside `exclusive`'s `no_runner`
- `fleet`: the `run` of this host for this repo:

  - `alive`: whether one is running now: this host's runner, asked on its socket naming this checkout, holds a fleet instance of this repo
  - `instance`, `host`: its fleet instance id: the one its log names, else the one the claims stamped with this host name; null when this host never ran one
  - `heartbeat_age`: seconds since its heartbeat; null: it has none
  - `heartbeats`: {found, live}: how many heartbeats the remote has, whoever stamped them, and the instance ids of those within the lease. A `run` told to stop removes its own, and a start removes the dead ones that hold nothing: more than one found and not live is fleets nobody started again
  - `log`: the file its ticks are logged in, or "no log": no `run` of this version ran here
  - `last_tick`: {count, ended, next_due, sleep, took, line, did, error}: when its last tick ended and the next is due, in epoch seconds, the seconds it slept after it and took from its start to its end (null where its log has no start of it), the line `run` printed, and what `tick` answers as `did`; error, when the tick failed whole. null with no log
  - `failed_steps`: the last {const:DOCTOR_FAILED_SHOWN} failures, newest first: {tick, ended, do, issue, patrol, error, count}. One cause that stopped several steps of a tick is one failure: count is how many, and do, issue and patrol the first's
  - `events`: the last {const:DOCTOR_EVENTS_SHOWN} events that reached a `run` of this host, newest first, whichever instance: {at, event, cause, instance} — event being wake (between ticks, or during one), start, stop (told to, by SIGTERM or SIGINT), broken, back, moved or not_moved (the host runner moved onto a newer afkconvoy, with both releases, or that one did not start: `afkconvoy run --help`), host_wake, watch (a thing a watch did, and how it ended: `afkconvoy run --help`) or cut (a cut started, and how it ended: `afkconvoy cut --help`); cause says what caused it, in words. [] with no log
  - `broken`: {why, since, next, instance}: its fleet instance is broken (`afkconvoy run --help`) — why, as it last broke; since when, and when the host runner starts it again, in epoch seconds, null where no host runner runs on this host; and the fleet instance that broke, "" where none had started. null: it is not broken
  - `concurrency`: {in_force, said_by}: how many claims its fleet instance holds at work at once on this host, as `plan` says it — said_by `checkout` or `default`. {error} where the config or the checkout list cannot be read
  - `tracker`: the repo its fleet reads and writes its issues in, as `afkconvoy checkouts` says it: null where the checkout says none, and its own repo is its tracker. {error} where the checkout list cannot be read
  - `github_api`: {host, login, core, graphql, secondary, for_rest_reads, for_graphql_reads}. Each quota has {remaining, reset}; null remaining means unknown and reset is Unix seconds. Selection respects primary exhaustion until reset and shared secondary cooldowns; fallback requires equivalent facts. {error} when the observation cannot be read. [`afkconvoy help troubleshooting`](help:troubleshooting) explains capability refusals and write receipts.
- `claims`: each claim of that instance: {issue, title, status, pr, stage, role, held_for, worker, worktree, gate} — status as `plan` gives it; stage and role in a bound repo; held_for in seconds; worker is {state, quiet}, state being busy, stopped or gone, and null when orca could not say; a session that says busy while orca reports the agent done says disputed instead, with session: "busy", and raises a finding once orca has said so for the grace period. A claim left alone by a fleet lock instead says {state: "waiting_on_lock", locks: [issue numbers]} and raises no stopped or missing worker finding; the lock's own worker keeps its findings. gate is {green_at} when a green gate run is on record for the tree the worktree is at, else null
- `issues`: as `plan` answers them: {frontier, excluded, escalating, locks, free_slots, patrols} — what a worker can be started on, what waits and why, and what is being handed to a human and why. null when they could not be read
- `refusals`: issue number → the harnesses that refused that task, each with `harness` and `evidence`, read from GitHub. Later starts skip them across every stage; the harness stays healthy for other issues.
- `candidates`: the remote candidate chains for every target branch of this repo, each in order and identified by `target_branch`. Each candidate is {phase, commit, issue, pr, predecessor, waiting, target_branch, id, cause, cancelled_by, round}. `round` is the id of the candidate whose slow gate it waits on: the tip of the round now slow-gating, for each candidate of that round and the tip itself; null for one behind it or with no round running, which waits for the next round. `waiting` is `constructing_worktree`, `validating`, `waiting_for_round` (fast-green, until a round's slow gate lands it), `slow_gating` (the round's tip, running the slow gate), `waiting_for_predecessor`, `confirming_process_exit`, `gate_red`, `cancelled` or `landed`. A finished candidate — phase `landed`, `red` or `cancelled` — carries no gate excerpt. A candidate gating now (phase `validating` or `slow`) may still carry `excerpt`. An error reading the chain is reported as `{error}`.
- `codex_workers`: this checkout's Codex sessions, each with issue, worktree, harness, model, effort, model_source, effort_source and codex_home (its private session directory). Automatic starts read the model from your `~/.codex/config.toml` (`user_selection`) unless a model label is followed (`model_label`; a stored `casting` is read as that), and no user preference leaves the model null for Codex's native default (`codex_default`). Every start reads the effort from your own Codex config, or uses Codex's model default. Sources stay attached to the session after your config changes; a saved `/model` choice is shown with its session's private config as the source. [`afkconvoy help worker-launch`](help:worker-launch).
- `waiting_on_human`: as `plan` answers it, and as the panel lists it under "With a human": every open issue nobody holds that waits for a human — escalated, at a stage that is a human's, past the end of its workflow and still open, or reporting a patrol's failed run — each as {issue, title, why, url, since, answer}: `why` is what kind of wait, then the reason the issue’s comment gives, in its first sentence; `url` is that comment (the report itself for an older failed patrol or cut with no reason comment), or null where no comment explains the wait; `answer` is the line that answers it, run as printed apart from what stands between < and >, naming the checkout as that command takes it so the line runs from any directory. An escalation with no position gets the team's only flow label filled in; with several workflows, `why` lists the flow labels to choose `<flow label>` from. null when the issues could not be read
- `findings`: what is wrong, most severe first — none: []. Each is {kind, severity, what, evidence, fix, see, watch_id}:
  - `kind`: what it is a finding of, in snake_case: the troubleshooting row it is headed by (`no_run`, `step_unrecovered`, `health_overdue`)

  - `severity`: how much it matters, one of those below
  - `what`: the fault, in a sentence
  - `evidence`: what was read that says so
  - `fix`: one command line, run as printed apart from what stands between < and > — it names the checkout as the command takes it, so the line runs from any directory, and `afkconvoy doctor --all` is run from anywhere — or, for a fault in a team's or a role's file, or a key the repo's config leaves out, `<file>: <key>` to edit. For an escalation with no position and several workflows, `what` lists the flow labels to choose `<flow label>` from
  - `see`: the command that prints the help topic explaining it: `afkconvoy help humans` where what it reports waits for a human, else `afkconvoy help troubleshooting`, which has a row for each finding
  - `watch_id`: the condition's identity, made where it is judged and scoped to where it was found: the same while the condition lasts, whatever `what` says of it. `finding:<checkout>:` is one of that checkout, `finding:host:` one of the host, `kit-research:` a harness's unplaced models in the host kit; a resource's is its episode identity. The host's watch acts on each finding but an `info` one, or its own, by `kind` and `watch_id` as doctor says it, never by its words

A finding's `severity` is one of, most first:

{table:severity}

It exits 0 with no critical finding, and non-zero with one — printing the same object, not {"error"} — so that a script can gate on it. Errors: "is not in a git checkout", or git's own words for a checkout it refuses. Whatever else cannot be read is itself a finding.

With `--all`:

{"host": {"host_kit", "exclusive", "watch", "notify", "daybook", "resources", "installed", "other_accounts"}, "findings", "checkouts": [{"checkout", "repo", "doctor"}], "until"}

- `host`: what of `host` is the host's alone, as above, said once
- `findings`: the host's findings, said once however many checkouts found them, most severe first: a command missing or logged out, orca not running, a harness paused, the host runner not moved onto a newer afkconvoy, a worker health observation overdue by the pace of every fleet instance on the host. A checkout's own findings add, under `--all`, its fleet instance not alive, and, with `--since`, each tick of the interval that failed with what no unrecovered step's finding says
- `checkouts`: each checkout on the list, in its order — [] with none — with the repo its remote `origin` names, null where it is no git checkout any more, and its `doctor`: what `doctor` answers in it — `waiting_on_human` among it — but for the host's, which are above. Each is asked as the fleet of the host its `run` named, unless `--host` names one. {"error", "see"} where it could not be asked
- `until`: this look's boundary in epoch seconds, captured before reading any checkout. Pass it as the next `--since`. Null when a checkout or its interval could not be read: keep the previous boundary and try again.

With `--all --since <time>`, each checkout's `doctor.watching` adds what happened on this host and on GitHub. Time is epoch seconds or UTC `YYYY-MM-DDTHH:MM:SSZ`. The interval includes `since` and excludes `until`: events stamped in the current second are left for the next look, so adjacent looks lose no step and repeat none. Without `--since`, no interval is read and the state and findings are answered as usual.

- `since`, `until`: the interval's boundaries, in epoch seconds.
- `issues`: each issue that moved, and each issue with an open PR, as {issue, title, pr, steps, last_step}. `steps` are oldest first, each {at, source, step, detail}; source is tick_log or github. Exclusive queue counts, waits, exit codes and run times are in detail, as logged. `last_step` adds `ago`, seconds since the last step, including a step before `since`; null if neither source has one. A queued, running or red gate therefore explains a PR quiet on GitHub.
- `ticks`: ticks that acted, held admission back or failed, oldest first, as {instance, first, last, count, line, failed, did, errors}. Consecutive successful ticks with the same line, steps and errors are folded, count saying how many; every failed tick is separate, including a failure said only in its line.
- `events`: other tick log lines, oldest first, as {at, instance, line}: commands, starts, stops, hot workers and what watches did among them. A command that names no issue stays here.
- `ended`: GitHub's issue_closed, pr_merged, pr_closed and intake steps in the interval, each {at, issue, pr, step, detail}. Fork PRs and another repo's issues in a shared tracker are left out.
- `log`: {file, first, retained_only}: the local log, its oldest retained timestamp (null with none), and true because older lines may have been cut back. The interval can only report local steps still retained. GitHub reads include closed issues and PRs, and their intake and stage records. {error, see} in place of watching means that interval could not be read; the top-level until is null and the command exits non-zero.

It exits non-zero where any finding is critical, the host's or a checkout's, or a checkout could not be asked — printing the same object. Errors: the checkout list cannot be read.

The findings: a command the fleet needs is missing or logged out; orca cannot be asked, or does not know the checkout; the worker launch command of a harness the fleet could be asked for cannot start a worker — said with the harness and what calls for it; no `run` is alive and claims are held; the fleet instance of this checkout is broken — with why, since when and when the host runner starts it again; the same step failed in each of the last three ticks; the team has a fault at its head; a fleet lock stands on an issue that cannot be worked (all critical) — a step failed and has not recovered, including a worker that failed to start, with its error and the line to try again; a claim has no worker; a worker is silent with its claim still at work, as the tick judges it — its `fix` the command the tick runs next for it; the frontier is not empty, slots are free and the last tick started nobody; a flow label names no workflow of the team; an issue has been at a stage longer than the repo's p95 for completed performances of that stage, or, with fewer than 20, longer than its own earlier record gaps of at least a minute; an issue is one rework from a breaker; a patrol is failed or full; an issue has waited for a human for more than an hour, its `fix` being the line that answers it; the gate is slow — with the host, the fixed duration budget when declared, the termination gate limit and the {const:SLOW_GATE_LANDINGS} runs, each a commit and its seconds, as `afkconvoy plan` has them, and the issue open for it when there is one; the host runner did not move onto the newer release installed where it was started from — it did not start, does not say its version, or waited past the release move limit — with the file, both releases and why, its `fix` that file's `--version` (all warning) — the repo's config names `concurrency`, which nothing reads any more — with the value it names and the concurrency in force: nothing is held up, even where the values differ. Its `fix` is deleting the key alone where the checkout says its own or the config names the number already in force; otherwise `afkconvoy run <checkout> --concurrency <value>` keeps the config's number on this host before deleting the key; the role or the team a stage in flight started under has moved since; and the repo's config declares no gate limit, so the gate has the default's {const:GATE_SECONDS} seconds before anything ends it: `fix` is the file and the key, `gate_seconds`; and a worktree on this host is kept with no claim on its issue — said with what it holds that the remote does not (uncommitted changes, commits only it has, or nothing), its `fix` the line that removes it, pushing first whatever only it holds (all info). A checkout that names a harness no worker of this checkout starts on is said with the checkout, the harness and why — the role holds no limits for that harness, or the harness cannot start — and no other harness is started in its place.

Moved: the role's directory holds something else at the head of its repo than at the commit the stage started under — a file added, removed or changed. A new commit is not it: in a repo of several roles a push gives every role a new commit, and moves the ones whose directory it changed. The team has moved when its directory has, without what a worker is not given of it: the directories of its roles, and the .claude/ at its top. So a push that changes only a role moves no team, one that changes only the team's rules — the CLAUDE.md at its top, which every worker is given — moves the team, and one that changes a workflow moves no role. A role in a repo of its own moves with every push that changes a file there. The two are compared in the copies this host keeps (copies/ under the cache directory); where the one the stage started under is no longer there, a different commit is all there is to go by, and it is said to have moved. `evidence` gives both commits.

## What trips you up

- A warning may be the fleet mid-step: a claim has no worker between the tick that took it and the one that starts its worker. Run it again after a tick before you act on one; a critical finding does not go away by waiting.
- It says nothing of ticks nobody logged. A `run` logs under your cache directory (`afkconvoy help cache` says where it is) on the host it runs on: run `doctor` there, as the same user. The log is a log: nothing reads it to decide anything, and deleting it loses only what `doctor` could say of the ticks before.
- It makes no startup probe, so it looks for the fleet's refs where this repo's last `plan`, `tick` or `run` found the remote keeps them. In a clone none ever ran in, it looks under refs/convoy/.
- `fix` is the first thing to try, not a promise: read `afkconvoy <command> --help` of the command it names before you run it.

A human wait taken by another host has `taken_by` naming that host and an info finding. It starts no watch round here, is absent from this host's human report entries and sends nothing through `--notify-command` and asks no question. `taken_by` is absent when the wait is available here or held here. Reading doctor changes no claim; the host runner takes available waits before queueing its look.

## Worker proxy bypass

`--worker-no-proxy [HARNESS=]HOSTS` shows what a start with those flags would give each harness, under `worker_no_proxy`: [`afkconvoy help worker-proxy`](help:worker-proxy)."#;

#[cfg(test)]
mod tests {
    use afkconvoy_core::patrol::Row;
    use afkconvoy_core::plan::{Dispatchable, Excluded, Mine, PeerClaim};
    use afkconvoy_core::worker::GRACE;

    use super::*;
    use crate::log::Cause;

    const NOW: u64 = 100_000;

    fn host() -> Host {
        Host {
            tools: vec![
                (Tool::Git, Found::Ready),
                (Tool::Gh, Found::Ready),
                (Tool::Orca, Found::Ready),
            ],
            gh_host: Some("github.com".into()),
            orca_unasked: None,
            launches: vec![Launch {
                checked: Checked {
                    harness: afkconvoy_core::harness::Harness::ClaudeCode,
                    callers: vec![],
                    command: "claude".into(),
                    tool: None,
                    fault: None,
                    flags_fault: None,
                },
            }],
        }
    }

    /// A tick of a live fleet at its bound of 2: `errors` are its failed steps.
    fn tick(count: u64, errors: Value) -> (u64, Tick) {
        let answer = json!({"did": [], "errors": errors, "held": 2, "frontier_remaining": 0});
        let tick = Tick {
            count,
            sleep: 90,
            line: "nothing to do".into(),
            bound: Some(2),
            answer: Some(answer),
            error: None,
        };
        (NOW - 500 + count * 90, tick)
    }

    fn run(ticks: Vec<(u64, Tick)>) -> Run {
        Run {
            alive: true,
            instance: Some("alpha".into()),
            host: "h1".into(),
            heartbeat_age: Some(60),
            heartbeats: Heartbeats { found: 1, live: vec!["alpha".into()] },
            log_file: Some(PathBuf::from("/cache/afkconvoy/runs/widgets.jsonl")),
            ticks: Some(ticks),
            last_took: None,
            tick_started: None,
            hot: vec![],
            broken: None,
            events: vec![],
        }
    }

    fn set() -> WorkingSet {
        WorkingSet {
            unworked_blockers: vec![],
            frontier: vec![],
            escalating: vec![],
            excluded: vec![],
            waiting_on_human: vec![],
            mine: vec![],
            peer_live: vec![],
            stale: vec![],
            stale_closed: vec![],
            gone: vec![],
            free_slots: 2,
            now: NOW,
            locks: vec![],
            flowing: vec![],
            patrols: vec![],
            slow_gate: None,
            held_back: vec![],
            casting: vec![],
            cut: None,
        }
    }

    fn state() -> State {
        State {
            wait_holders: Default::default(),
            refusals: json!({}),
            targets: json!([]),
            set: set(),
            held: vec![],
            stood: vec![],
            stage_baselines: Default::default(),
            team: TeamRead::Unbound,
            ready_label: "ready-for-agent".into(),
            tracker: Tracker::Own,
            unclaimed: vec![],
            not_ours: vec![],
            on_gone: vec![],
            candidates: json!([]),
            deliveries: vec![],
        }
    }

    fn team() -> TeamRead {
        TeamRead::Loaded {
            flow_labels: vec!["flow:feature".into()],
            nothing_safe: vec![],
            unmatched_models: vec![],
            unmatched: vec![],
        }
    }

    #[test]
    fn a_role_that_can_run_nothing_safely_and_an_unmatched_roster_entry_are_named() {
        let team = TeamRead::Loaded {
            flow_labels: vec![],
            nothing_safe: vec![("lead".into(), "lead has no harness …".into())],
            unmatched_models: vec![],
            unmatched: vec![("dev".into(), "the roster's entry …".into())],
        };
        let found = team_findings(&team);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].what.starts_with("lead has no harness on this host it holds limits for"));
        assert_eq!(found[0].severity, Kind::NothingSafe.severity());
        assert!(found[1].what.contains("`dev` matches nothing in the host kit"));
        assert_eq!(found[1].fix, "afkconvoy kit, or the team's roster: roster.dev");
    }

    /// A claim at work on `issue`, its worker busy in its worktree.
    fn held(issue: u64) -> Held {
        Held {
            issue,
            title: Some(format!("issue {issue}")),
            status: Status::NoPr,
            pr: None,
            stage: None,
            role: None,
            held_for: Some(600),
            worker: Some(WorkerState::Busy),
            orca_done: None,
            silent: None,
            worktree: Some(PathBuf::from(format!("/wt/issue-{issue}"))),
            under: None,
            moved: vec![],
            gate: None,
        }
    }

    /// An issue at the stage `build` of the workflow `feature`, with a worker owed.
    fn stood(number: u64) -> Stood {
        Stood {
            number,
            flow_labels: vec!["flow:feature".into()],
            workflow: Ok("feature".into()),
            stage: Some("build".into()),
            role: Some("dev".into()),
            rework: (0, 2),
            in_a_row: vec![("review".into(), 0, 2)],
            recorded: vec![],
            opened_by: None,
        }
    }

    fn excluded(number: u64, reason: &str) -> Excluded {
        Excluded { number, title: format!("issue {number}"), reason: reason.into() }
    }

    fn patrol(standing: Standing) -> Row {
        Row {
            name: "triage".into(),
            standing,
            every: 3600,
            last_run: Some(NOW - 100),
            open: 0,
            max_open: 2,
            max_runs_a_day: None,
            max_landed_a_day: None,
            landed_in_day: None,
            runs_in_day: None,
            until: None,
            failure: None,
            asked: None,
            claim: None,
        }
    }

    fn found(host: &Host, run: &Run, state: &State) -> Vec<Finding> {
        let checkout = Path::new("/repos/widgets");
        let found =
            findings(&Seen { now: NOW, host, checkout, config: None, run, state: Ok(state) });
        for finding in &found {
            // Never prose alone: one line to run, or a file and the key in it.
            let fix = &finding.fix;
            let line = ["afkconvoy ", "gh ", "orca ", "brew ", "curl ", "claude "]
                .iter()
                .any(|program| fix.starts_with(program))
                || fix == "pi";
            let file = fix.split(": ").next().is_some_and(|file| file.ends_with(".md"));
            assert!(line || file, "a fix that is neither a line nor a file and key: {fix:?}");
            assert!(!fix.contains('\n') && !finding.evidence.is_empty(), "{finding:?}");
        }
        found
    }

    /// The fixes of the findings whose `what` contains `what`.
    fn fixes(found: &[Finding], what: &str) -> Vec<String> {
        let about = found.iter().filter(|finding| finding.what.contains(what));
        about.map(|finding| finding.fix.clone()).collect()
    }

    #[test]
    fn a_fleet_with_no_fault_has_no_finding() {
        let mut state = state();
        state.team = team();
        state.held = vec![held(3)];
        state.stood = vec![stood(3)];
        state.set.patrols = vec![patrol(Standing::NotDue), patrol(Standing::Running)];
        let ticks = vec![tick(1, json!([])), tick(2, json!([])), tick(3, json!([]))];
        assert_eq!(found(&host(), &run(ticks), &state), []);
        // …nor has a host no fleet ever ran on
        let nobody =
            Run { alive: false, instance: None, heartbeat_age: None, ticks: None, ..run(vec![]) };
        assert_eq!(found(&host(), &nobody, &self::state()), []);
    }

    #[test]
    fn a_host_that_lacks_what_the_fleet_needs_is_critical_with_the_line_that_fixes_it() {
        let mut lacking = host();
        lacking.tools = vec![
            (Tool::Gh, Found::LoggedOut(afkconvoy_core::tools::LoginAnswer::default())),
            (Tool::Orca, Found::Missing),
        ];
        let found = found(&lacking, &run(vec![]), &state());
        assert_eq!(fixes(&found, "`gh` is not logged in"), ["gh auth login"]);
        assert_eq!(fixes(&found, "`orca` is not installed"), [Tool::Orca.when_missing()]);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|finding| finding.severity == Severity::Critical));

        let mut unknown = host();
        unknown.tools[2].1 = Found::KnowsNoCheckout("/repos/widgets".into());
        let found = self::found(&unknown, &run(vec![]), &state());
        assert_eq!(
            fixes(&found, "`orca` does not know this checkout"),
            ["orca repo add --path '/repos/widgets'"]
        );
        // an orca that is not running, and one that runs and cannot say
        let mut stopped = host();
        stopped.tools[2].1 = Found::NotRunning;
        let found = self::found(&stopped, &run(vec![]), &state());
        assert_eq!(fixes(&found, "`orca` is not running"), ["orca open"]);
        stopped.orca_unasked = Some("orca repo list failed: no".into());
        let found = self::found(&stopped, &run(vec![]), &state());
        assert_eq!(fixes(&found, "`orca` cannot be asked about this checkout"), ["orca repo list"]);

        let mut flagless = host();
        flagless.launches[0].checked.flags_fault = Some("it takes no --settings".into());
        let found = self::found(&flagless, &run(vec![]), &state());
        assert_eq!(fixes(&found, "cannot start a worker in its role"), ["afkconvoy kit"]);
    }

    #[test]
    fn a_harness_a_stage_calls_for_is_found_as_the_default_one_is_with_what_calls_for_it() {
        let pi = |found: Found| {
            let mut host = host();
            host.launches.push(Launch {
                checked: Checked {
                    harness: afkconvoy_core::harness::Harness::Pi,
                    callers: vec!["the stage `build` of the workflow `feature`".into()],
                    command: "pi".into(),
                    tool: Some((Tool::Harness(Harness::Pi), found)),
                    fault: Some("\"pi\" resolves to nothing in a login shell".into()),
                    flags_fault: Some("`pi --help` failed in a login shell".into()),
                },
            });
            host
        };
        let missing = pi(Found::Missing);
        let found = self::found(&missing, &run(vec![]), &state());
        assert_eq!(fixes(&found, "`pi` is not installed"), [Tool::Harness(Harness::Pi).install()]);
        assert_eq!(found.len(), 1, "said once, and not again of its command: {found:?}");
        assert!(
            found[0]
                .evidence
                .ends_with("called for by the stage `build` of the workflow `feature`")
        );
        assert_eq!(found[0].severity, Severity::Critical);
        assert_eq!(host_json(&missing)["tools"][3], json!({"name": "pi", "state": "missing"}));

        let mut logged_out = pi(Found::LoggedOut(afkconvoy_core::tools::LoginAnswer::default()));
        logged_out.launches[1].checked.fault = None;
        logged_out.launches[1].checked.flags_fault = None;
        let found = self::found(&logged_out, &run(vec![]), &state());
        assert_eq!(fixes(&found, "`pi` holds credentials for no provider"), ["pi"]);
        assert!(found[0].what.contains("`/login` at its prompt logs it in"), "{found:?}");
        assert!(found[0].evidence.starts_with("`pi --list-models` says so"), "{found:?}");
        assert_eq!(self::found(&pi(Found::Ready), &run(vec![]), &state()).len(), 2);
    }

    #[test]
    fn no_run_alive_with_claims_held_is_critical() {
        let mut state = state();
        state.held = vec![held(3), held(5)];
        let dead = Run { alive: false, heartbeat_age: Some(5000), ..run(vec![]) };
        let found = found(&host(), &dead, &state);
        assert_eq!(fixes(&found, "no `afkconvoy run` is alive"), ["afkconvoy run"]);
        assert_eq!(found[0].severity, Severity::Critical);
        assert!(found[0].what.contains("alpha holds #3, #5"), "{found:?}");
        assert!(found[0].evidence.contains("1h 23m old"), "{found:?}");
        // alive, or holding nothing, it is none
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
        assert_eq!(self::found(&host(), &dead, &self::state()), []);
    }

    #[test]
    fn a_claim_with_no_worker_names_the_start_to_make_by_hand() {
        let mut state = state();
        let gone = Held { worker: Some(WorkerState::Gone), ..held(3) };
        let joining = Held { status: Status::Landing, pr: Some(30), ..gone.clone() };
        let never_cut = Held { worktree: None, ..Held { issue: 7, ..gone.clone() } };
        state.held = vec![gone.clone(), Held { issue: 5, ..joining }, never_cut];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "has no worker"),
            [
                "afkconvoy dispatch --issue 3 --instance alpha",
                "afkconvoy restart --issue 5 --instance alpha",
                "afkconvoy dispatch --issue 7 --instance alpha",
            ]
        );
        assert!(found.iter().all(|finding| finding.severity == Severity::Warning));
        // one whose work is on the train or landed is owed no worker…
        state.held = vec![Held { status: Status::Closed, ..gone }];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
        // …and one orca could not speak of is not said to have none
        state.held = vec![Held { worker: None, ..held(3) }];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    #[test]
    fn a_claim_gets_a_ticks_chance_to_start_its_worker() {
        let mut fleet = run(vec![tick(1, json!([]))]);
        fleet.last_took = Some(10);
        let gone = |age| Held { held_for: Some(age), worker: Some(WorkerState::Gone), ..held(375) };
        for age in [0, 27, 99] {
            assert_eq!(
                claim_findings(&fleet, &[gone(age)], NOW, &[]),
                [],
                "claim stamped {age}s ago"
            );
        }
        assert_eq!(claim_findings(&fleet, &[gone(100)], NOW, &[])[0].kind, Kind::NoWorker);
        assert_eq!(claim_findings(&fleet, &[gone(101)], NOW, &[])[0].kind, Kind::NoWorker);
        assert_eq!(
            claim_findings(&fleet, &[Held { held_for: None, ..gone(0) }], NOW, &[])[0].kind,
            Kind::NoWorker,
            "an unknown stamp retains the finding"
        );
        assert_eq!(claim_findings(&run(vec![]), &[gone(27)], NOW, &[])[0].kind, Kind::NoWorker);
    }

    #[test]
    fn a_running_tick_dispatches_a_claim_before_doctor_calls_it_workerless() {
        // 2026-11-01: tick 18 began at 11:43:58, the claim was stamped at
        // 11:44:12, and the look at 11:44:39 saw no terminal yet.
        let began = 1_793_533_438;
        let stamped = began + 14;
        let entry = |at, instance: &str, what| Entry {
            at,
            instance: instance.into(),
            host: "h1".into(),
            what,
        };
        let mut entries = vec![entry(
            began,
            "alpha",
            What::TickStarted { count: 18, cause: Cause::Timer, by: None },
        )];
        let read = |entries: &[Entry]| {
            let (_, _, ticks) = run_log::run(entries, "h1");
            Run {
                tick_started: run_log::tick_started(entries, "h1"),
                last_took: run_log::last_took(entries, "h1"),
                ..run(ticks)
            }
        };
        let gone = |now| Held {
            held_for: Some(now - stamped),
            worker: Some(WorkerState::Gone),
            ..held(375)
        };
        for now in [began + 41, began + 300] {
            assert_eq!(claim_findings(&read(&entries), &[gone(now)], now, &[]), []);
        }
        let at_start = Held { held_for: Some(41), ..gone(began + 41) };
        assert_eq!(claim_findings(&read(&entries), &[at_start], began + 41, &[]), []);
        let older = Held { held_for: Some(1000), ..gone(began + 41) };
        assert_eq!(
            claim_findings(&read(&entries), &[older], began + 41, &[])[0].kind,
            Kind::NoWorker,
            "a tick that began after an old claim does not hide it"
        );

        entries.push(entry(began + 77, "alpha", What::Tick(tick(18, json!([])).1)));
        assert_eq!(run_log::tick_started(&entries, "h1"), None);
        // A following tick has now had its chance, even if a wake made it
        // finish inside the sleep plus the preceding tick's duration.
        entries.push(entry(
            began + 80,
            "alpha",
            What::TickStarted { count: 19, cause: Cause::Woken, by: None },
        ));
        entries.push(entry(began + 90, "alpha", What::Tick(tick(19, json!([])).1)));
        let found = claim_findings(&read(&entries), &[gone(began + 90)], began + 90, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::NoWorker);
        assert_eq!(found[0].fix, "afkconvoy dispatch --issue 375 --instance alpha");

        // An old instance's unfinished tick is no proof for a new run.
        entries.push(entry(
            began + 100,
            "old",
            What::TickStarted { count: 20, cause: Cause::Timer, by: None },
        ));
        entries.push(entry(
            began + 101,
            "alpha",
            What::Start { worker_command: "codex".into(), harness_commands: [].into() },
        ));
        assert_eq!(run_log::tick_started(&entries, "h1"), None);
    }

    #[test]
    fn changing_durations_tick_counts_and_free_slots_keep_their_condition_identity() {
        let same = |before: &Finding, after: &Finding| {
            assert_ne!(before.what, after.what, "doctor keeps its current presentation");
            assert_eq!(before.watch_id, after.watch_id);
        };
        let stopped = |quiet| Held {
            worker: Some(WorkerState::Idle { quiet: Some(quiet) }),
            silent: Some(Silenced {
                command: "nudge",
                nudged: false,
                waited: None,
                declared: false,
            }),
            ..held(3)
        };
        same(
            &claim_findings(&run(vec![]), &[stopped(GRACE)], NOW, &[])[0],
            &claim_findings(&run(vec![]), &[stopped(GRACE + 360)], NOW, &[])[0],
        );
        let long = Stood { recorded: vec![NOW - 3300, NOW - 3000, NOW - 2000], ..stood(7) };
        same(
            &issue_findings(std::slice::from_ref(&long), &team(), &Default::default(), NOW)[0],
            &issue_findings(&[long], &team(), &Default::default(), NOW + 360)[0],
        );
        let failed = |count| tick(count, json!([{"do": "dispatch", "issue": 3, "error": "busy"}]));
        let once = log_findings(&run(vec![failed(1)]));
        let repeated = log_findings(&run(vec![failed(1), failed(2), failed(3)]));
        same(&once[0], &repeated[0]);
        same(&repeated[0], &log_findings(&run(vec![failed(2), failed(3), failed(4)]))[0]);
        let mut state = state();
        state.set.frontier =
            vec![Dispatchable { number: 3, title: "work".into(), stage: None, cast: None }];
        let before = plan_findings(&run(vec![failed(1)]), &state);
        state.set.free_slots += 1;
        same(&before[0], &plan_findings(&run(vec![failed(2)]), &state)[0]);
    }

    #[test]
    fn a_silent_worker_with_its_claim_at_work_names_what_the_tick_runs_next() {
        let mut state = state();
        let silent =
            |command, nudged, waited| Some(Silenced { command, nudged, waited, declared: false });
        let stopped = |quiet, silent| Held {
            worker: Some(WorkerState::Idle { quiet: Some(quiet) }),
            silent,
            ..held(3)
        };
        // due its nudge: as soon as the tick judges it silent, with no clock of doctor's
        state.held = vec![stopped(GRACE, silent("nudge", false, None))];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "has been stopped for 5m"),
            ["afkconvoy nudge --issue 3 --instance alpha"]
        );
        assert!(found[0].evidence.contains("/wt/issue-3"), "{found:?}");
        assert!(found[0].evidence.starts_with("it declared nothing"), "{found:?}");
        // silent past its nudge: the tick escalates it
        state.held = vec![stopped(GRACE * 2, silent("tick", true, None))];
        let found = self::found(&host(), &run(vec![]), &state);
        assert_eq!(fixes(&found, "has been stopped for"), ["afkconvoy tick --instance alpha"]);
        assert!(found[0].evidence.starts_with("it stayed silent after its nudge"), "{found:?}");
        // its own background work past the gate limit is a silence, however
        // short the terminal's quiet
        state.held = vec![stopped(GRACE, silent("tick", false, Some(7200)))];
        let found = self::found(&host(), &run(vec![]), &state);
        assert_eq!(fixes(&found, "has been stopped for"), ["afkconvoy tick --instance alpha"]);
        assert!(found[0].evidence.contains("waited 2h 0m on background work"), "{found:?}");
        // a worker the tick does not judge silent is none, however long it stopped
        state.held = vec![stopped(GRACE * 100, None)];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    /// sunfmin/control#623: doctor and health read one agent the same way,
    /// whichever of orca's `programStatus.state` and `state` says its turn
    /// is over.
    #[test]
    fn doctor_and_health_agree_on_an_agent_whose_two_states_differ() {
        let failed: Value =
            serde_json::from_str(include_str!("../tests/fixtures/orca-failed-agent.json")).unwrap();
        let mut working_by_program = failed.clone();
        working_by_program["programStatus"]["state"] = json!("working");
        let mut done_by_program = failed.clone();
        done_by_program["state"] = json!("working");
        done_by_program["programStatus"]["state"] = json!("done");
        for orca in [failed, working_by_program, done_by_program] {
            let agent = crate::orca::Agent::of(&orca).unwrap();
            let held = Held {
                worker: Some(WorkerState::Busy),
                orca_done: agent.over_since(),
                ..held(623)
            };
            assert_eq!(
                worker::disputed_past_grace(held.worker, held.orca_done, 799 + GRACE),
                agent.failed(),
                "doctor and health disagree on {orca}"
            );
        }
    }

    /// sunfmin/control#511: a session that says busy while orca has said
    /// done for the grace period is a finding, and neither read is reported
    /// alone as the worker's state.
    #[test]
    fn a_worker_busy_by_its_session_and_done_by_orca_is_disputed() {
        let over = |agent: Value| crate::orca::Agent::of(&agent).and_then(|a| a.over_since());
        let report = json!({"state": "done", "stateStartedAt": (NOW - GRACE) * 1000,
            "programStatus": {"state": "done"}});
        assert_eq!(over(report), Some(NOW - GRACE));
        assert_eq!(over(json!({"state": "done", "programStatus": {"state": "working"}})), None);
        assert_eq!(over(Value::Null), None);
        let landing = |since: Option<u64>| Held {
            status: Status::Landing,
            pr: Some(52),
            orca_done: since,
            ..held(491)
        };
        let found = claim_findings(&run(vec![]), &[landing(Some(NOW - GRACE))], NOW, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, Kind::WorkerDisputed);
        assert_eq!(
            found[0].fix,
            "afkconvoy tell --issue 491 --text 'Your last turn stopped. Carry on: run afkconvoy \
             land --issue 491 again'"
        );
        assert_eq!(held_json(&landing(Some(NOW - GRACE)), &[])["worker"]["state"], "disputed");
        // Orca only just said so, or does not: no finding, and busy is busy.
        for since in [Some(NOW - 10), None] {
            assert_eq!(claim_findings(&run(vec![]), &[landing(since)], NOW, &[]), []);
        }
        assert_eq!(held_json(&landing(None), &[])["worker"]["state"], "busy");
        let at_work = Held { orca_done: Some(0), ..held(3) };
        let found = claim_findings(&run(vec![]), &[at_work], NOW, &[]);
        assert!(found[0].fix.contains("--text 'Your last turn stopped. Carry on with your brief"));
    }

    #[test]
    fn a_fleet_lock_hides_only_the_workers_it_leaves_alone() {
        let silent =
            Some(Silenced { command: "tick", nudged: true, waited: None, declared: false });
        let stopped = |issue| Held {
            worker: Some(WorkerState::Idle { quiet: Some(900) }),
            silent,
            ..held(issue)
        };
        let gone = Held { worker: Some(WorkerState::Gone), ..held(4) };
        let claims = [stopped(3), gone, stopped(7)];
        let found = claim_findings(&run(vec![]), &claims, NOW, &[7]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::WorkerStopped);
        assert!(found[0].what.contains("#7"));
        assert_eq!(
            held_json(&claims[0], &[7])["worker"],
            json!({"state": "waiting_on_lock", "locks": [7]})
        );
    }

    #[test]
    fn a_failed_dispatch_uses_the_fleets_line_and_keeps_the_screen_as_evidence() {
        let checkout = Path::new("/Users/sunfmin/Developments/zhiyuanwiki");
        let error = "orca terminal send failed: agent_prompt_blocked: the line started no turn \
                     within 30s; delivery receipt: {\"state\":\"sent\"}; worktree /wt/issue-386 \
                     (terminal worker-386)\nScreen's last lines:\n\
                     Run `node --experimental-strip-types ./license/src/deploy.ts\n\
                          --mint-fulfill-token` to mint a deploy secret.";
        let failed = tick(4, json!([{"do": "dispatch", "issue": 386, "error": error}]));
        let found = log_findings(&run(vec![failed]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::StepUnrecovered);
        assert_eq!(found[0].evidence, error);
        assert_eq!(
            crate::checkout::about_checkout(checkout, &found[0].fix),
            "afkconvoy dispatch -C '/Users/sunfmin/Developments/zhiyuanwiki' --issue 386 --instance alpha"
        );
    }

    #[test]
    fn quoted_external_evidence_never_supplies_a_named_line() {
        for evidence in [
            "\nScreen's last lines:\nthrow new Error(`protocol: ${name}`)",
            "\nScreen's last lines:\n`-C <CHECKOUT>`",
            "; delivery receipt: {\"text\":\"run `deploy --publish`\"}",
            "\nstdout:\nrun `deploy --publish`",
            "\nstderr:\nrun `deploy --publish`",
            "\r\nScreen's last lines:\nrun `afkconvoy stop --all`",
            ": orca terminal send failed: agent_prompt_blocked: run `deploy --publish`",
            ": orca terminal wait failed: run `deploy --publish`",
            ": gh api failed: run `deploy --publish`",
            ": git fetch failed: run `deploy --publish`",
        ] {
            assert_eq!(
                named_line(&format!("cannot start the worker{evidence}")),
                None,
                "{evidence}"
            );
            let own = "orca knows no repo here — add it with `orca repo add --path /r`";
            assert_eq!(
                named_line(&format!("{own}{evidence}")),
                Some("orca repo add --path /r".into()),
                "a fleet remedy before the evidence still counts: {evidence}"
            );
        }
        assert_eq!(named_line("no remedy, only an unmatched `quoted command"), None);
        assert_eq!(named_line("try `a command\nScreen's last lines:\n`"), None);
        assert_eq!(
            named_line("the step `dispatch` needs ` afkconvoy kit `"),
            Some("afkconvoy kit".into())
        );
    }

    #[test]
    fn a_step_that_failed_in_each_of_the_last_ticks_is_critical_with_its_errors_own_line() {
        let knows_not = "orca knows no repo at /repos/widgets — add this checkout once with \
                         `orca repo add --path '/repos/widgets'`";
        let failed =
            |issue: u64, error: &str| json!({"do": "dispatch", "issue": issue, "error": error});
        let both = || json!([failed(3, knows_not), failed(5, "orca worktree create failed: busy")]);
        let ticks = vec![tick(4, both()), tick(5, both()), tick(6, both())];
        let found = found(&host(), &run(ticks), &state());
        assert_eq!(
            fixes(&found, "`dispatch #3` failed"),
            ["orca repo add --path '/repos/widgets'"]
        );
        // an error that names no line: the same transition, by hand
        assert_eq!(
            fixes(&found, "`dispatch #5` failed"),
            ["afkconvoy dispatch --issue 5 --instance alpha"]
        );
        assert!(found[0].what.contains("the last 3 ticks (4 to 6)"), "{found:?}");
        assert_eq!(found[0].severity, Severity::Critical);

        // a tick that failed whole, three times over
        let whole = |count| {
            let (at, tick) = tick(count, json!([]));
            (
                at,
                Tick { answer: None, bound: None, error: Some("gh pr list failed".into()), ..tick },
            )
        };
        let found = self::found(&host(), &run(vec![whole(1), whole(2), whole(3)]), &state());
        assert_eq!(fixes(&found, "`tick` failed"), ["afkconvoy plan --instance alpha"]);

        // twice, or not in a row, or on another issue each time: warnings
        let twice = vec![tick(5, both()), tick(6, both())];
        let found = self::found(&host(), &run(twice), &state());
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|finding| finding.severity == Severity::Warning));
        let broken = vec![tick(4, both()), tick(5, json!([])), tick(6, both())];
        let found = self::found(&host(), &run(broken), &state());
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|finding| finding.severity == Severity::Warning));
        let other = |issue| json!([failed(issue, "no")]);
        let moving = vec![tick(4, other(3)), tick(5, other(5)), tick(6, other(7))];
        let found = self::found(&host(), &run(moving), &state());
        assert_eq!(found.len(), 3);
        assert!(found.iter().all(|finding| finding.severity == Severity::Warning));
    }

    #[test]
    fn a_failed_tend_train_or_cut_train_is_that_failed_step() {
        let failed = tick(
            4,
            json!([
                {"do": "tend_train", "error": "not ready"},
                {"do": "cut_train", "error": "no checkout"}
            ]),
        );
        let found = log_findings(&run(vec![failed, tick(5, json!([]))]));
        let what: Vec<_> = found.iter().map(|finding| finding.what.as_str()).collect();
        assert_eq!(
            what,
            [
                "`tend_train` failed in tick 4 and has not recovered",
                "`cut_train` failed in tick 4 and has not recovered",
            ]
        );
    }

    #[test]
    fn failures_stay_visible_across_unrelated_ticks_and_clear_when_work_recovers() {
        let failed = tick(4, json!([{"do": "slow_gate", "error": "not ready"}]));
        let unrelated = tick(5, json!([]));
        let mut ticks = vec![failed, unrelated];
        let found = log_findings(&run(ticks.clone()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].what, "`slow_gate` failed in tick 4 and has not recovered");
        assert_eq!(found[0].fix, "afkconvoy tick --instance alpha");
        let mut recovered = tick(6, json!([]));
        recovered.1.answer.as_mut().unwrap()["did"] =
            json!([{"do": "slow_gate", "outcome": "done"}]);
        ticks.push(recovered);
        assert_eq!(log_findings(&run(ticks)), []);

        let failed = tick(4, json!([{"do": "dispatch", "issue": 3, "error": "not ready"}]));
        let mut board = tick(5, json!([]));
        board.1.answer.as_mut().unwrap()["did"] =
            json!([{"do": "board", "issue": 3, "outcome": "done"}]);
        let mut ticks = vec![failed, board];
        assert_eq!(log_findings(&run(ticks.clone())).len(), 1);
        let mut continued = tick(6, json!([]));
        continued.1.answer.as_mut().unwrap()["did"] =
            json!([{"do": "judge", "issue": 3, "outcome": "continued"}]);
        ticks.push(continued);
        assert_eq!(log_findings(&run(ticks)), []);
    }

    #[test]
    fn an_advance_with_nothing_to_do_recovers_an_earlier_dispatch_failure() {
        let failed = tick(
            4,
            json!([{"do": "dispatch", "issue": 3, "error": "orca worktree create failed: runtime_timeout"}]),
        );
        let mut advanced = tick(5, json!([]));
        advanced.1.answer.as_mut().unwrap()["did"] =
            json!([{"do": "advance", "issue": 3, "outcome": "nothing"}]);
        assert_eq!(log_findings(&run(vec![failed, advanced])), []);
    }

    #[test]
    fn an_unrecovered_step_keeps_its_identity_whatever_its_severity() {
        let failed = |count| json!({"tick": count, "do": "advance", "issue": 160, "error": "no live terminal"});
        let once = log_findings(&run(vec![tick(21, json!([failed(21)]))]));
        assert_eq!(once.len(), 1);
        assert_eq!(once[0].kind, Kind::StepUnrecovered);
        assert_eq!(once[0].evidence, "no live terminal");

        let repeated = vec![
            tick(21, json!([failed(21)])),
            tick(22, json!([failed(22)])),
            tick(23, json!([failed(23)])),
        ];
        let again = log_findings(&run(repeated));
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].kind, Kind::StepFailed);
        assert_eq!(again[0].severity, Severity::Critical);
        assert_ne!(again[0].what, once[0].what, "its words change");
        assert_eq!(again[0].watch_id, once[0].watch_id, "its condition does not");
        assert_eq!(once[0].watch_id, r#"checkout:["step","advance",160,null]"#);
    }

    #[test]
    fn a_health_observation_is_overdue_only_past_its_ticks_chance() {
        // The instance holding /w sleeps 90s between ticks, and its last took 10s.
        let pace = crate::health::Pace::new(90, 10, None, [(7, PathBuf::from("/w"))]);
        let paces = [Pace { checkout: "/c".into(), pace }];
        let health = |next: u64| -> crate::health::Health {
            serde_json::from_value(json!({
                "workers": [{"path": "/w", "terminal": "t", "harness": "codex", "snapshots": [],
                    "next_check": next, "rate_retries": 0, "source": "rule", "action": "waiting",
                    "error": "stuck"}],
                "observations_failed": [["/f", "no terminal", next]]}))
            .unwrap()
        };
        assert_eq!(overdue_of(&health(998), &paces, 1000), [], "the next tick observes it");
        assert_eq!(overdue_of(&health(901), &paces, 1000), [], "within its sleep and a tick");
        let missed = overdue_of(&health(899), &paces, 1000);
        let ids: Vec<_> = missed.iter().map(|finding| finding.watch_id.as_str()).collect();
        assert_eq!(ids, [r#"host:["health-overdue","/w"]"#, r#"host:["health-overdue","/f"]"#]);
        assert!(
            missed.iter().all(|finding| finding.kind == Kind::HealthOverdue && finding.of_host)
        );
        assert!(missed[0].evidence.contains("still waiting: stuck"), "{missed:?}");
        assert!(missed[1].evidence.contains("its retry is overdue: no terminal"), "{missed:?}");
        let later = overdue_of(&health(800), &paces, 1000);
        assert_eq!(later[0].watch_id, missed[0].watch_id, "later, the same observation");
    }

    #[test]
    fn a_worker_a_fleet_lock_leaves_alone_has_no_overdue_observation_until_it_lifts() {
        let mut state = state();
        state.held = vec![held(4), held(9)];
        state.set.locks = vec![9];
        let worker = |path: &str| {
            json!({"path": path, "terminal": "t", "harness": "codex", "snapshots": [],
                "next_check": 100, "rate_retries": 0, "source": "rule", "action": "waiting"})
        };
        let health: crate::health::Health = serde_json::from_value(json!({
            "workers": [worker("/wt/issue-4"), worker("/wt/issue-9")],
            "observations_failed": []}))
        .unwrap();
        let paths = |state: &State| -> Vec<String> {
            let pace = pace_of(Path::new("/c"), &run(vec![]), Some(state));
            overdue_of(&health, &[pace], 1000)
                .iter()
                .map(|finding| finding.watch_id.clone())
                .collect()
        };
        assert_eq!(
            paths(&state),
            [r#"host:["health-overdue","/wt/issue-9"]"#],
            "the lock issue's worker is observed; #4's is left alone"
        );
        state.set.locks = vec![];
        assert_eq!(
            paths(&state),
            [
                r#"host:["health-overdue","/wt/issue-4"]"#,
                r#"host:["health-overdue","/wt/issue-9"]"#
            ],
            "the lock lifted: #4's overdue observation is found again"
        );
    }

    #[test]
    fn a_listed_checkout_not_alive_is_one_finding_beside_a_broken_or_claimed_one() {
        let dead = Run { alive: false, ..run(vec![]) };
        let found = not_alive_finding(&dead, &[]).unwrap();
        assert_eq!(
            (found.kind, found.watch_id.as_str()),
            (Kind::NotAlive, r#"checkout:["not-alive"]"#)
        );
        let no_run = &run_findings(&dead, &[held(3)]);
        assert_eq!(no_run[0].kind, Kind::NoRun);
        assert_eq!(no_run[0].watch_id, found.watch_id, "claims held are the same condition");
        assert_eq!(not_alive_finding(&dead, no_run), None);
        assert_eq!(not_alive_finding(&run(vec![]), &[]), None, "alive");
    }

    #[test]
    fn a_failed_tick_is_a_finding_unless_an_unrecovered_step_says_it() {
        let step = log_findings(&run(vec![tick(
            4,
            json!([{"do": "dispatch", "issue": 3, "error": "busy"}]),
        )]));
        let watching = |errors: Value| {
            json!({"ticks": [{"instance": "alpha", "first": 10, "last": 10, "count": 1,
                "line": "FAILED", "failed": true, "did": [], "errors": errors}]})
        };
        let said = json!([{"do": "dispatch", "issue": 3, "error": "busy"}]);
        assert_eq!(watching::tick_findings(&watching(said.clone()), &step), []);
        let whole = watching::tick_findings(&watching(json!([{"error": "gh is down"}])), &step);
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].kind, Kind::TickFailed);
        assert_eq!(whole[0].watch_id, r#"checkout:["tick","alpha",[["step","tick",null,null]]]"#);
        let recovered = watching::tick_findings(&watching(said), &[]);
        assert_eq!(
            recovered[0].watch_id,
            r#"checkout:["tick","alpha",[["step","dispatch",3,null]]]"#
        );
        let quiet = json!({"ticks": [{"instance": "alpha", "first": 10, "last": 10, "count": 1,
            "line": "ok", "failed": false, "did": [], "errors": []}]});
        assert_eq!(watching::tick_findings(&quiet, &[]), []);
    }

    #[test]
    fn a_recovered_step_leaves_no_finding() {
        let failure =
            json!({"tick": 21, "do": "advance", "issue": 160, "error": "no live terminal"});
        let mut ticks = vec![tick(21, json!([failure.clone()]))];
        for count in 22..=30 {
            let mut recovered = tick(count, json!([]));
            recovered.1.answer.as_mut().unwrap()["did"] =
                json!([{"do": "advance", "issue": 160, "outcome": "nothing"}]);
            ticks.push(recovered);
        }
        assert_eq!(log_findings(&run(ticks)), []);
    }

    #[test]
    fn an_issue_longer_at_its_stage_than_any_before_it_took_names_show() {
        let mut state = state();
        state.team = team();
        // the two stages before took 300s and 1000s; it has been here for 2000s
        let long = Stood { recorded: vec![NOW - 3300, NOW - 3000, NOW - 2000], ..stood(7) };
        state.stood = vec![long.clone()];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "#7 has been at the stage `build` for 33m"),
            ["afkconvoy show --issue 7"]
        );
        assert!(
            found[0].evidence.contains("the longest between two earlier records on this issue, excluding gaps under a minute is 16m"),
            "{found:?}"
        );
        // no longer than one before it; with nothing before it; or waiting on a human
        let shorter = Stood { recorded: vec![NOW - 5000, NOW - 2000], ..stood(7) };
        let first = Stood { recorded: vec![NOW - 2000], ..stood(7) };
        let waiting = Stood { role: None, ..long.clone() };
        // …or long by the others' measure, and still within what any stage takes
        let quick = Stood { recorded: vec![NOW - 400, NOW - 390, NOW - 300], ..stood(7) };
        state.stood = vec![shorter, first, waiting, quick];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    #[test]
    fn long_stage_uses_the_repo_baseline_before_earlier_quick_records() {
        let baselines = [("build".to_string(), (3600, 20))].into();
        let issue = Stood { recorded: vec![NOW - 1500, NOW - 1440, NOW - 1380], ..stood(7) };
        assert!(issue_findings(std::slice::from_ref(&issue), &team(), &baselines, NOW).is_empty());
        let found = issue_findings(&[issue], &team(), &baselines, NOW + 2300);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::LongAtStage);
        assert!(found[0].evidence.contains("repo p95 for `build` from 20"));
        assert!(found[0].evidence.contains("1h"));
        let quick = Stood { recorded: vec![NOW - 2100, NOW - 2050, NOW - 2000], ..stood(8) };
        assert!(issue_findings(&[quick], &team(), &Default::default(), NOW).is_empty());
    }

    #[test]
    fn an_issue_one_rework_from_a_breaker_names_show() {
        let mut state = state();
        state.team = team();
        let in_total = Stood { rework: (2, 0), ..stood(7) };
        let in_a_row = Stood { in_a_row: vec![("review".into(), 2, 0)], ..stood(8) };
        state.stood = vec![in_total, in_a_row];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "is one rework from a breaker"),
            ["afkconvoy show --issue 7", "afkconvoy show --issue 8"]
        );
        assert!(found[0].evidence.contains("`max_rework`"), "{found:?}");
        assert!(found[1].evidence.contains("the stage `review` rejected it 2 time(s) in a row"));
        // with rework left, or in a workflow that allows none and has had none: none
        let left = Stood { rework: (1, 1), in_a_row: vec![("review".into(), 1, 1)], ..stood(7) };
        let none_allowed =
            Stood { rework: (0, 0), in_a_row: vec![("review".into(), 0, 0)], ..stood(8) };
        state.stood = vec![left, none_allowed];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    #[test]
    fn a_frontier_nothing_was_dispatched_from_names_the_failed_step_or_the_dispatch() {
        let mut state = state();
        let waiting = |number| Dispatchable {
            number,
            title: format!("issue {number}"),
            stage: None,
            cast: None,
        };
        state.set.frontier = vec![waiting(3), waiting(5)];
        // the last tick left the frontier with a slot free, and nothing failed in it
        let left = |held: u64| {
            let (at, mut tick) = tick(9, json!([]));
            tick.answer =
                Some(json!({"did": [], "errors": [], "held": held, "frontier_remaining": 2}));
            (at, tick)
        };
        let found = found(&host(), &run(vec![left(1)]), &state);
        assert_eq!(
            fixes(&found, "the frontier holds #3, #5"),
            ["afkconvoy dispatch --issue 3 --instance alpha"]
        );
        assert!(found[0].evidence.starts_with("tick 9: "), "{found:?}");
        // …or its start failed: the step that failed is what to fix
        let error =
            "orca knows no repo at /r — add this checkout once with `orca repo add --path /r`";
        let failed = tick(9, json!([{"do": "dispatch", "issue": 3, "error": error}]));
        let found = self::found(&host(), &run(vec![failed]), &state);
        assert_eq!(fixes(&found, "the frontier holds"), ["orca repo add --path /r"]);

        // the last tick was at its bound, or dispatched: the frontier only waits for the next
        assert_eq!(self::found(&host(), &run(vec![left(2)]), &state), []);
        let (at, mut started) = left(1);
        started.answer.as_mut().unwrap()["did"] =
            json!([{"do": "dispatch", "issue": 1, "outcome": "done"}]);
        assert_eq!(self::found(&host(), &run(vec![(at, started)]), &state), []);
        // no slot is free now, or the frontier is empty: none either
        state.set.free_slots = 0;
        assert_eq!(self::found(&host(), &run(vec![left(1)]), &state), []);
        state.set.free_slots = 2;
        state.set.frontier = vec![];
        assert_eq!(self::found(&host(), &run(vec![left(1)]), &state), []);
    }

    #[test]
    fn a_ready_issue_held_off_by_a_pr_the_fleet_did_not_open_names_closing_it() {
        let mut state = state();
        state.not_ours = vec![NotOurs { issue: 45, pr: 155, branch: "someone/fix".into() }];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "#45 is held off by a PR not the fleet's: #155"),
            ["gh pr close 155"]
        );
        assert_eq!(found[0].severity, Severity::Warning);
        assert!(found[0].evidence.contains("someone/fix"), "{found:?}");
    }

    #[test]
    fn a_flow_label_that_names_no_workflow_names_flows() {
        let mut state = state();
        state.team = team();
        let why = "its flow label flow:bogus names no workflow of the team";
        let lost = Stood {
            flow_labels: vec!["flow:bogus".into()],
            workflow: Err(why.into()),
            stage: None,
            role: None,
            ..stood(7)
        };
        state.stood = vec![lost, stood(8)];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(fixes(&found, "#7 follows no workflow of the team"), ["afkconvoy flows"]);
        assert!(
            found[0]
                .evidence
                .contains("it carries flow:bogus; the team's workflows take flow:feature")
        );
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn a_lock_that_cannot_be_worked_is_critical_and_names_the_label_to_put_on_it() {
        let mut state = state();
        state.set.locks = vec![9];
        state.set.excluded =
            vec![excluded(9, "no ready-for-agent label"), excluded(4, "a fleet lock stands (#9)")];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "the fleet lock #9 stands and cannot be worked"),
            ["gh issue edit 9 --add-label ready-for-agent"]
        );
        assert_eq!(found[0].severity, Severity::Critical);
        assert!(found[0].evidence.ends_with("no ready-for-agent label"), "{found:?}");

        // a bound repo's: the flow label of its team's one workflow, or `flows` to choose
        state.team = team();
        state.set.excluded = vec![excluded(9, "no flow label")];
        let found = self::found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "the fleet lock #9"),
            ["gh issue edit 9 --add-label flow:feature"]
        );
        let TeamRead::Loaded { flow_labels, .. } = &mut state.team else { unreachable!() };
        flow_labels.push("flow:fix".into());
        assert_eq!(
            fixes(&self::found(&host(), &run(vec![]), &state), "the fleet lock #9"),
            ["afkconvoy flows"]
        );
        // held up by something else: the issue itself says what
        state.set.excluded = vec![excluded(9, "open blockers: #41")];
        assert_eq!(
            fixes(&self::found(&host(), &run(vec![]), &state), "the fleet lock #9"),
            ["gh issue view 9"]
        );

        // a lock on the frontier, or one somebody holds, is being worked
        state.set.excluded = vec![];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
        state.set.excluded = vec![excluded(9, "already claimed")];
        state.set.peer_live =
            vec![PeerClaim { number: 9, instance: Some("beta".into()), sha: "c9".into() }];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
        state.set.peer_live = vec![];
        state.set.mine = vec![Mine {
            number: 9,
            title: None,
            status: Status::NoPr,
            pr: None,
            due: None,
            declared: false,
            idle: false,
        }];
        state.held = vec![held(9)];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    #[test]
    fn a_patrol_that_failed_or_is_full_names_the_issue_that_holds_it() {
        let mut state = state();
        state.team = team();
        let failed = Row { failure: Some(12), ..patrol(Standing::Failed) };
        let full = Row { name: "sweep".into(), open: 2, ..patrol(Standing::Full) };
        state.set.patrols = vec![failed, full];
        let opened = |number| Stood { opened_by: Some("sweep".into()), ..stood(number) };
        state.stood = vec![opened(4), opened(6), stood(8)];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "the patrol `triage` does not run: its last run failed"),
            ["gh issue close 12"]
        );
        assert_eq!(fixes(&found, "the patrol `sweep` does not run"), ["afkconvoy show --issue 4"]);
        assert!(found[1].evidence.contains("2 open of 2: #4, #6"), "{found:?}");
        // however else one stands, it is none
        let others = [
            Standing::Due,
            Standing::NotDue,
            Standing::Locked,
            Standing::Running,
            Standing::PeerRunning,
            Standing::StaleRun,
        ];
        state.set.patrols = others.into_iter().map(patrol).collect();
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    #[test]
    fn a_team_with_a_fault_at_its_head_is_critical_and_names_each_file_and_key() {
        let mut state = state();
        let fault = |file: &str, key: &str| Fault {
            file: file.into(),
            key: key.into(),
            problem: "is not a role of the team".into(),
        };
        state.team = TeamRead::Faulty {
            name: "acme/team".into(),
            faults: vec![fault("workflows/feature.md", "stages[1].by"), fault("team.md", "")],
        };
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "the team acme/team has a fault at its head"),
            ["workflows/feature.md: stages[1].by", "team.md"]
        );
        assert!(found.iter().all(|finding| finding.severity == Severity::Critical));
        state.team = TeamRead::Unread("gh api failed".into());
        assert_eq!(
            fixes(&self::found(&host(), &run(vec![]), &state), "cannot be read"),
            ["afkconvoy check"]
        );
        state.team = team();
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    fn under() -> Under {
        Under {
            stage: "build".into(),
            role: "dev".into(),
            role_commit: "dev-0".into(),
            team_commit: "team-0".into(),
        }
    }

    fn moved(which: &str, started: &str, head: &str) -> Moved {
        Moved { which: which.into(), started: started.into(), head: head.into() }
    }

    #[test]
    fn what_moved_since_a_stage_in_flight_started_is_said_and_no_more() {
        let mut state = state();
        state.team = team();
        let both =
            vec![moved("the team", "team-0", "team-1"), moved("the role `dev`", "dev-0", "dev-1")];
        state.held = vec![Held { under: Some(under()), moved: both.clone(), ..held(3) }];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "the team moved since the stage `build` of #3 started"),
            ["afkconvoy check"]
        );
        assert_eq!(
            fixes(&found, "the role `dev` moved since the stage `build` of #3 started"),
            ["afkconvoy check"]
        );
        assert_eq!(found[1].evidence, "it started under dev-0; the head is dev-1");
        assert!(found.iter().all(|finding| finding.severity == Severity::Info));
        // a stage neither has moved under: none
        state.held = vec![Held { under: Some(under()), ..held(3) }];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
        // a claim that is no longer at work says nothing of where it started
        state.held =
            vec![Held { status: Status::Closed, under: Some(under()), moved: both, ..held(3) }];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    #[test]
    fn a_config_that_declares_no_gate_limit_is_said_and_one_that_does_is_not() {
        let found = |yaml: &str| {
            let config = config::parse(&format!(
                "```yaml\nbase_branch: main\ngate_command: make\n{yaml}```\n"
            ));
            let (host, run, state) = (host(), run(vec![]), state());
            let seen = Seen {
                now: NOW,
                host: &host,
                checkout: Path::new("/repos/widgets"),
                config: config.as_ref().ok(),
                run: &run,
                state: Ok(&state),
            };
            findings(&seen)
        };
        let undeclared = found("");
        assert_eq!(
            fixes(&undeclared, "declares no gate limit: the gate may run for 3600s"),
            ["docs/agents/afkconvoy.md: gate_seconds"]
        );
        assert_eq!(undeclared[0].severity, Severity::Info);
        assert!(undeclared[0].evidence.contains("sets no `gate_seconds`"), "{undeclared:?}");
        assert_eq!(found("gate_seconds: 600\n"), []);
        assert_eq!(found("gate_seconds: 3600\n"), [], "the default, declared");
    }

    #[test]
    fn a_config_that_names_a_concurrency_is_said_with_the_line_that_keeps_it_on_the_host() {
        // the config as `yaml` has it, with the concurrency its checkout
        // says on this host laid over it: none at 0
        let found = |yaml: &str, own: usize| {
            let config = config::parse(&format!(
                "```yaml\nbase_branch: main\ngate_command: make\ngate_seconds: 600\n{yaml}```\n"
            ));
            let config = config.unwrap().laid(own);
            let (host, run, state) = (host(), run(vec![]), state());
            let checkout = Path::new("/repos/widgets");
            let config = Some(&config);
            findings(&Seen {
                now: NOW,
                host: &host,
                checkout,
                config,
                run: &run,
                state: Ok(&state),
            })
        };
        // the checkout says none: the default holds, and the line that keeps
        // the config's on this host comes before deleting the key
        let named = found("concurrency: 5\n", 0);
        let what = "this repo's config names `concurrency: 5`, which nothing reads any more: this \
                    checkout's fleet instance holds up to 3 claims";
        assert_eq!(
            fixes(&named, what),
            ["afkconvoy run /repos/widgets --concurrency 5, then delete `concurrency` from \
                 docs/agents/afkconvoy.md"]
        );
        assert_eq!((named.len(), named[0].severity), (1, Severity::Info));
        assert!(
            named[0].evidence.contains("says none on this host, so the default, 3"),
            "{named:?}"
        );
        // the checkout says its own: the key is all there is left to delete
        let own = found("concurrency: 5\n", 6);
        assert_eq!(fixes(&own, "holds up to 6 claims"), ["docs/agents/afkconvoy.md: concurrency"]);
        assert!(own[0].evidence.contains("this checkout says 6 on this host"), "{own:?}");
        // the config names the number already in force: no run is needed,
        // whether the checkout says its own or holds the default
        for (named, own) in [(3, 0), (6, 6)] {
            let same = found(&format!("concurrency: {named}\n"), own);
            assert_eq!(
                fixes(&same, "which nothing reads any more"),
                ["docs/agents/afkconvoy.md: concurrency"]
            );
            assert_eq!(same[0].severity, Severity::Info);
        }
        // the key deleted: nothing, whatever the checkout says
        assert_eq!(found("", 6), []);
        assert_eq!(found("", 0), []);
    }

    #[test]
    fn state_that_cannot_be_read_is_critical_and_findings_come_most_severe_first() {
        let why = "gh issue list failed: HTTP 401";
        let found = findings(&Seen {
            now: NOW,
            host: &host(),
            checkout: Path::new("/repos/widgets"),
            config: None,
            run: &run(vec![]),
            state: Err(why),
        });
        assert_eq!(fixes(&found, "the fleet's state cannot be read"), ["afkconvoy scan"]);
        assert_eq!(found[0].evidence, why);

        let mut state = state();
        state.team = team();
        state.stood = vec![Stood { rework: (2, 0), ..stood(7) }];
        state.held = vec![Held {
            under: Some(under()),
            moved: vec![moved("the role `dev`", "dev-0", "dev-1")],
            ..held(7)
        }];
        state.set.locks = vec![9];
        state.set.excluded = vec![excluded(9, "no flow label")];
        let found = self::found(&host(), &run(vec![]), &state);
        let severities: Vec<Severity> = found.iter().map(|finding| finding.severity).collect();
        assert_eq!(severities, [Severity::Critical, Severity::Warning, Severity::Info]);
    }

    #[test]
    fn the_last_tick_says_the_seconds_it_took_from_its_own_start() {
        let entry = |at: u64, instance: &str, what: What| Entry {
            at,
            instance: instance.into(),
            host: "h1".into(),
            what,
        };
        let start = What::Start { worker_command: "claude".into(), harness_commands: [].into() };
        let started = |count| What::TickStarted { count, cause: Cause::Timer, by: None };
        let mut entries = vec![
            entry(10, "alpha", start),
            entry(10, "alpha", started(1)),
            entry(14, "alpha", What::Tick(tick(1, json!([])).1)),
            entry(104, "alpha", started(2)),
            entry(116, "alpha", What::Tick(tick(2, json!([])).1)),
        ];
        assert_eq!(run_log::last_took(&entries, "h1"), Some(12));
        entries.push(entry(206, "alpha", started(3)));
        assert_eq!(
            run_log::last_took(&entries, "h1"),
            Some(12),
            "a tick under way is not the last"
        );
        let cut: Vec<Entry> = entries.drain(4..).collect();
        assert_eq!(run_log::last_took(&cut, "h1"), None, "the log kept no start of it");
        let mut answer = run(vec![(116, tick(2, json!([])).1)]);
        answer.last_took = Some(12);
        let said = &fleet_json(&answer)["last_tick"];
        assert_eq!((said["sleep"].as_u64(), said["took"].as_u64()), (Some(90), Some(12)));
    }
}

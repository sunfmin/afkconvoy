//! `afkconvoy doctor`: the state of the fleet on this host, and what is wrong
//! with it — each finding with the line to run, or the file and key to edit.
//!
//! `read` is the half that asks: this host, the log of its `run`, GitHub and
//! the remote, orca after each worker. It writes nothing anywhere but the
//! fleet's own cache, and makes no startup probe: it is run beside a live
//! fleet, often. `findings` is the half that judges what was read, and asks
//! nothing. Nothing here fixes anything: the fleet's own commands do.

use std::path::PathBuf;

use afkconvoy_core::casting;
use afkconvoy_core::claims::{InstanceId, claim_ref};
use afkconvoy_core::config::Config;
use afkconvoy_core::exclusive::Ticket;
use afkconvoy_core::gate::GateCommand;
use afkconvoy_core::lease::{LEASE, stamp_age, stamp_stale};
use afkconvoy_core::left::{Kept, KeptBecause};
use afkconvoy_core::patrol::Standing;
use afkconvoy_core::plan::{LabelBar, NOT_OURS, Status, WorkingSet, closing_pr};
use afkconvoy_core::slow_gate;
use afkconvoy_core::team::Fault;
use afkconvoy_core::tools::{Found, START_ORCA, Tool, add_checkout, harness_missing};
use afkconvoy_core::waiting::{WAITED_LONG, Waiting};
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::admission::Admission;
use crate::checkout_list::{List, repo_of};
use crate::config;
use crate::exclusive::Queue;
use crate::fleet::Fleet;
use crate::gate::{head_tree, recorded};
use crate::git::{REMOTE, Repo};
use crate::host_runner;
use crate::judge::leaving;
use crate::launch::Launches;
use crate::log::{self, Entry, Failed, Log, Tick, What, by_cause};
use crate::orca::{Source, Under, Worktree};
use crate::panel::lasted;
use crate::role::{self, Checked};
use crate::stage::Stood;
use crate::team::{self, Refused};
use crate::tools;
use crate::topic::See;

/// How many ticks in a row a step fails in before it is a finding.
const FAILED_TICKS: usize = 3;
/// How many failed steps the answer lists, newest first.
pub(crate) const FAILED_SHOWN: usize = 10;
/// How many of the events that reached a `run` the answer lists, newest first.
pub(crate) const EVENTS_SHOWN: usize = 20;
/// How long a worker is stopped for before it is a finding: past every wait
/// a tick gives one before it nudges.
const STOPPED_LONG: u64 = 1800;
/// How long an issue is at a stage before its stay is held against the
/// stages before it.
const STAGE_LONG: u64 = 900;

/// What the answer says of a host whose `run` left no log.
const NO_LOG: &str = "no log";

/// How much a finding matters, most first: `doctor --help` lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, agent_help::Help)]
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
}

/// Names each kind of finding there is, with how much it matters and its
/// symptom: the words its `what` always holds.
macro_rules! kinds {
    ($($kind:ident: $severity:ident, $symptom:literal;)+) => {
        /// One kind of thing `doctor` finds wrong: what a finding is made as.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
                    Kind::LockUnworked | Kind::PatrolFailed | Kind::WaitedLong => See::Humans,
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
    StepFailed: Critical, "failed in each of the last";
    HarnessDown: Critical, "harness is down";
    HarnessPaused: Warning, "is paused on this host";
    RosterModelUnmatched: Warning, "names no model the host kit holds";
    TeamUnread: Critical, "the team this repo names cannot be read";
    TeamFaulty: Critical, "has a fault at its head";
    NothingSafe: Warning, "has no harness on this host it holds limits for";
    RosterUnmatched: Warning, "matches nothing in the host kit";
    LockUnworked: Critical, "stands and cannot be worked";
    StateUnread: Critical, "the fleet's state cannot be read";
    NoWorker: Warning, "has no worker";
    WorkerStopped: Warning, "has been stopped for";
    NothingStarted: Warning, "the last tick started no worker";
    NoWorkflow: Warning, "follows no workflow of the team";
    LongAtStage: Warning, "longer than any stage before it took";
    NearBreaker: Warning, "is one rework from a breaker";
    PatrolFailed: Warning, "does not run: its last run failed";
    PatrolFull: Warning, "have reached its `max_open`";
    WaitedLong: Warning, "has waited for a human for";
    NotOurs: Warning, "is held off by a PR not the fleet's";
    ClaimOnGone: Warning, "is claimed but GitHub has it no more";
    SlowGate: Warning, "the gate is slow";
    Moved: Info, "moved since the stage";
    KeptWorktree: Info, "is kept with no claim";
    NoGateLimit: Info, "declares no gate limit";
}

impl Kind {
    /// Whether what a finding of this kind says is wrong is the host's, the
    /// same whichever checkout `doctor` is asked in: a command missing or
    /// logged out, orca not running, a harness paused.
    fn of_host(self) -> bool {
        matches!(
            self,
            Kind::ToolMissing
                | Kind::ToolLoggedOut
                | Kind::NoProvider
                | Kind::OrcaNotRunning
                | Kind::HarnessPaused
        )
    }
}

pub(crate) fn finding(kind: Kind, what: String, evidence: String, fix: String) -> Finding {
    debug_assert!(what.contains(kind.symptom()), "{kind:?} is not said in: {what}");
    let (severity, see, of_host) = (kind.severity(), kind.see(), kind.of_host());
    Finding { severity, what, evidence, fix, see, of_host }
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
    /// Its last check for host sleep, as its log has it; `None`: it made none.
    pub host_sleep: Option<HostSleepChecked>,
    /// The newest events that reached this host's `run`s, newest first.
    pub events: Vec<Event>,
    /// The workers its log says are hot: each turned hot, and has not
    /// stopped being hot since.
    pub hot: Vec<HotRead>,
    /// Its fleet instance broken, as its log has it: the host runner starts
    /// it again after a backoff.
    pub broken: Option<BrokenRead>,
}

/// A worker that is hot, as the log has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotRead {
    /// `#7`, or the name of its worktree.
    pub worker: String,
    /// When it turned hot, why, and what it held as it did, in words.
    pub since: u64,
    pub why: String,
    pub held: String,
}

/// A fleet instance that is broken, as the log has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrokenRead {
    /// Why it broke, as it last did.
    pub why: String,
    /// When it broke: a start of it again that failed since is no end of it.
    pub since: u64,
    /// When the host runner starts it again; `None`: no host runner runs on
    /// this host to.
    pub next: Option<u64>,
    /// The fleet instance that broke; empty where it broke before it was any.
    pub instance: String,
}

/// One event that reached a `run` from outside it: a wake, its start or
/// stop, the host put to sleep or waking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub at: u64,
    /// `wake`, `start`, `stop`, `host_sleep` or `host_wake`.
    pub event: &'static str,
    /// What caused it, in words.
    pub cause: String,
    pub instance: String,
}

/// A check for host sleep, as the log has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSleepChecked {
    /// When it ended.
    pub at: u64,
    /// Whether it put the host to sleep.
    pub slept: bool,
    /// What kept the host awake, each in words.
    pub awake: Vec<String>,
}

impl Run {
    fn last_tick(&self) -> Option<&(u64, Tick)> {
        self.ticks.as_ref().and_then(|ticks| ticks.last())
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
        matches!(self.status, Status::NoPr | Status::Joining | Status::AtStage)
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
    pub set: WorkingSet,
    pub held: Vec<Held>,
    pub stood: Vec<Stood>,
    pub team: TeamRead,
    pub ready_label: String,
    /// The worktrees on this host whose issue holds no claim.
    pub unclaimed: Vec<Unclaimed>,
    /// The open ready issues held off, with no claim, only by an open PR
    /// that is not the fleet's (`plan::NOT_OURS`).
    pub not_ours: Vec<NotOurs>,
    /// The claims on issues GitHub says were deleted or transferred.
    pub on_gone: Vec<OnGone>,
}

/// A claim on an issue GitHub answers 410 or 404 for: deleted, or
/// transferred to another repo. Its holder's next tick lets it go; until
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

/// A worktree on this host whose issue holds no claim: one a park or an
/// escalation kept, or an older afkconvoy left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unclaimed {
    pub issue: u64,
    pub path: PathBuf,
    pub branch: String,
    /// Why it is kept; `None` when it holds nothing the remote does not.
    pub kept: Option<Kept>,
}

/// Everything `doctor` read.
pub struct Seen<'a> {
    pub now: u64,
    pub host: &'a Host,
    /// The repo's config; `None` where it cannot be read.
    pub config: Option<&'a Config>,
    pub run: &'a Run,
    /// The fleet's state, or why it could not be read.
    pub state: Result<&'a State, &'a str>,
}

/// The first command line `error` itself names, between backticks: an error
/// of the fleet's says the line that fixes it where it knows one.
fn named_line(error: &str) -> Option<String> {
    let quoted = error.split('`').skip(1).step_by(2);
    quoted.map(str::trim).find(|line| line.contains(' ')).map(str::to_string)
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
                tool.install().to_string(),
            ),
            Found::LoggedOut => (
                if *tool == Tool::Pi { Kind::NoProvider } else { Kind::ToolLoggedOut },
                format!("`{name}` {}", tool.logged_out()),
                format!(
                    "`{}` says so, in a login shell{called}",
                    tool.login_question(gh_host).unwrap_or_default()
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
        found.push(finding(kind, what, evidence, fix));
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
fn claim_findings(run: &Run, held: &[Held]) -> Vec<Finding> {
    let mut found = vec![];
    let instance = run.instance.as_deref().unwrap_or_default();
    for claim in held.iter().filter(|claim| claim.at_work()) {
        let issue = claim.issue;
        let place = match &claim.worktree {
            Some(worktree) => worktree.display().to_string(),
            None => "no worktree".to_string(),
        };
        match claim.worker {
            // With no `run` alive nothing is about to start one: that is the finding.
            Some(WorkerState::Gone) if run.alive => {
                let (evidence, fix) = match (&claim.worktree, claim.status) {
                    (None, _) => (format!("orca links no worktree to #{issue}"), "dispatch"),
                    (Some(_), Status::Joining) => {
                        (format!("orca has no live terminal in {place}"), "restart")
                    }
                    (Some(_), _) => (format!("orca has no live terminal in {place}"), "dispatch"),
                };
                let held_for =
                    claim.held_for.map(lasted).unwrap_or_else(|| "an unknown time".into());
                found.push(finding(
                    Kind::NoWorker,
                    format!("the claim on #{issue} has no worker"),
                    format!(
                        "{evidence}; the claim was stamped {held_for} ago. A tick starts one on \
                         its own: the fix is for a claim the next tick leaves so"
                    ),
                    on(fix, issue, instance),
                ));
            }
            Some(WorkerState::Idle { quiet: Some(quiet) }) if quiet >= STOPPED_LONG => {
                found.push(finding(
                    Kind::WorkerStopped,
                    format!(
                        "the worker of #{issue} has been stopped for {}, and its claim is still \
                         at work: it declared nothing that ends it",
                        lasted(quiet)
                    ),
                    format!("orca reports it stopped, in {place}: look at its terminal there"),
                    on("nudge", issue, instance),
                ));
            }
            _ => {}
        }
        let Some(under) = &claim.under else { continue };
        for Moved { which, started, head } in &claim.moved {
            found.push(finding(
                Kind::Moved,
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
        _ => format!("afkconvoy plan --instance {instance}"),
    })
}

/// The same step failed in each of the last ticks.
fn log_findings(run: &Run) -> Vec<Finding> {
    let ticks = run.ticks.as_deref().unwrap_or_default();
    let Some(last) = ticks.len().checked_sub(FAILED_TICKS).map(|from| &ticks[from..]) else {
        return vec![];
    };
    let Some(((_, newest), before)) = last.split_last() else { return vec![] };
    let instance = run.instance.as_deref().unwrap_or_default();
    let again = |failed: &Failed| {
        before.iter().all(|(_, tick)| tick.failed().iter().any(|was| was.same_step(failed)))
    };
    let (first, latest) = (last[0].1.count, newest.count);
    newest
        .failed()
        .iter()
        .filter(|failed| again(failed))
        .map(|failed| {
            finding(
                Kind::StepFailed,
                format!(
                    "`{}` failed in each of the last {FAILED_TICKS} ticks ({first} to {latest})",
                    failed.named()
                ),
                failed.error.clone(),
                fix_of(failed, instance),
            )
        })
        .collect()
}

/// An issue long at one stage, one rework from a breaker, or under a flow
/// label that names no workflow.
fn issue_findings(stood: &[Stood], team: &TeamRead, now: u64) -> Vec<Finding> {
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
        let took = issue.recorded.windows(2).map(|pair| pair[1].saturating_sub(pair[0])).max();
        let here = issue.recorded.last().and_then(|last| stamp_age(*last, now));
        if let Some((took, here)) =
            took.zip(here).filter(|(took, here)| here > took && *here >= STAGE_LONG)
        {
            found.push(finding(
                Kind::LongAtStage,
                format!(
                    "#{number} has been at the stage `{stage}` for {}, longer than any stage \
                     before it took",
                    lasted(here)
                ),
                format!(
                    "its last stage record is {} old; the longest between two before it is {}",
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
            found.push(finding(
                Kind::NothingStarted,
                format!(
                    "the frontier holds {}, {} slot(s) are free, and the last tick started no worker",
                    numbered(&set.frontier.iter().map(|issue| issue.number).collect::<Vec<_>>()),
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
        let label = |label: &str| format!("gh issue edit {lock} --add-label {label}");
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
            format!("gh issue view {lock}")
        };
        found.push(finding(
            Kind::LockUnworked,
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
                format!("gh issue close {failure}"),
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
            format!("the patrol `{name}` does not run: {why}"),
            evidence,
            fix,
        ));
    }
    found
}

/// An issue has waited for a human for long: nothing in the fleet moves it.
fn waiting_findings(set: &WorkingSet, now: u64) -> Vec<Finding> {
    let long = |waiting: &Waiting| {
        let waited = waiting.waited_long(now)?;
        Some(finding(
            Kind::WaitedLong,
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
        ))
    };
    set.waiting_on_human.iter().filter_map(long).collect()
}

/// A worktree kept on this host with no claim on its issue: what it holds
/// that the remote does not, and the line that removes it — pushing first
/// whatever only it holds.
fn unclaimed_finding(unclaimed: &Unclaimed) -> Finding {
    let (path, branch) = (unclaimed.path.display(), &unclaimed.branch);
    let remove = format!("orca worktree rm --worktree path:{path} --force");
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
        format!("#{issue} is claimed but GitHub has it no more: it was deleted or transferred"),
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
        format!(
            "this repo's config declares no gate limit: the gate may run for {seconds}s before \
             anything ends it"
        ),
        format!("{} sets no `gate_seconds`, so the gate limit is {seconds}s", config::FILE),
        format!("{}: gate_seconds", config::FILE),
    ))
}

/// Everything that is wrong in what `seen` holds, most severe first.
pub fn findings(seen: &Seen) -> Vec<Finding> {
    let mut found = host_findings(seen.host);
    found.extend(seen.config.and_then(config_finding));
    found.extend(log_findings(seen.run));
    found.extend(broken_finding(seen.run));
    match seen.state {
        Ok(state) => {
            found.extend(run_findings(seen.run, &state.held));
            found.extend(team_findings(&state.team));
            found.extend(plan_findings(seen.run, state));
            found.extend(claim_findings(seen.run, &state.held));
            found.extend(issue_findings(&state.stood, &state.team, seen.now));
            found.extend(waiting_findings(&state.set, seen.now));
            found.extend(state.unclaimed.iter().map(unclaimed_finding));
            found.extend(state.not_ours.iter().map(not_ours_finding));
            found.extend(state.on_gone.iter().map(on_gone_finding));
        }
        Err(why) => found.push(finding(
            Kind::StateUnread,
            "the fleet's state cannot be read: GitHub, the remote or the config".to_string(),
            why.to_string(),
            named_line(why).unwrap_or_else(|| "afkconvoy scan".to_string()),
        )),
    }
    found.sort_by_key(|finding| finding.severity);
    found
}

/// What this host has of what the fleet of the checkout `repo`, under
/// `config`, needs, read from its current host kit.
fn read_host(repo: &Repo, config: Option<&Config>) -> Host {
    let launches = Launches::foreseen(repo);
    let default = launches.default();
    let looked = tools::look(repo, launches.of(default).ok().flatten());
    // An unbound repo knows of no harness: nothing calls for its one command's.
    let called = match config.and_then(|config| config.bound.as_ref()) {
        Some(bound) => {
            let team = role::current(repo, bound, &config.unwrap().labels());
            role::called_for(repo, team.as_ref())
        }
        None => vec![(default, vec![])],
    };
    let checked = role::check(&launches, &called).unwrap_or_default();
    let launch = |checked: Checked| Launch { checked };
    Host {
        tools: looked.found,
        gh_host: looked.host,
        orca_unasked: looked.orca_unasked,
        launches: checked.into_iter().map(launch).collect(),
    }
}

/// The worker launch commands a `run` said it started with: the first word
/// of each, retained only as log evidence, never used to select commands.
type StartedWith = Vec<String>;

/// What the log says of the `run` of `host`: its fleet instance, the worker
/// launch commands it started with, and that instance's ticks.
pub(crate) fn read_log(
    entries: &[Entry],
    host: &str,
) -> (Option<String>, Option<StartedWith>, Vec<(u64, Tick)>) {
    // A command's line names the instance it acted as, not the `run`'s.
    let Some(newest) = entries.iter().rev().find(|entry| entry.host == host && entry.of_run())
    else {
        return (None, None, vec![]);
    };
    let own = || entries.iter().filter(|entry| entry.instance == newest.instance);
    let command = own().rev().find_map(|entry| match &entry.what {
        What::Start { worker_command, harness_commands } => {
            let named = harness_commands.iter().map(|(harness, word)| format!("{harness}={word}"));
            Some(std::iter::once(worker_command.clone()).chain(named).collect())
        }
        _ => None,
    });
    let ticks = own().filter_map(|entry| match &entry.what {
        What::Tick(tick) => Some((entry.at, tick.clone())),
        _ => None,
    });
    (Some(newest.instance.clone()), command, ticks.collect())
}

/// The last check for host sleep the newest `run` of `host` made, as the log
/// has it.
fn read_host_sleep(entries: &[Entry], host: &str) -> Option<HostSleepChecked> {
    let newest = entries.iter().rev().find(|entry| entry.host == host && entry.of_run())?;
    let own = entries.iter().filter(|entry| entry.instance == newest.instance);
    own.rev().find_map(|entry| match &entry.what {
        What::HostSleep { slept, awake } => {
            Some(HostSleepChecked { at: entry.at, slept: *slept, awake: awake.clone() })
        }
        _ => None,
    })
}

/// The workers the newest `run` of `host` found hot and has not found
/// stopped being hot since, as the log has them, longest hot first.
pub(crate) fn read_hot(entries: &[Entry], host: &str) -> Vec<HotRead> {
    let Some(newest) = entries.iter().rev().find(|entry| entry.host == host && entry.of_run())
    else {
        return vec![];
    };
    let mut hot: Vec<HotRead> = vec![];
    for entry in entries.iter().filter(|entry| entry.instance == newest.instance) {
        match &entry.what {
            What::Hot { worker, why, held } => {
                hot.retain(|read| &read.worker != worker);
                let (worker, why, held) = (worker.clone(), why.clone(), held.clone());
                hot.push(HotRead { worker, since: entry.at, why, held });
            }
            What::Cooled { worker, .. } => hot.retain(|read| &read.worker != worker),
            _ => {}
        }
    }
    hot
}

/// The fleet instance of `host` broken, as the log has it: its newest line
/// of a start, a break, a coming back or a stop is a break. `runs` says whether a
/// host runner runs on this host, which starts it again.
pub(crate) fn read_broken(entries: &[Entry], host: &str, runs: bool) -> Option<BrokenRead> {
    let of_host = entries.iter().rev().filter(|entry| entry.host == host);
    // A `stop:` line after it broke: `stop` took its checkout off the list.
    let mut starts = of_host.filter(|entry| {
        matches!(
            entry.what,
            What::Start { .. } | What::Broken { .. } | What::Back { .. } | What::Stop { .. }
        )
    });
    let newest = starts.next()?;
    let What::Broken { why, since, next } = &newest.what else { return None };
    let (why, since, next) = (why.clone(), *since, runs.then_some(*next));
    Some(BrokenRead { why, since, next, instance: newest.instance.clone() })
}

/// The events that reached the `run`s of `host` (`Entry::event`), as the log
/// has them: the newest `EVENTS_SHOWN`, newest first, whichever instance they
/// reached — a `run` stopped and started again is two.
fn read_events(entries: &[Entry], host: &str) -> Vec<Event> {
    let of_host = entries.iter().rev().filter(|entry| entry.host == host);
    let event = |entry: &Entry| {
        let (event, cause) = entry.event()?;
        Some(Event { at: entry.at, event, cause, instance: entry.instance.clone() })
    };
    of_host.filter_map(event).take(EVENTS_SHOWN).collect()
}

/// The team the config of the checkout `repo` names, where it is `bound`,
/// as it stands at its head → with the team itself, where it loaded: what a
/// stage in flight is held against.
fn read_team(repo: &Repo, bound: bool) -> (TeamRead, Option<team::Loaded>) {
    if !bound {
        return (TeamRead::Unbound, None);
    }
    match team::load(team::From::Config, Some(repo)) {
        Ok(loaded) => {
            let workflows = loaded.team.workflows.iter();
            let flow_labels = workflows.map(|workflow| workflow.flow_label().to_string()).collect();
            // A host kit that cannot be read is a finding of its own.
            let kit = crate::kit::running().map(|(kit, _)| kit.range());
            let (nothing_safe, unmatched) = match &kit {
                Ok(kit) => (
                    casting::range_faults(&loaded.team, kit),
                    casting::unmatched_entries(&loaded.team, kit),
                ),
                Err(_) => (vec![], vec![]),
            };
            (
                TeamRead::Loaded {
                    flow_labels,
                    nothing_safe,
                    unmatched,
                    unmatched_models: kit
                        .as_ref()
                        .map(|kit| casting::unmatched_models(&loaded.team, kit))
                        .unwrap_or_default(),
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
        let green = worktree.as_ref().and_then(|worktree| {
            let tree = head_tree(&Repo::new(worktree.path.clone())).ok()?;
            recorded(&fleet.repo, &tree, &gate, fleet.now).ok()?
        });
        let at = stood.iter().find(|issue| issue.number == mine.number);
        let stamped = fleet.scan.claim_of(mine.number).and_then(|claim| claim.owner.as_ref());
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

fn read_state(repo: &Repo, now: u64, run: &mut Run) -> Result<State> {
    let (mut fleet, open) = Fleet::read(repo, now)?;
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
    let (set, flows) = fleet.rebuild(&open, instance.as_ref())?;
    let stood = flows.stood();
    let (team, loaded) = read_team(repo, fleet.config.bound.is_some());
    let held = read_held(&fleet, &set, &stood, loaded.as_ref());
    let unclaimed = read_unclaimed(&fleet);
    let ready_label = fleet.config.ready_label.clone();
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
            claim_ref: claim_ref(fleet.repo.layout(), issue),
            holder: fleet
                .scan
                .claim_of(issue)
                .and_then(|claim| claim.owner.as_ref())
                .map(|owner| owner.instance.clone()),
        })
        .collect();
    Ok(State { set, held, stood, team, ready_label, unclaimed, not_ours, on_gone })
}

/// The worktrees on this host whose issue holds no claim, each with what it
/// holds that the remote does not. One that cannot be read is left out.
fn read_unclaimed(fleet: &Fleet) -> Vec<Unclaimed> {
    let Some(linked) = Source::of(&fleet.repo).and_then(|source| Worktree::linked(&source)).ok()
    else {
        return vec![];
    };
    let unclaimed = linked.into_iter().filter(|(issue, _)| fleet.scan.claim_of(*issue).is_none());
    unclaimed
        .filter_map(|(issue, worktree)| {
            let left = leaving(fleet, &worktree).ok()?.left;
            Some(Unclaimed {
                issue,
                kept: left.kept().copied(),
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
            Found::LoggedOut => "logged_out",
            Found::NotRunning if host.orca_unasked.is_some() => "unasked",
            Found::NotRunning => "not_running",
            Found::KnowsNoCheckout(_) => "knows_no_checkout",
        };
        json!({"name": tool.name(), "state": state})
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
            "line": tick.line, "did": answer.map(|answer| &answer["did"]),
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
        "host_sleep": run.host_sleep.as_ref().map(|checked| json!({"checked": checked.at,
            "slept": checked.slept, "kept_awake_by": checked.awake})),
        "events": run.events,
        // Only a `run` that is alive watches its workers.
        "hot_workers": if run.alive { json!(run.hot) } else { json!([]) },
        "broken": run.broken,
    })
}

fn held_json(held: &Held) -> Value {
    let worker = held.worker.map(|worker| match worker {
        WorkerState::Gone => json!({"state": "gone"}),
        WorkerState::Busy => json!({"state": "busy"}),
        WorkerState::Idle { quiet } => json!({"state": "stopped", "quiet": quiet}),
    });
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
    for key in ["frontier", "excluded", "escalating", "locks", "free_slots", "patrols"] {
        issues[key] = plan.get(key).cloned().unwrap_or_else(|| json!([]));
    }
    Ok(issues)
}

/// The host's `exclusive` queue: the command that runs alone, and those that
/// wait their turn, in order — or why it could not be read.
fn exclusive_json(standing: Result<Vec<Ticket>>) -> Value {
    let ticket = |ticket: &Ticket| {
        json!({"command": ticket.command, "dir": ticket.dir, "issue": ticket.issue,
            "pid": ticket.pid, "asked": ticket.asked, "ran": ticket.ran})
    };
    match standing {
        Ok(live) => json!({
            "running": live.first().map(ticket),
            "waiting": live.iter().skip(1).map(ticket).collect::<Vec<_>>(),
        }),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

/// `afkconvoy doctor`, for the fleet of `host` in the checkout `repo`:
///
///   {"host", "fleet", "claims", "issues", "waiting_on_human": [{"issue", "title",
///    "why", "since", "answer"}], "findings": [{"severity", "what", "evidence",
///    "fix"}]}
///
/// An answer with a `critical` finding is an `Unwell` error: the same object,
/// and a non-zero exit. It reads only — but for the fleet's own cache, where
/// reading a team keeps its copies.
pub fn doctor(repo: &Repo, now: u64, host: &str) -> Result<Value> {
    doctor_with_proxy(repo, now, host, &[])
}

/// Adds the network settings of a start the host is considering.
pub fn doctor_with_proxy(repo: &Repo, now: u64, host: &str, proxy: &[String]) -> Result<Value> {
    let (mut answer, found) = examined(repo, now, host, proxy)?;
    let unwell = critical(&found);
    answer["findings"] = serde_json::to_value(found)?;
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

/// What `doctor` makes of one checkout (`examined`): its answer but for
/// `findings`, and what it found, most severe first.
type Examined = (Value, Vec<Finding>);

/// What `doctor` reads of the fleet of `host` in the checkout `repo`, under
/// the network settings `proxy` names.
fn examined(repo: &Repo, now: u64, host: &str, proxy: &[String]) -> Result<Examined> {
    let launches = Launches::foreseen(repo).with_no_proxy(proxy)?;

    let log = Log::of(repo).ok();
    let entries = log.as_ref().and_then(Log::read);
    let (instance, _started_with, ticks) = read_log(entries.as_deref().unwrap_or_default(), host);
    let mut run = Run {
        alive: host_runner::holds(repo),
        instance,
        host: host.to_string(),
        heartbeat_age: None,
        heartbeats: Heartbeats::default(),
        log_file: log.map(|log| log.file),
        host_sleep: read_host_sleep(entries.as_deref().unwrap_or_default(), host),
        hot: read_hot(entries.as_deref().unwrap_or_default(), host),
        broken: read_broken(
            entries.as_deref().unwrap_or_default(),
            host,
            crate::host_runner::runs(),
        ),
        events: read_events(entries.as_deref().unwrap_or_default(), host),
        // A log only commands wrote to is no `run`'s.
        ticks: entries.filter(|entries| entries.iter().any(Entry::of_run)).map(|_| ticks),
    };
    let config = config::load(&repo.dir.join(config::FILE));
    let seen_host = read_host(repo, config.as_ref().ok());
    let state = read_state(repo, now, &mut run).map_err(|error| format!("{error:#}"));
    let seen = Seen {
        now,
        host: &seen_host,
        config: config.as_ref().ok(),
        run: &run,
        state: state.as_ref().map_err(String::as_str),
    };
    let mut found = findings(&seen);
    found.extend(crate::harness_down::findings(repo, run.instance.as_deref())?);
    found.extend(crate::paused::findings(now));
    found.sort_by_key(|finding| finding.severity);
    let (claims, issues, waiting) = match &state {
        Ok(state) => (
            state.held.iter().map(held_json).collect(),
            issues_json(&state.set)?,
            serde_json::to_value(&state.set.waiting_on_human)?,
        ),
        Err(_) => (vec![], Value::Null, Value::Null),
    };
    let mut host = host_json(&seen_host);
    host["worker_no_proxy"] = launches.proxy_json();
    host["admission"] = match Admission::read(&repo.dir) {
        Ok(admission) => admission.json(),
        Err(error) => json!({"error": format!("{error:#}")}),
    };
    // Beside admission's reading: what the last host inspection found of it.
    host["admission"]["inspected"] = crate::inspection::last_on_host()
        .map_or(Value::Null, |last| crate::inspection::last_json(&last));
    host["settings"] = match List::of_host() {
        Ok(list) => crate::host_settings::json(&list, &repo.dir),
        Err(error) => json!({"error": format!("{error:#}")}),
    };
    if let Some(host) = host.as_object_mut() {
        host.extend(host_once(now));
    }
    let answer = json!({
        "host": host,
        "fleet": fleet_json(&run),
        "claims": Value::Array(claims),
        "issues": issues,
        "waiting_on_human": waiting,
    });
    Ok((answer, found))
}

/// What of `doctor`'s `host` is the host's alone, the same whichever
/// checkout it is asked in — the host kit, the queue of `exclusive`, the
/// inspection running — by its key: `doctor --all` says it once.
fn host_once(now: u64) -> Map<String, Value> {
    let inspection = crate::inspection::on_host();
    Map::from_iter([
        ("host_kit".to_string(), crate::kit::show(now)),
        ("exclusive".to_string(), exclusive_json(Queue::of_host().and_then(|q| q.standing()))),
        (
            "inspection".to_string(),
            inspection.map_or(Value::Null, |running| crate::inspection::json(&running)),
        ),
    ])
}

/// `afkconvoy doctor --all`: `doctor` of every checkout on this host's
/// checkout list, in its order, each asked on a thread of its own, as the
/// fleet of `host` — else of the host its `run` named, else of this machine
/// — under the network settings `proxy` names:
///
///   {"host": {"host_kit", "exclusive", "inspection"}, "findings",
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
    let listed = List::of_host()?.read()?.checkouts;
    // Each checkout's repo, where it can be read, beside what was made of it.
    let asked: Vec<(Option<String>, Result<Examined>)> = std::thread::scope(|scope| {
        let asking: Vec<_> = listed
            .iter()
            .map(|entry| {
                let named = host.map(str::to_string).or_else(|| entry.named.host.clone());
                let host = named.unwrap_or_else(crate::this_host);
                scope.spawn(move || {
                    let repo = Repo::at(&entry.checkout);
                    let key = repo.as_ref().ok().and_then(|repo| repo_of(repo).ok());
                    (key, repo.and_then(|repo| examined(&repo, now, &host, proxy)))
                })
            })
            .collect();
        let panicked = || (None, Err(anyhow!("its `doctor` panicked")));
        asking.into_iter().map(|asked| asked.join().unwrap_or_else(|_| panicked())).collect()
    });
    let once = host_once(now);
    let mut of_host: Vec<Finding> = vec![];
    let mut unwell = false;
    let mut checkouts = vec![];
    for (entry, (key, examined)) in listed.iter().zip(asked) {
        let doctor = match examined {
            Ok((mut answer, found)) => {
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
                answer["findings"] = serde_json::to_value(own)?;
                answer
            }
            Err(error) => {
                unwell = true;
                crate::error_json(&error)
            }
        };
        checkouts.push(json!({"checkout": entry.checkout, "repo": key, "doctor": doctor}));
    }
    of_host.sort_by_key(|finding| finding.severity);
    unwell |= critical(&of_host);
    let answer = json!({
        "host": once,
        "findings": of_host,
        "checkouts": checkouts,
    });
    if unwell {
        return Err(Unwell(answer).into());
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use afkconvoy_core::patrol::Row;
    use afkconvoy_core::plan::{Dispatchable, Excluded, Mine, PeerClaim};

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
            host_sleep: None,
            hot: vec![],
            broken: None,
            events: vec![],
        }
    }

    fn set() -> WorkingSet {
        WorkingSet {
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
            proposals: vec![],
            slow_gate: None,
            outgrown_train: None,
            held_back: vec![],
            caster: None,
        }
    }

    fn state() -> State {
        State {
            set: set(),
            held: vec![],
            stood: vec![],
            team: TeamRead::Unbound,
            ready_label: "ready-for-agent".into(),
            unclaimed: vec![],
            not_ours: vec![],
            on_gone: vec![],
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
        let found = findings(&Seen { now: NOW, host, config: None, run, state: Ok(state) });
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
        lacking.tools = vec![(Tool::Gh, Found::LoggedOut), (Tool::Orca, Found::Missing)];
        let found = found(&lacking, &run(vec![]), &state());
        assert_eq!(fixes(&found, "`gh` is not logged in"), ["gh auth login"]);
        assert_eq!(fixes(&found, "`orca` is not installed"), [Tool::Orca.install()]);
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
                    tool: Some((Tool::Pi, found)),
                    fault: Some("\"pi\" resolves to nothing in a login shell".into()),
                    flags_fault: Some("`pi --help` failed in a login shell".into()),
                },
            });
            host
        };
        let missing = pi(Found::Missing);
        let found = self::found(&missing, &run(vec![]), &state());
        assert_eq!(fixes(&found, "`pi` is not installed"), [Tool::Pi.install()]);
        assert_eq!(found.len(), 1, "said once, and not again of its command: {found:?}");
        assert!(
            found[0]
                .evidence
                .ends_with("called for by the stage `build` of the workflow `feature`")
        );
        assert_eq!(found[0].severity, Severity::Critical);
        assert_eq!(host_json(&missing)["tools"][3], json!({"name": "pi", "state": "missing"}));

        let mut logged_out = pi(Found::LoggedOut);
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
        let joining = Held { status: Status::Joining, pr: Some(30), ..gone.clone() };
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
        state.held = vec![
            Held { status: Status::Joined, ..gone.clone() },
            Held { status: Status::Closed, ..gone },
        ];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
        // …and one orca could not speak of is not said to have none
        state.held = vec![Held { worker: None, ..held(3) }];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
    }

    #[test]
    fn a_worker_stopped_for_long_with_its_claim_at_work_names_the_nudge() {
        let mut state = state();
        let stopped = |quiet| Held { worker: Some(WorkerState::Idle { quiet }), ..held(3) };
        state.held = vec![stopped(Some(STOPPED_LONG))];
        let found = found(&host(), &run(vec![]), &state);
        assert_eq!(
            fixes(&found, "has been stopped for 30m"),
            ["afkconvoy nudge --issue 3 --instance alpha"]
        );
        assert!(found[0].evidence.contains("/wt/issue-3"), "{found:?}");
        // a short stop, or one of a runtime that keeps no time, is none
        state.held = vec![stopped(Some(STOPPED_LONG - 1)), stopped(None)];
        assert_eq!(self::found(&host(), &run(vec![]), &state), []);
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

        // twice, or not in a row, or on another issue each time: none
        let twice = vec![tick(5, both()), tick(6, both())];
        assert_eq!(self::found(&host(), &run(twice), &state()), []);
        let broken = vec![tick(4, both()), tick(5, json!([])), tick(6, both())];
        assert_eq!(self::found(&host(), &run(broken), &state()), []);
        let other = |issue| json!([failed(issue, "no")]);
        let moving = vec![tick(4, other(3)), tick(5, other(5)), tick(6, other(7))];
        assert_eq!(self::found(&host(), &run(moving), &state()), []);
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
            found[0].evidence.contains("the longest between two before it is 16m"),
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
    fn state_that_cannot_be_read_is_critical_and_findings_come_most_severe_first() {
        let why = "gh issue list failed: HTTP 401";
        let found = findings(&Seen {
            now: NOW,
            host: &host(),
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
    fn the_events_that_reached_this_hosts_runs_are_listed_newest_first_with_their_causes() {
        let entry = |at: u64, instance: &str, host: &str, what: What| Entry {
            at,
            instance: instance.into(),
            host: host.into(),
            what,
        };
        let start = What::Start { worker_command: "claude".into(), harness_commands: [].into() };
        let started = |count, cause| What::TickStarted { count, cause, by: None };
        let entries = [
            entry(10, "old", "h1", start.clone()),
            entry(20, "old", "h1", What::Stop { line: "holding no claim".into() }),
            entry(30, "alpha", "h1", start.clone()),
            entry(30, "alpha", "h1", started(1, Cause::Start)),
            entry(40, "alpha", "h1", What::Tick(tick(1, json!([])).1)),
            entry(50, "alpha", "h1", started(2, Cause::Timer)),
            entry(55, "alpha", "h1", What::WokenDuring { during: 2, by: None }),
            entry(60, "alpha", "h1", started(3, Cause::Woken)),
            entry(70, "alpha", "h1", What::HostSleep { slept: false, awake: vec!["x".into()] }),
            entry(80, "alpha", "h1", What::HostSleep { slept: true, awake: vec![] }),
            entry(90, "alpha", "h1", What::HostWoke { asleep: 3600 }),
            entry(95, "beta", "h2", start),
        ];
        let events = read_events(&entries, "h1");
        let said: Vec<(u64, &str, &str, &str)> = events
            .iter()
            .map(|event| (event.at, event.event, event.cause.as_str(), event.instance.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (90, "host_wake", "after 3600s asleep", "alpha"),
                (80, "host_sleep", "every fleet on this host was idle", "alpha"),
                (60, "wake", "woken: tick 3 started", "alpha"),
                (55, "wake", "woken during tick 2: tick 3 followed", "alpha"),
                (30, "start", "afkconvoy run started, its workers with `claude`", "alpha"),
                (20, "stop", "told to stop (SIGTERM or SIGINT): holding no claim", "old"),
                (10, "start", "afkconvoy run started, its workers with `claude`", "old"),
            ]
        );
        // only the newest are listed
        let many: Vec<Entry> =
            (0..30).map(|at| entry(at, "alpha", "h1", started(at, Cause::Woken))).collect();
        let events = read_events(&many, "h1");
        assert_eq!(events.len(), EVENTS_SHOWN);
        assert_eq!(events[0].at, 29);
    }

    #[test]
    fn the_hot_workers_are_those_the_newest_run_found_hot_and_not_cooled_since() {
        let entry = |at: u64, instance: &str, what: What| Entry {
            at,
            instance: instance.into(),
            host: "h1".into(),
            what,
        };
        let hot = |worker: &str| What::Hot {
            worker: worker.into(),
            why: "past 4.0 cores for 120s".into(),
            held: "6.0 cores, 1.0 GiB resident".into(),
        };
        let cooled = |worker: &str| What::Cooled {
            worker: worker.into(),
            since: 0,
            held: "6.0 cores, 1.0 GiB resident".into(),
            gone: false,
        };
        let start = What::Start { worker_command: "claude".into(), harness_commands: [].into() };
        let entries = [
            entry(10, "old", start.clone()),
            entry(20, "old", hot("#1")),
            entry(30, "alpha", start),
            entry(40, "alpha", hot("#7")),
            entry(50, "alpha", hot("#8")),
            entry(60, "alpha", cooled("#7")),
            entry(70, "alpha", hot("#9")),
        ];
        let read = read_hot(&entries, "h1");
        let said: Vec<(&str, u64)> =
            read.iter().map(|hot| (hot.worker.as_str(), hot.since)).collect();
        // #1 was the old run's, and #7 cooled
        assert_eq!(said, [("#8", 50), ("#9", 70)]);
        assert!(read_hot(&entries, "h2").is_empty());
    }

    #[test]
    fn the_last_check_for_host_sleep_is_read_for_the_instance_this_host_ran_last() {
        let entry = |at: u64, instance: &str, slept: bool, awake: &[&str]| Entry {
            at,
            instance: instance.into(),
            host: "h1".into(),
            what: What::HostSleep { slept, awake: awake.iter().map(|a| a.to_string()).collect() },
        };
        let ticked =
            Entry { what: What::Tick(tick(1, json!([])).1), ..entry(40, "alpha", false, &[]) };
        let entries = [
            entry(10, "old", true, &[]),
            entry(20, "alpha", false, &["beta is not idle"]),
            entry(30, "alpha", false, &["the keyboard or mouse was used 30s ago"]),
            ticked,
        ];
        let checked = read_host_sleep(&entries, "h1").unwrap();
        assert_eq!(checked.at, 30);
        assert_eq!(checked.awake, ["the keyboard or mouse was used 30s ago"]);
        assert_eq!(read_host_sleep(&entries[..1], "h1").map(|checked| checked.slept), Some(true));
        assert_eq!(read_host_sleep(&entries, "h2"), None);
    }

    #[test]
    fn the_log_is_read_for_the_instance_this_host_ran_last() {
        let entry = |at: u64, instance: &str, host: &str, what: What| Entry {
            at,
            instance: instance.into(),
            host: host.into(),
            what,
        };
        let start = |command: &str| What::Start {
            worker_command: command.into(),
            harness_commands: [("pi".to_string(), "pi".to_string())].into(),
        };
        let ticked = |count| What::Tick(tick(count, json!([])).1);
        let entries = [
            entry(10, "old", "h1", start("agent")),
            entry(20, "old", "h1", ticked(1)),
            entry(30, "alpha", "h1", start("claude --yolo")),
            entry(40, "alpha", "h1", ticked(1)),
            entry(50, "alpha", "h1", ticked(2)),
        ];
        let (instance, command, ticks) = read_log(&entries, "h1");
        assert_eq!(instance.as_deref(), Some("alpha"));
        // as the legacy start record represents them: the default harness's bare
        assert_eq!(command.unwrap(), ["claude --yolo", "pi=pi"]);
        let counts: Vec<(u64, u64)> = ticks.iter().map(|(at, tick)| (*at, tick.count)).collect();
        assert_eq!(counts, [(40, 1), (50, 2)], "its own ticks only");
        assert_eq!(read_log(&entries, "h2"), (None, None, vec![]));
    }
}

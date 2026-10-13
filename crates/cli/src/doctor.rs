//! `afkconvoy doctor`: the state of the fleet on this host, and what is wrong
//! with it — each finding with the line to run, or the file and key to edit.
//!
//! `read` is the half that asks: this host, the log of its `run`, GitHub and
//! the remote, orca after each worker. It writes nothing anywhere but the
//! fleet's own cache, and makes no startup probe: it is run beside a live
//! fleet, often. `findings` is the half that judges what was read, and asks
//! nothing. Nothing here fixes anything: the fleet's own commands do.

use std::path::PathBuf;

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::config::Config;
use afkconvoy_core::gate::GateCommand;
use afkconvoy_core::lease::{LEASE, stamp_age, stamp_stale};
use afkconvoy_core::patrol::Standing;
use afkconvoy_core::plan::{LabelBar, Status, WorkingSet};
use afkconvoy_core::team::Fault;
use afkconvoy_core::tools::{Found, START_ORCA, Tool, add_checkout, harness_missing};
use afkconvoy_core::waiting::{WAITED_LONG, Waiting};
use afkconvoy_core::worker::WorkerState;
use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};

use crate::config;
use crate::fleet::Fleet;
use crate::gate::{head_tree, recorded};
use crate::git::Repo;
use crate::launch::Launches;
use crate::log::{Entry, Failed, Log, Tick, What, by_cause};
use crate::orca::{Source, Under, Worktree};
use crate::panel::lasted;
use crate::role::{self, Checked};
use crate::stage::Stood;
use crate::team::{self, Refused};
use crate::tools;
use crate::topic::See;
use crate::wake::listening;

/// How many ticks in a row a step fails in before it is a finding.
const FAILED_TICKS: usize = 3;
/// How many failed steps the answer lists, newest first.
const FAILED_SHOWN: usize = 10;
/// How long a worker is stopped for before it is a finding: past every wait
/// a tick gives one before it nudges.
const STOPPED_LONG: u64 = 1800;
/// How long an issue is at a stage before its stay is held against the
/// stages before it.
const STAGE_LONG: u64 = 900;

/// What the answer says of a host whose `run` left no log.
const NO_LOG: &str = "no log";

/// How much a finding matters, most first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The fleet does not work, or will not, until it is fixed.
    Critical,
    /// One issue or one patrol is held up, or is about to be.
    Warning,
    /// Nothing to fix: said so that a surprise is explained.
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
    StepFailed: Critical, "failed in each of the last";
    TeamUnread: Critical, "the team this repo names cannot be read";
    TeamFaulty: Critical, "has a fault at its head";
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
    Moved: Info, "moved since the stage";
}

fn finding(kind: Kind, what: String, evidence: String, fix: String) -> Finding {
    debug_assert!(what.contains(kind.symptom()), "{kind:?} is not said in: {what}");
    Finding { severity: kind.severity(), what, evidence, fix, see: kind.see() }
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
    /// Whether the `run` on this host said so in its log; else it is the one
    /// a `run` started now with no `--worker-command` for it would use.
    pub of_run: bool,
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
    /// Whether one listens for wakes: it is alive.
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
}

/// Everything `doctor` read.
pub struct Seen<'a> {
    pub now: u64,
    pub host: &'a Host,
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
                "afkconvoy run --help".to_string(),
            ));
        }
    }
    found
}

/// No `run` is alive, and claims are held.
fn run_findings(run: &Run, held: &[Held]) -> Vec<Finding> {
    let Some(instance) = run.instance.as_ref().filter(|_| !run.alive && !held.is_empty()) else {
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
        format!("nothing listens for wakes in this repo's git dir; {beat}"),
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

/// The team, or a role of it, has a fault at its head.
fn team_findings(team: &TeamRead) -> Vec<Finding> {
    match team {
        TeamRead::Unbound | TeamRead::Loaded { .. } => vec![],
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

/// Everything that is wrong in what `seen` holds, most severe first.
pub fn findings(seen: &Seen) -> Vec<Finding> {
    let mut found = host_findings(seen.host);
    found.extend(log_findings(seen.run));
    match seen.state {
        Ok(state) => {
            found.extend(run_findings(seen.run, &state.held));
            found.extend(team_findings(&state.team));
            found.extend(plan_findings(seen.run, state));
            found.extend(claim_findings(seen.run, &state.held));
            found.extend(issue_findings(&state.stood, &state.team, seen.now));
            found.extend(waiting_findings(&state.set, seen.now));
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

/// What this host has of what the fleet needs. `started_with` is what the
/// `run` here was given as its worker launch commands, as its log has them
/// (`read_log`); with none to say, each is the one a `run` started now would
/// use. No human is asked.
fn read_host(config: Option<&Config>, started_with: Option<&[String]>) -> Host {
    let said = started_with.and_then(|given| Launches::new(given, false).ok());
    let of_run = said.is_some();
    let launches = said.unwrap_or_else(Launches::foreseen);
    let default = launches.default();
    let looked = tools::look(launches.of(default).ok().flatten());
    // An unbound repo knows of no harness: nothing calls for its one command's.
    let called = match config.and_then(|config| config.bound.as_ref()) {
        Some(bound) => role::called_for(bound),
        None => vec![(default, vec![])],
    };
    let checked = role::check(&launches, &called).unwrap_or_default();
    let launch = |checked: Checked| Launch {
        // A `run` says its default harness's command, and each other it was given.
        of_run: of_run && (checked.harness == default || launches.says(checked.harness)),
        checked,
    };
    Host {
        tools: looked.found,
        gh_host: looked.host,
        orca_unasked: looked.orca_unasked,
        launches: checked.into_iter().map(launch).collect(),
    }
}

/// The worker launch commands a `run` said it started with: the first word
/// of each, as `--worker-command` would be given them.
type StartedWith = Vec<String>;

/// What the log says of the `run` of `host`: its fleet instance, the worker
/// launch commands it started with, and that instance's ticks.
fn read_log(
    entries: &[Entry],
    host: &str,
) -> (Option<String>, Option<StartedWith>, Vec<(u64, Tick)>) {
    let Some(newest) = entries.iter().rev().find(|entry| entry.host == host) else {
        return (None, None, vec![]);
    };
    let own = || entries.iter().filter(|entry| entry.instance == newest.instance);
    let command = own().rev().find_map(|entry| match &entry.what {
        What::Start { worker_command, harness_commands } => {
            let named = harness_commands.iter().map(|(harness, word)| format!("{harness}={word}"));
            Some(std::iter::once(worker_command.clone()).chain(named).collect())
        }
        What::Tick(_) => None,
    });
    let ticks = own().filter_map(|entry| match &entry.what {
        What::Tick(tick) => Some((entry.at, tick.clone())),
        What::Start { .. } => None,
    });
    (Some(newest.instance.clone()), command, ticks.collect())
}

/// The team a bound repo names, as it stands at its head → with the team
/// itself, where it loaded: what a stage in flight is held against.
fn read_team(bound: bool) -> (TeamRead, Option<team::Loaded>) {
    if !bound {
        return (TeamRead::Unbound, None);
    }
    match team::load(team::From::Config) {
        Ok(loaded) => {
            let workflows = loaded.team.workflows.iter();
            let flow_labels = workflows.map(|workflow| workflow.flow_label()).collect();
            (TeamRead::Loaded { flow_labels }, Some(loaded))
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

fn read_state(now: u64, run: &mut Run) -> Result<State> {
    let (fleet, open) = Fleet::read(now)?;
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
    let (team, loaded) = read_team(fleet.config.bound.is_some());
    let held = read_held(&fleet, &set, &stood, loaded.as_ref());
    Ok(State { set, held, stood, team, ready_label: fleet.config.ready_label.clone() })
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
        "worker_commands": host.launches.iter().map(|Launch { checked, of_run }| json!({
            // An unbound repo knows of no harness: nothing calls for one.
            "harness": (!checked.callers.is_empty()).then_some(checked.harness),
            "called_for_by": checked.callers,
            "command": checked.command,
            "from": if *of_run { "run" } else { "default" },
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

/// `afkconvoy doctor`, for the fleet of `host` in the checkout it is run in:
///
///   {"host", "fleet", "claims", "issues", "waiting_on_human": [{"issue", "title",
///    "why", "since", "answer"}], "findings": [{"severity", "what", "evidence",
///    "fix"}]}
///
/// An answer with a `critical` finding is an `Unwell` error: the same object,
/// and a non-zero exit. It reads only — but for the fleet's own cache, where
/// reading a team keeps its copies.
pub fn doctor(now: u64, host: &str) -> Result<Value> {
    let repo = Repo::here()?;
    let log = Log::of(&repo).ok();
    let entries = log.as_ref().and_then(Log::read);
    let (instance, started_with, ticks) = read_log(entries.as_deref().unwrap_or_default(), host);
    let mut run = Run {
        alive: listening(&repo),
        instance,
        host: host.to_string(),
        heartbeat_age: None,
        heartbeats: Heartbeats::default(),
        log_file: log.map(|log| log.file),
        ticks: entries.map(|_| ticks),
    };
    let config = config::load(&repo.dir.join(config::FILE));
    let seen_host = read_host(config.as_ref().ok(), started_with.as_deref());
    let state = read_state(now, &mut run).map_err(|error| format!("{error:#}"));
    let seen =
        Seen { now, host: &seen_host, run: &run, state: state.as_ref().map_err(String::as_str) };
    let found = findings(&seen);
    let (claims, issues, waiting) = match &state {
        Ok(state) => (
            state.held.iter().map(held_json).collect(),
            issues_json(&state.set)?,
            serde_json::to_value(&state.set.waiting_on_human)?,
        ),
        Err(_) => (vec![], Value::Null, Value::Null),
    };
    let answer = json!({
        "host": host_json(&seen_host),
        "fleet": fleet_json(&run),
        "claims": Value::Array(claims),
        "issues": issues,
        "waiting_on_human": waiting,
        "findings": found,
    });
    if found.iter().any(|finding| finding.severity == Severity::Critical) {
        return Err(Unwell(answer).into());
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use afkconvoy_core::patrol::Row;
    use afkconvoy_core::plan::{Dispatchable, Excluded, Mine, PeerClaim};

    use super::*;

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
                of_run: true,
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
            free_slots: 2,
            now: NOW,
            locks: vec![],
            flowing: vec![],
            patrols: vec![],
            proposals: vec![],
        }
    }

    fn state() -> State {
        State {
            set: set(),
            held: vec![],
            stood: vec![],
            team: TeamRead::Unbound,
            ready_label: "ready-for-agent".into(),
        }
    }

    fn team() -> TeamRead {
        TeamRead::Loaded { flow_labels: vec!["flow:feature".into()] }
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
            failure: None,
            claim: None,
        }
    }

    fn found(host: &Host, run: &Run, state: &State) -> Vec<Finding> {
        let found = findings(&Seen { now: NOW, host, run, state: Ok(state) });
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
        assert_eq!(fixes(&found, "cannot start a worker in its role"), ["afkconvoy run --help"]);
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
                of_run: false,
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
        let waiting =
            |number| Dispatchable { number, title: format!("issue {number}"), stage: None };
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
        state.set.excluded = vec![excluded(9, "1 open blocker(s)")];
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
        state.set.mine =
            vec![Mine { number: 9, title: None, status: Status::NoPr, pr: None, due: None }];
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
    fn state_that_cannot_be_read_is_critical_and_findings_come_most_severe_first() {
        let why = "gh issue list failed: HTTP 401";
        let found = findings(&Seen { now: NOW, host: &host(), run: &run(vec![]), state: Err(why) });
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
        // as `--worker-command` would be given them: the default harness's bare
        assert_eq!(command.unwrap(), ["claude --yolo", "pi=pi"]);
        let counts: Vec<(u64, u64)> = ticks.iter().map(|(at, tick)| (*at, tick.count)).collect();
        assert_eq!(counts, [(40, 1), (50, 2)], "its own ticks only");
        assert_eq!(read_log(&entries, "h2"), (None, None, vec![]));
    }
}

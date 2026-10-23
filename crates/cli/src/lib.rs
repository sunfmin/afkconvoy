//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::config::{RepoDir, RepoName};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::host_sleep::IDLE_WAIT;
use afkconvoy_core::inspection::{Found, INSPECTION_ENV};
use afkconvoy_core::line::Line;
use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::position::Declared;
use afkconvoy_core::role::{
    CONSULT_ENV, HARNESS_ENV, ISSUE_ENV, MODEL_ENV, ROLE_COMMIT_ENV, ROLE_ENV, STAGE_ENV,
    TEAM_COMMIT_ENV,
};

use anyhow::Context;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::{Value, json};

pub mod abandon;
pub mod acting;
pub mod admission;
pub mod advance;
pub mod board;
pub mod caster;
pub mod claims;
pub mod config;
pub mod copies;
pub mod dispatch;
pub mod doctor;
pub mod evidence;
pub mod exclusive;
pub mod fleet;
pub mod flows;
pub mod forget;
pub mod gate;
pub mod gh;
pub mod git;
pub mod guard;
pub mod harness_down;
pub mod host_evidence;
pub mod host_sleep;
pub mod hot;
pub mod inspection;
pub mod intake;
pub mod judge;
pub mod kept;
mod kit;
pub mod landing;
pub mod launch;
pub mod lock;
pub mod log;
pub mod measure;
pub mod opening;
pub mod orca;
pub mod panel;
pub mod park;
pub mod patrol;
pub mod paused;
pub mod precedent;
pub mod probe;
pub mod reclaim;
pub mod recovery;
pub mod role;
pub mod scaffold;
pub mod serve;
pub mod settle;
pub mod slow_gate;
pub mod stage;
pub mod steer;
pub mod stop;
pub mod switch;
pub mod takeover;
pub mod team;
pub mod tend;
pub mod tick;
pub mod tools;
pub mod topic;
pub mod train;
pub mod wake;
pub mod watch;
pub mod workflow;

use acting::Acting;
use claims::{Me, Scan, Take};
use dispatch::Agent;
use fleet::Fleet;
use git::Repo;
use launch::Launches;
use tick::Pacing;

/// The environment variables that tell `run` `--hot-cores`, `--hot-memory` and
/// `--hot-written`.
const HOT_CORES_ENV: &str = "AFKCONVOY_HOT_CORES";
const HOT_MEMORY_ENV: &str = "AFKCONVOY_HOT_MEMORY";
const HOT_WRITTEN_ENV: &str = "AFKCONVOY_HOT_WRITTEN";

/// A number of cores or GiB, whole or not, as thousandths of it.
fn thousandths(said: &str) -> Result<u64, String> {
    match said.parse::<f64>() {
        Ok(number) if number.is_finite() && number > 0.0 => Ok((number * 1000.0).round() as u64),
        _ => Err(format!("`{said}` is not a number above zero")),
    }
}

/// The environment variable that tells `run` `--no-host-sleep`.
pub const NO_HOST_SLEEP_ENV: &str = "AFKCONVOY_NO_HOST_SLEEP";

/// What this binary was built from, as `--version` says it: stamped by
/// `build.rs`.
pub const VERSION: &str = env!("AFKCONVOY_VERSION");

/// The skill file `skill` prints, and the one `npx skills add` installs: a
/// user's own agent learns from it that afkconvoy is there.
const SKILL: &str = include_str!("../../../skills/afkconvoy/SKILL.md");

/// `-h` is the one-screen listing; `--help` is `help.txt`, the text an agent
/// reads to learn the tool. Each subcommand keeps its one line for `-h`, and
/// its `--help` adds `help/<name>.txt`: when to run it, and what it answers.
/// `help` is this crate's own, not clap's: it prints a topic too. The
/// listing goes straight from the usage line to the commands: one line less,
/// so that `-h` stays one screen.
#[derive(Parser)]
#[command(name = "afkconvoy", version = VERSION, about, long_about = guide(), disable_help_subcommand = true,
    help_template = "{before-help}{about-with-newline}\n{usage-heading} {usage}\n{all-args}{after-help}")]
struct Cli {
    /// The time to act at, in epoch seconds [default: the clock's]
    #[arg(long, global = true, hide = true)]
    now: Option<u64>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect and edit this host's worker launch commands
    #[command(after_long_help = include_str!("help/kit.txt"))]
    Kit,
    /// Cast every uncast issue that has a choice to make, in one session of the host kit's first
    #[command(after_long_help = include_str!("help/cast.txt"))]
    Cast,
    /// Print the repo's config as JSON, defaults filled
    #[command(after_long_help = include_str!("help/config.txt"))]
    Config {
        /// The config file [default: docs/agents/afkconvoy.md of this checkout]
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Read a team and its workflows: what was read, or every fault
    #[command(after_long_help = include_str!("help/check.txt"))]
    Check {
        #[command(flatten)]
        team: TeamArgs,
    },
    /// Write the skeleton of a team with its roles, or of one role, or a whole example team,
    /// and create its repo on GitHub if asked
    #[command(after_long_help = include_str!("help/scaffold.txt"))]
    Scaffold {
        /// What to write the skeleton of
        #[arg(value_enum)]
        what: scaffold::What,
        /// The directory to write it in: one not there yet, or empty
        dir: PathBuf,
        /// A role of the team, once for each: <name> for one written here, under roles/<name>,
        /// or <name>=<owner/repo[/path]> for one that is elsewhere. The example workflow is
        /// performed by the first
        #[arg(long, value_parser = scaffold::named)]
        role: Vec<scaffold::Named>,
        /// Of a team: write this example whole — a team the binary holds, with its roles —
        /// in place of a skeleton. It takes no --role. `afkconvoy help example` reads the
        /// team with you
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(scaffold::example_names()))]
        example: Option<String>,
        /// Also commit the skeleton and create this repo on GitHub from it, as owner/repo
        #[arg(long, value_parser = repo)]
        repo: Option<RepoName>,
        /// Create that repo public [default: private]
        #[arg(long, requires = "repo")]
        public: bool,
    },
    /// Say how to open an issue the fleet will work, and create the labels that takes
    #[command(after_long_help = include_str!("help/flows.txt"))]
    Flows,
    /// Print the skill file that sends an agent of your own to `--help`: text, not JSON
    #[command(after_long_help = include_str!("help/skill.txt"))]
    Skill,
    /// Draw a team's workflows as graphs, or say where one issue is in its workflow
    #[command(after_long_help = include_str!("help/show.txt"))]
    Show {
        /// The workflow to draw [default: every workflow of the team] — or an issue's
        /// number, which is `--issue`
        #[arg(value_name = "WORKFLOW|ISSUE")]
        workflow: Option<String>,
        /// Say instead where this issue is, from its stage records: its position, what
        /// follows, and the rework it has left
        #[arg(long, conflicts_with = "workflow")]
        issue: Option<u64>,
        #[command(flatten)]
        team: TeamArgs,
    },
    /// Say where an issue would be after each of a list of verdicts, starting no worker
    #[command(after_long_help = include_str!("help/rehearse.txt"))]
    Rehearse {
        /// The workflow the issue follows
        workflow: String,
        /// What is declared on it, in order: pass, reject, blocked, already-satisfied,
        /// escalate, or ruling
        #[arg(value_parser = declared)]
        declared: Vec<Declared>,
        #[command(flatten)]
        team: TeamArgs,
    },
    /// Say what the fleet on this host is doing and what is wrong with it, each finding with its fix
    #[command(after_long_help = include_str!("help/doctor.txt"))]
    Doctor {
        #[command(flatten)]
        proxy: ProxyArgs,
        /// The host whose fleet is asked after [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
    },
    /// Show what a tick would act on, and change nothing
    #[command(after_long_help = include_str!("help/plan.txt"))]
    Plan {
        #[command(flatten)]
        proxy: ProxyArgs,
        /// The fleet instance whose claims are its own [default: nobody's]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
    },
    /// Run the fleet: tick after tick, until stopped
    #[command(after_long_help = include_str!("help/run.txt"))]
    Run {
        /// The fleet instance to run as [default: a new one, named for this host]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        agent: AgentArgs,
        /// Serve the panel — the fleet as a web page, live — on this port of this host
        #[arg(long, value_name = "PORT")]
        panel: Option<u16>,
        /// Keep this host awake: put it to sleep never, and keep every other fleet on it
        /// from doing so [default: put it to sleep once every fleet on it has been idle
        /// ten minutes]
        #[arg(long, env = NO_HOST_SLEEP_ENV)]
        no_host_sleep: bool,
        /// Seconds every fleet on this host must have been idle before it is put to sleep
        #[arg(long, hide = true, default_value_t = IDLE_WAIT)]
        host_sleep_after: u64,
        /// The cores a worker's processes may take before it turns hot, held two minutes
        /// [default: half this host's cores, one at the least]
        #[arg(long, env = HOT_CORES_ENV, value_name = "CORES", value_parser = thousandths)]
        hot_cores: Option<u64>,
        /// The GiB of memory a worker's processes may hold resident before it turns hot
        /// [default: a quarter of this host's memory]
        #[arg(long, env = HOT_MEMORY_ENV, value_name = "GIB", value_parser = thousandths)]
        hot_memory: Option<u64>,
        /// The GiB a worker's processes may write to disk in ten minutes before it turns hot
        /// [default: a tenth of the disk free here, one GiB at the least]
        #[arg(long, env = HOT_WRITTEN_ENV, value_name = "GIB", value_parser = thousandths)]
        hot_written: Option<u64>,
        /// Stop after this many ticks
        #[arg(long, hide = true)]
        ticks: Option<u64>,
        /// Milliseconds one second of sleep takes
        #[arg(long, hide = true, default_value_t = 1000)]
        second_ms: u64,
    },
    /// Print the panel — the fleet as a web page — as one rebuild shows it now, or open a `run`'s live one
    #[command(after_long_help = include_str!("help/panel.txt"))]
    Panel {
        /// The fleet instance whose claims are its own [default: nobody's]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        /// Have this host's `run` for this repo serve its panel, live, and say where:
        /// it is not restarted, and one already serving it says the address it has
        #[arg(long, conflicts_with_all = ["instance", "host"])]
        open: bool,
        /// With --open: the port of this host to serve it on [default: any free one]
        #[arg(long, value_name = "PORT", requires = "open")]
        port: Option<u16>,
    },
    /// Run one reconciliation pass and exit
    #[command(after_long_help = include_str!("help/tick.txt"))]
    Tick {
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Run the gate in this worktree and put a green run on record (a worker's command)
    #[command(after_long_help = include_str!("help/gate.txt"))]
    Gate {
        /// First merge in what the PR lands behind: the landing train, or the base branch
        #[arg(long)]
        train: bool,
        /// The host a green run is recorded as made on [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
    },
    /// Put this worktree's PR on the landing train (a worker's command), or land the train
    #[command(after_long_help = include_str!("help/land.txt"))]
    Land {
        /// The issue the PR closes
        #[arg(value_name = "ISSUE", conflicts_with_all = ["train", "issue"])]
        number: Option<u64>,
        /// The same, as a flag
        #[arg(long, required_unless_present_any = ["train", "number"], conflicts_with = "train")]
        issue: Option<u64>,
        /// Gate the landing train and land it on the base branch (the train worker's command)
        #[arg(long)]
        train: bool,
        /// Seconds GitHub is given to show a landed PR as merged
        #[arg(long, default_value_t = 60)]
        merged_timeout: u64,
        /// The host a green run made by `--train` is recorded as made on [default: this
        /// machine's name]
        #[arg(long)]
        host: Option<String>,
    },
    /// Declare the verdict that ends a stage, with the text it produced (a worker's
    /// command, and a human's for a stage nobody holds)
    #[command(after_long_help = include_str!("help/verdict.txt"))]
    Verdict {
        /// What the stage declares: pass, reject, blocked, already-satisfied, or escalate
        #[arg(value_parser = verdict)]
        verdict: Declared,
        #[command(flatten)]
        issue: IssueArg,
        /// The stage that declares it: the one its worker was started on
        #[arg(long)]
        stage: String,
        #[command(flatten)]
        text: TextArgs,
        /// The issues a `blocked` verdict waits on, as numbers separated by commas
        #[arg(long, value_delimiter = ',')]
        blocked_by: Vec<u64>,
        /// The commit of the role's repo a worker's stage ran under [a human names none]
        #[arg(long, env = ROLE_COMMIT_ENV, requires = "team_commit")]
        role_commit: Option<String>,
        /// The commit of the team's repo a worker's stage ran under [a human names none]
        #[arg(long, env = TEAM_COMMIT_ENV, requires = "role_commit")]
        team_commit: Option<String>,
        /// The harness a worker's stage ran on [a human names none]
        #[arg(long, env = HARNESS_ENV, value_parser = harness, requires = "role_commit")]
        harness: Option<Harness>,
        /// The model a worker ran, where it was started on one [a human names none]
        #[arg(long, env = MODEL_ENV, requires = "role_commit")]
        model: Option<String>,
        /// The worker is the stage's consult, letting its `escalate` through [a human is none]
        #[arg(long, env = CONSULT_ENV)]
        consult: bool,
    },
    /// Answer an escalated issue with a ruling: its stage runs again under it (a human's
    /// command, and a consult's for the stage that asked it)
    #[command(after_long_help = include_str!("help/rule.txt"))]
    Rule {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        text: TextArgs,
        /// The stage whose hand-off a consult rules on: the one its environment
        /// names [a human names none]
        #[arg(long, env = STAGE_ENV, requires = "role_commit")]
        stage: Option<String>,
        /// The commit of the role's repo a consult ran under [a human names none]
        #[arg(long, env = ROLE_COMMIT_ENV, requires = "team_commit")]
        role_commit: Option<String>,
        /// The commit of the team's repo a consult ran under [a human names none]
        #[arg(long, env = TEAM_COMMIT_ENV, requires = "role_commit")]
        team_commit: Option<String>,
        /// The harness a consult ran on [a human names none]
        #[arg(long, env = HARNESS_ENV, value_parser = harness, requires = "role_commit")]
        harness: Option<Harness>,
        /// The model a worker ran, where it was started on one [a human names none]
        #[arg(long, env = MODEL_ENV, requires = "role_commit")]
        model: Option<String>,
    },
    /// Send an issue to a named stage of its workflow (a human's command, and a consult's)
    #[command(after_long_help = include_str!("help/send.txt"))]
    Send {
        #[command(flatten)]
        issue: IssueArg,
        /// The stage to put it at
        #[arg(long)]
        stage: String,
        #[command(flatten)]
        text: TextArgs,
        /// The commit of the role's repo a consult ran under [a human names none]
        #[arg(long, env = ROLE_COMMIT_ENV, requires = "team_commit")]
        role_commit: Option<String>,
        /// The commit of the team's repo a consult ran under [a human names none]
        #[arg(long, env = TEAM_COMMIT_ENV, requires = "role_commit")]
        team_commit: Option<String>,
        /// The harness a consult ran on [a human names none]
        #[arg(long, env = HARNESS_ENV, value_parser = harness, requires = "role_commit")]
        harness: Option<Harness>,
        /// The model a worker ran, where it was started on one [a human names none]
        #[arg(long, env = MODEL_ENV, requires = "role_commit")]
        model: Option<String>,
        /// The worker is the stage's consult [a human is none]
        #[arg(long, env = CONSULT_ENV)]
        consult: bool,
    },
    /// Take an issue in to a workflow: its flow label, on the record (a worker's command)
    #[command(after_long_help = include_str!("help/intake.txt"))]
    Intake {
        #[command(flatten)]
        issue: IssueArg,
        /// The workflow it is to follow, by name: what follows `flow:` in its flow label
        #[arg(long)]
        workflow: String,
        /// The role of the worker that takes it in
        #[arg(long, env = ROLE_ENV)]
        role: Option<String>,
        /// The issue that worker is on
        #[arg(long, env = ISSUE_ENV, conflicts_with = "patrol")]
        from: Option<u64>,
        /// The patrol that worker runs, when it is on no issue
        #[arg(long, env = PATROL_ENV)]
        patrol: Option<String>,
        /// Taken in by the train worker, for a defect of the base branch: no role, issue or patrol
        #[arg(long)]
        train: bool,
        /// The issues it is to wait for, as numbers separated by commas
        #[arg(long, value_delimiter = ',')]
        blocked_by: Vec<u64>,
    },
    /// Raise a fleet lock on an issue: the fleet works lock issues only until it closes (a worker's command)
    #[command(after_long_help = include_str!("help/lock.txt"))]
    Lock {
        #[command(flatten)]
        issue: IssueArg,
        /// The role that raises it [default: the one this worker was started in, if any]
        #[arg(long, env = ROLE_ENV)]
        role: Option<String>,
    },
    /// Have a patrol run now, past its interval and its daily bound: the next tick runs it
    #[command(after_long_help = include_str!("help/patrol-now.txt"))]
    PatrolNow {
        /// The patrol, as the repo's config names it under `patrols`
        name: String,
    },
    /// Declare a patrol's run or a proposal's over: the next tick ends it (a worker's command)
    #[command(after_long_help = include_str!("help/over.txt"))]
    Over {
        /// The run, as its brief names it
        #[arg(long)]
        run: String,
    },
    /// Say what an inspection of a hot worker or of the host found: its `run` types it at
    /// the worker and leaves it on the issue, or keeps it beside admission's reading (an
    /// inspection's command)
    #[command(after_long_help = include_str!("help/inspected.txt"))]
    Inspected {
        /// What it found: of a hot worker wrong, heavy or not-its; of the host reclaimable,
        /// fleet or not-its
        #[arg(long, value_parser = found)]
        found: Found,
        #[command(flatten)]
        text: TextArgs,
        /// Of the host, with reclaimable: a path under the home folder to free — what git
        /// ignores in a checkout no process uses, a closed issue's worktree, a tool's cache
        #[arg(long, value_name = "PATH")]
        reclaim: Vec<PathBuf>,
        /// Of the host: a path to name for a human, which is never deleted
        #[arg(long, value_name = "PATH")]
        name: Vec<PathBuf>,
    },
    /// Run a heavy command alone on this host, once no other `exclusive` command runs (a
    /// worker's command)
    #[command(after_long_help = include_str!("help/exclusive.txt"))]
    Exclusive {
        /// The command and its arguments, after `--`
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Allow or refuse the tool call described on stdin (a worker's hook, added at launch)
    #[command(after_long_help = include_str!("help/guard.txt"))]
    Guard,
    /// Start a fresh worker on one issue
    #[command(after_long_help = include_str!("help/dispatch.txt"))]
    Dispatch {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
        /// Where to start from: what survives, or — discarding it — the base branch
        #[arg(long, value_enum, default_value = "auto")]
        start: dispatch::StartFrom,
    },
    /// Show what `dispatch` would continue an issue from, without acting
    #[command(after_long_help = include_str!("help/recovery.txt"))]
    Recovery {
        #[command(flatten)]
        issue: IssueArg,
    },
    /// Close one of my claims whose issue needed no change
    #[command(after_long_help = include_str!("help/close.txt"))]
    Close {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
    },
    /// Hand one of my claims to a human
    #[command(after_long_help = include_str!("help/escalate.txt"))]
    Escalate {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        /// Why: what a human needs to act on it in one reply
        #[arg(long)]
        reason: String,
    },
    /// Leave one of my claims waiting on the issues its worker declared it blocked by
    #[command(after_long_help = include_str!("help/park.txt"))]
    Park {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
    },
    /// Tell the worker of one of my claims, stopped with no PR and no verdict, to carry on
    #[command(after_long_help = include_str!("help/nudge.txt"))]
    Nudge {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
    },
    /// Replace the silent worker of one of my claims whose PR is joining the train, once
    #[command(after_long_help = include_str!("help/restart.txt"))]
    Restart {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Move this host's workers of the repo onto another harness: from now on, and those of my claims now
    #[command(after_long_help = include_str!("help/switch.txt"))]
    Switch {
        /// The harness to move onto: claude-code, pi, codex or dsh
        #[arg(long, value_parser = chosen_harness, required_unless_present = "clear")]
        harness: Option<Harness>,
        /// Forget this host's choice: each worker runs what its range and casting say again
        #[arg(long, conflicts_with = "harness")]
        clear: bool,
        /// The issue whose worker to move
        #[arg(value_name = "ISSUE", conflicts_with_all = ["all", "patrol"])]
        issue: Option<u64>,
        /// The patrol whose running worker to move
        #[arg(long, conflicts_with = "all")]
        patrol: Option<String>,
        /// Move the worker of every claim of the instance on this host, one after another
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Give up the landing train, its worker silent past a nudge, with nothing landed
    #[command(after_long_help = include_str!("help/abandon.txt"))]
    Abandon {
        #[command(flatten)]
        who: Who,
    },
    /// Take another fleet instance's claims without waiting for the lease
    #[command(after_long_help = include_str!("help/takeover.txt"))]
    Takeover {
        /// Show every instance the remote remembers, and take nothing
        #[arg(long, conflicts_with_all = ["from", "yes"])]
        list: bool,
        /// The instance whose claims to take
        #[arg(long)]
        from: Option<String>,
        /// Take from an instance whose heartbeat is still fresh
        #[arg(long)]
        yes: bool,
        /// The fleet instance to take them as
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
    },
    /// End the sleep of this host's `run` for this repo: its next tick starts now (a worker's command)
    #[command(after_long_help = include_str!("help/wake.txt"))]
    Wake,
    /// List every claim and heartbeat on the remote
    #[command(after_long_help = include_str!("help/scan.txt"))]
    Scan,
    /// Claim one issue for a fleet instance
    #[command(after_long_help = include_str!("help/claim.txt"))]
    Claim {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
    },
    /// Release a fleet instance's own claim on one issue
    #[command(after_long_help = include_str!("help/release.txt"))]
    Release {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
    },
    /// Refresh a fleet instance's heartbeat, if it is due
    #[command(after_long_help = include_str!("help/heartbeat.txt"))]
    Heartbeat {
        #[command(flatten)]
        who: Who,
    },
    /// Print this message, the help of the given command, or a help topic
    Help {
        /// A command, or a topic: `--help` lists both
        name: Option<String>,
    },
}

/// The one issue a command is about: its number, as an argument or as
/// `--issue`. One of the two, never both.
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
struct IssueArg {
    /// The issue's number
    #[arg(value_name = "ISSUE")]
    number: Option<u64>,
    /// The same, as a flag
    #[arg(long = "issue", value_name = "ISSUE")]
    flag: Option<u64>,
}

impl IssueArg {
    fn number(&self) -> u64 {
        // The group has clap refuse a command line that names neither.
        self.number.or(self.flag).unwrap_or_default()
    }
}

/// The team a command reads.
#[derive(clap::Args)]
struct TeamArgs {
    /// The team's directory on GitHub, as owner/repo or owner/repo/path [default: the
    /// team this checkout's config names]
    #[arg(long, value_parser = team_dir)]
    team: Option<RepoDir>,
    /// A directory of a team's files, read as they are instead of from GitHub; with
    /// --team, as an edit of that team: a role it names as ./path is read from that repo
    #[arg(long)]
    dir: Option<PathBuf>,
}

impl TeamArgs {
    fn from(self) -> team::From {
        match (self.team, self.dir) {
            (of, Some(dir)) => team::From::Dir(dir, of),
            (Some(team), None) => team::From::Repo(team),
            (None, None) => team::From::Config,
        }
    }
}

/// The text a stage record carries.
#[derive(clap::Args)]
struct TextArgs {
    /// The text of the record: what the stage produced, or what a human says
    #[arg(long, required_unless_present = "text_file", conflicts_with = "text_file")]
    text: Option<String>,
    /// A file holding that text
    #[arg(long)]
    text_file: Option<PathBuf>,
}

impl TextArgs {
    fn read(self) -> anyhow::Result<String> {
        match (self.text, self.text_file) {
            (Some(text), _) => Ok(text),
            (None, Some(file)) => std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display())),
            (None, None) => Ok(String::new()),
        }
    }
}

/// The host's network settings, never the repo's config.
#[derive(clap::Args, Default)]
struct ProxyArgs {
    /// Hosts reached directly, comma-separated, for the default harness or
    /// HARNESS; repeat for each harness. DSH defaults to api.deepseek.com.
    /// An empty list disables the stock addition. Extends no_proxy and NO_PROXY.
    #[arg(long, value_name = "[HARNESS=]HOSTS")]
    worker_no_proxy: Vec<String>,
}

/// How a command that starts a worker starts it.
#[derive(clap::Args)]
struct AgentArgs {
    // Keep only a diagnostic for callers migrating from the removed flag.
    #[arg(long = "worker-command", hide = true, value_parser = removed_worker_command)]
    _worker_command: Option<String>,

    #[command(flatten)]
    proxy: ProxyArgs,
    /// Seconds an agent is given to be ready for its prompt
    #[arg(long, default_value_t = 120, hide = true)]
    ready_timeout: u64,
}

/// The fleet instance a command acts as.
#[derive(clap::Args)]
struct Who {
    #[arg(long, value_parser = InstanceId::parse)]
    instance: InstanceId,
    /// The host the instance runs on [default: this machine's name]
    #[arg(long)]
    host: Option<String>,
}

impl Who {
    fn me(self, now: u64) -> Me {
        Me { instance: self.instance, host: self.host.unwrap_or_else(this_host), now }
    }
}

/// The start of a command line that runs this very binary, for a prompt: a
/// worker's shell need not have it on its PATH. What follows is its arguments.
pub fn own_command() -> Line {
    Line::new().program(&own_program())
}

/// The same, run from `dir`: a worker that starts in its role's copy runs its
/// commands from its worktree.
pub fn own_command_in(dir: &str) -> Line {
    Line::new().fixed("cd ").word(dir).fixed(" && ").program(&own_program())
}

fn own_program() -> String {
    let exe = std::env::current_exe().ok().and_then(|path| path.to_str().map(str::to_string));
    exe.unwrap_or_else(|| "afkconvoy".to_string())
}

/// The commands that act: each logs a line and wakes the `run` as it ends
/// (`Command::acting`), and its `--help` says so (`acting::HELP`).
const ACTS: [&str; 25] = [
    "dispatch",
    "cast",
    "close",
    "escalate",
    "park",
    "nudge",
    "restart",
    "switch",
    "abandon",
    "takeover",
    "claim",
    "release",
    "heartbeat",
    "rule",
    "send",
    "land",
    "verdict",
    "intake",
    "lock",
    "patrol-now",
    "over",
    "inspected",
    "gate",
    "wake",
    "tick",
];

/// The command line as parsed: `Cli`, each command that acts with what it logs
/// added to its `--help`.
fn cli() -> clap::Command {
    let mut cli = Cli::command();
    for name in ACTS {
        cli = cli.mut_subcommand(name, |command| {
            let help = command.get_after_long_help().map(ToString::to_string).unwrap_or_default();
            command.after_long_help(format!("{}\n\n{}\n", help.trim_end(), acting::HELP))
        });
    }
    cli
}

/// What `--help` opens with: `help.txt`, with the topics listed where it says
/// `{topics}`.
fn guide() -> String {
    include_str!("help.txt").replace("{topics}", &topic::listing())
}

/// What `help` prints: the guide with no name, the long help of the command
/// called `name`, or the topic called that. `None` for any other name.
fn help(name: Option<&str>) -> Option<String> {
    let mut cli = cli();
    // Built, so that a command's usage line names the whole command line.
    cli.build();
    let Some(name) = name else {
        return Some(cli.render_long_help().to_string());
    };
    match cli.find_subcommand_mut(name) {
        Some(command) => Some(command.render_long_help().to_string()),
        None => topic::text(name).map(str::to_string),
    }
}

fn repo(text: &str) -> Result<RepoName, String> {
    RepoName::read(text).ok_or_else(|| "a repo is named as owner/repo".to_string())
}

fn team_dir(text: &str) -> Result<RepoDir, String> {
    RepoDir::read(text).map_err(|_| {
        "a team is named as its repo or a directory of one: owner/repo or owner/repo/path"
            .to_string()
    })
}

fn declared(text: &str) -> Result<Declared, String> {
    Declared::read(text).ok_or_else(|| {
        let words: Vec<&str> = Declared::ALL.iter().map(|declared| declared.word()).collect();
        format!("what is declared is one of: {}", words.join(", "))
    })
}

/// A stage or consult verdict. A ruling answers an escalation separately.
fn verdict(text: &str) -> Result<Declared, String> {
    let verdicts = || Declared::ALL.into_iter().filter(|declared| *declared != Declared::Ruling);
    verdicts().find(|declared| declared.word() == text).ok_or_else(|| {
        let words: Vec<&str> = verdicts().map(Declared::word).collect();
        format!("a verdict is one of: {}", words.join(", "))
    })
}

fn found(text: &str) -> Result<Found, String> {
    Found::read(text).ok_or_else(|| {
        let words: Vec<&str> = Found::ALL.into_iter().map(Found::word).collect();
        format!("what an inspection found is one of: {}", words.join(", "))
    })
}

/// A harness `switch` moves onto, by its key. `config`, which once forgot
/// this host's choice, is refused for `--clear`, which does that now.
fn chosen_harness(text: &str) -> Result<Harness, String> {
    match text {
        "config" => Err("`--harness config` is gone: `--clear` forgets this host's choice".into()),
        _ => harness(text),
    }
}

fn harness(text: &str) -> Result<Harness, String> {
    Harness::read(text).ok_or_else(|| Harness::unknown(text))
}

/// What a worker that declares ran under, when the commits say one does: a
/// human names none. A worker started before its environment named a harness
/// names none either (`Harness::UNNAMED`).
fn ran(
    role_commit: Option<String>,
    team_commit: Option<String>,
    harness: Option<Harness>,
    model: Option<String>,
) -> Option<stage::Ran> {
    let (role_commit, team_commit) = role_commit.zip(team_commit)?;
    let harness = harness.unwrap_or(Harness::UNNAMED);
    Some(stage::Ran { role_commit, team_commit, harness, model })
}

pub(crate) fn this_host() -> String {
    gethostname::gethostname().to_string_lossy().into()
}

fn removed_worker_command(_: &str) -> Result<String, String> {
    Err("--worker-command is gone: run `afkconvoy kit` to set this host's worker launch commands"
        .into())
}

impl AgentArgs {
    /// Reads this host's host kit (`Launches::new`), with no probe:
    /// the one worker a command started this way starts is refused where its
    /// harness's command resolves to nothing (`Agent::command`).
    fn agent(self) -> anyhow::Result<Agent> {
        let launches = Launches::new(true)?.with_no_proxy(&self.proxy.worker_no_proxy)?;
        Ok(Agent { launches, ready_timeout: self.ready_timeout })
    }

    /// Makes the startup probe of a start that starts workers → what it
    /// starts them with. What each command needs of this host is the probe's
    /// to check: for every harness the fleet could be asked for, before any
    /// worker is started. A start that is `trusting` makes none while one
    /// that checked these commands stands (`probe::standing`).
    fn start(self, now: u64, trusting: bool) -> anyhow::Result<Agent> {
        let agent = self.agent()?;
        // The default harness's is settled whatever stands: a human is
        // asked, where one must be, before anything else is.
        agent.launches.key()?;
        let stands = if trusting { probe::standing(now, &agent.launches) } else { None };
        let warnings = match stands {
            Some(warnings) => warnings,
            None => probe::startup(now, &agent.launches)?,
        };
        probe::announce(&warnings);
        Ok(agent)
    }
}

/// Runs one command line. Prints exactly one JSON object on stdout: the
/// command's answer, or `{"error": …}` with a non-zero exit — `doctor` exits
/// non-zero with its answer, when that holds a critical finding. An answer that
/// is text — a drawing, the skill file — is printed as it is. `guard` answers the
/// harness that asks it, in that harness's way: for Claude Code, a hook's,
/// with its exit code and a reason on stderr.
pub fn main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let parsed =
        cli().try_get_matches_from(args).and_then(|matches| Cli::from_arg_matches(&matches));
    let cli = match parsed {
        Ok(cli) => cli,
        // `--help` and `--version` are asked for by a human: clap prints them.
        Err(e) if !e.use_stderr() => e.exit(),
        Err(e) => return fail(2, e.render().to_string().trim()),
    };
    if matches!(cli.command, Command::Guard) {
        return guard::guard();
    }
    if let Command::Exclusive { command } = &cli.command {
        return exclusive::exclusive(command, || cli.now.unwrap_or_else(clock));
    }
    if let Command::Help { name } = &cli.command {
        let Some(text) = help(name.as_deref()) else {
            return fail(2, &topic::unknown(name.as_deref().unwrap_or_default()));
        };
        print!("{text}");
        return ExitCode::SUCCESS;
    }
    let acting = cli.command.acting();
    let now = || cli.now.unwrap_or_else(clock);
    if let Some(acting) = &acting {
        acting.start(now());
    }
    let ran = run(cli.command, cli.now);
    let ran = match &acting {
        Some(acting) => acting.end(now(), ran),
        None => ran,
    };
    match ran {
        Ok(Value::String(drawing)) => {
            print!("{drawing}");
            ExitCode::SUCCESS
        }
        Ok(answer) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            if let Some(refused) = e.downcast_ref::<team::Refused>() {
                println!("{}", json!({ "error": refused.to_string(), "faults": refused.json() }));
            } else if let Some(doctor::Unwell(answer)) = e.downcast_ref() {
                // Not an error to report: the answer itself, which says what is wrong.
                println!("{answer}");
            } else if let Some(see) = topic::explains(&e) {
                println!("{}", json!({ "error": format!("{e:#}"), "see": see }));
            } else {
                return fail(1, &format!("{e:#}"));
            }
            ExitCode::from(1)
        }
    }
}

fn fail(code: u8, message: &str) -> ExitCode {
    println!("{}", json!({ "error": message }));
    ExitCode::from(code)
}

fn clock() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

impl Command {
    /// What it says of itself as it acts (`acting`), if it changes fleet
    /// state: writes to GitHub — issues, comments, labels, blocked-by edges —
    /// or to the fleet's refs on the remote. One that only reads says nothing.
    fn acting(&self) -> Option<Acting> {
        let of = |name, issue: &IssueArg, who: Option<&Who>| {
            let acting = Acting::new(name).on_issue(Some(issue.number()));
            match who {
                Some(who) => acting.as_instance(who.instance.as_str()),
                None => acting,
            }
        };
        let as_who = |name, who: &Who| Acting::new(name).as_instance(who.instance.as_str());
        Some(match self {
            Command::Dispatch { issue, who, .. } => of("dispatch", issue, Some(who)),
            Command::Close { issue, who } => of("close", issue, Some(who)),
            Command::Escalate { issue, who, .. } => of("escalate", issue, Some(who)),
            Command::Park { issue, who } => of("park", issue, Some(who)),
            Command::Nudge { issue, who } => of("nudge", issue, Some(who)),
            Command::Restart { issue, who, .. } => of("restart", issue, Some(who)),
            Command::Switch { issue, patrol, all, who, .. } => {
                let acting = Acting::new("switch").as_instance(who.instance.as_str());
                match (issue, patrol, all) {
                    (Some(issue), _, _) => acting.on_issue(Some(*issue)),
                    (None, Some(patrol), _) => acting.on(format!("patrol {}", patrol.trim())),
                    (None, None, true) => acting.on("all".to_string()),
                    (None, None, false) => acting,
                }
            }
            Command::Abandon { who } => as_who("abandon", who),
            Command::Takeover { list: true, .. } => return None,
            Command::Takeover { from, instance, .. } => {
                let acting = Acting::new("takeover");
                let acting = match from {
                    Some(from) => acting.on(format!("from {from}")),
                    None => acting,
                };
                match instance {
                    Some(instance) => acting.as_instance(instance.as_str()),
                    None => acting,
                }
            }
            Command::Claim { issue, who } => of("claim", issue, Some(who)),
            Command::Release { issue, who } => of("release", issue, Some(who)),
            Command::Heartbeat { who } => as_who("heartbeat", who),
            Command::Rule { issue, .. } => of("rule", issue, None),
            Command::Send { issue, .. } => of("send", issue, None),
            Command::Land { number, issue, .. } => Acting::new("land").on_issue(issue.or(*number)),
            Command::Verdict { issue, .. } => of("verdict", issue, None),
            Command::Intake { issue, .. } => of("intake", issue, None),
            Command::Lock { issue, .. } => of("lock", issue, None),
            Command::PatrolNow { name } => Acting::new("patrol-now").on(name.trim().to_string()),
            Command::Over { run } => Acting::new("over").on(format!("run {}", run.trim())),
            Command::Inspected { .. } => match std::env::var(INSPECTION_ENV) {
                Ok(worker) => Acting::new("inspected").on(worker),
                Err(_) => Acting::new("inspected"),
            },
            Command::Gate { .. } => Acting::new("gate").on_own_issue().saying_start(),
            Command::Wake => Acting::new("wake").on_own_issue(),
            Command::Cast => Acting::new("cast"),
            Command::Tick { who, .. } => as_who("tick", who),
            Command::Kit
            | Command::Config { .. }
            | Command::Check { .. }
            | Command::Scaffold { .. }
            | Command::Flows
            | Command::Skill
            | Command::Show { .. }
            | Command::Rehearse { .. }
            | Command::Doctor { .. }
            | Command::Plan { .. }
            | Command::Run { .. }
            | Command::Panel { .. }
            | Command::Guard
            | Command::Exclusive { .. }
            | Command::Recovery { .. }
            | Command::Scan
            | Command::Help { .. } => return None,
        })
    }
}

fn run(command: Command, fixed_now: Option<u64>) -> anyhow::Result<Value> {
    let now = fixed_now.unwrap_or_else(clock);
    match command {
        Command::Kit => kit::configure(),
        Command::Cast => caster::cast(now),
        Command::Config { file } => {
            let file = match file {
                Some(file) => file,
                None => config::default_file()?,
            };
            Ok(serde_json::to_value(config::load(&file)?)?)
        }
        Command::Check { team } => team::check(team.from()),
        Command::Scaffold { what, dir, role, example, repo, public } => {
            let create = repo.map(|repo| scaffold::Create { repo, public });
            // The parser took the name for one there is an example of.
            let example = example.as_deref().and_then(scaffold::example);
            scaffold::scaffold(what, &dir, &role, example, create.as_ref())
        }
        Command::Flows => flows::flows(&Repo::here()?),
        Command::Skill => Ok(Value::String(SKILL.to_string())),
        Command::Show { workflow, issue, team } => {
            let loaded = team::load(team.from())?;
            // A workflow is not named by a number: one given is an issue's.
            let numbered = workflow.as_deref().and_then(|named| named.parse().ok());
            match issue.or(numbered) {
                Some(issue) => {
                    let gh = gh::GitHub { dir: Repo::here()?.dir };
                    stage::show_issue(&gh, &loaded.team, issue)
                }
                None => Ok(Value::String(workflow::show(&loaded, workflow.as_deref())?)),
            }
        }
        Command::Rehearse { workflow, declared, team } => {
            workflow::rehearse(&team::load(team.from())?, &workflow, &declared)
        }
        Command::Doctor { host, proxy } => {
            doctor::doctor_with_proxy(now, &host.unwrap_or_else(this_host), &proxy.worker_no_proxy)
        }
        Command::Plan { instance, proxy } => {
            let launches = Launches::foreseen().with_no_proxy(&proxy.worker_no_proxy)?;
            let warnings = match probe::standing(now, &launches) {
                Some(warnings) => warnings,
                None => probe::startup(now, &launches)?,
            };
            probe::announce(&warnings);
            let (mut fleet, open) = Fleet::read(now)?;
            let (mut set, flows) = fleet.rebuild(&open, instance.as_ref())?;
            let admission = admission::Admission::read(&fleet.repo.dir)?;
            set.held_back = admission.holds();
            let mut plan = serde_json::to_value(set)?;
            if let Some(bound) = &fleet.config.bound {
                plan["intake"] = json!({"open": flows.taken_in(), "limit": bound.intake_limit});
            }
            plan["admission"] = admission.json();
            plan["host_kit"] = kit::show(now);
            plan["worker_no_proxy"] = launches.proxy_json();
            Ok(plan)
        }
        Command::Run {
            instance,
            host,
            agent,
            panel,
            no_host_sleep,
            host_sleep_after,
            hot_cores,
            hot_memory,
            hot_written,
            ticks,
            second_ms,
        } => {
            if !no_host_sleep && !cfg!(target_os = "macos") {
                anyhow::bail!(
                    "this host is not macOS, the one a fleet can put to sleep once every fleet \
                     on it is idle: start it with `--no-host-sleep`, which keeps it awake"
                );
            }
            let host = host.unwrap_or_else(this_host);
            let salt = clock() as u32 ^ std::process::id();
            let instance = instance.unwrap_or_else(|| InstanceId::mint(&host, salt));
            let mut me = Me { instance, host, now };
            let second = Duration::from_millis(second_ms);
            let hot = hot::limits(hot::Said {
                millicores: hot_cores,
                resident: hot_memory.map(|thousandths| thousandths.saturating_mul(1 << 30) / 1000),
                written: hot_written.map(|thousandths| thousandths.saturating_mul(1 << 30) / 1000),
            });
            let pacing = Pacing { ticks, second, no_host_sleep, host_sleep_after, hot };
            kit::ensure()?;
            let agent = agent.start(now, false)?;
            tick::run(&mut me, &agent, &pacing, panel, move || fixed_now.unwrap_or_else(clock))
        }
        Command::Panel { open: true, port, .. } => opening::open(&Repo::here()?, port),
        Command::Panel { instance, host, .. } => {
            let (mut fleet, open) = Fleet::read(now)?;
            let host = host.unwrap_or_else(this_host);
            let me = instance.map(|instance| Me { instance, host, now });
            let (set, flows) = fleet.rebuild(&open, me.as_ref().map(|me| &me.instance))?;
            let shown = panel::read(&fleet, me.as_ref(), &set, &flows, &[]);
            Ok(Value::String(panel::render(&shown)))
        }
        Command::Tick { who, agent } => {
            let agent = agent.start(now, true)?;
            Ok(tick::tick(&who.me(now), &agent, &stop::Stop::never(), None)?.0.json())
        }
        Command::Gate { train, host } => {
            let repo = Repo::here()?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let host = host.unwrap_or_else(this_host);
            gate::gate(&repo, &config, train, &host, now)
        }
        Command::Land { number, issue, merged_timeout, host, .. } => {
            let repo = Repo::here()?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub { dir: repo.dir.clone() };
            match issue.or(number) {
                Some(issue) => train::land(&repo, &config, &gh, issue, now),
                None => {
                    let merged = Duration::from_secs(merged_timeout);
                    let host = host.unwrap_or_else(this_host);
                    landing::land_train(&repo, &config, &gh, &host, now, merged)
                }
            }
        }
        Command::Verdict {
            verdict,
            issue,
            stage,
            text,
            blocked_by,
            role_commit,
            team_commit,
            harness,
            model,
            consult,
        } => {
            let declaring = stage::Declaring {
                issue: issue.number(),
                stage,
                declared: verdict,
                blocked_by,
                text: text.read()?,
                ran: ran(role_commit, team_commit, harness, model),
                consult,
            };
            stage::verdict(&Repo::here()?, declaring)
        }
        Command::Rule { issue, text, stage, role_commit, team_commit, harness, model } => {
            let issue = issue.number();
            // With the commits it ran under, the ruling is a consult's: a
            // stage record under its claim, as a verdict is.
            if let Some(ran) = ran(role_commit, team_commit, harness, model) {
                let Some(stage) = stage else {
                    anyhow::bail!(
                        "a consult rules with `--stage`, the stage that asked it: its brief \
                         gives the line. A human rules with no commit named and none in the \
                         environment"
                    );
                };
                let declaring = stage::Declaring {
                    issue,
                    stage,
                    declared: Declared::Ruling,
                    blocked_by: vec![],
                    text: text.read()?,
                    ran: Some(ran),
                    consult: true,
                };
                return stage::verdict(&Repo::here()?, declaring);
            }
            let team = team::load(team::From::Config)?.team;
            steer::rule(&Repo::here()?, &team, issue, text.read()?)
        }
        Command::Send { issue, stage, text, role_commit, team_commit, harness, model, consult } => {
            let issue = issue.number();
            let team = team::load(team::From::Config)?.team;
            // With the commits it ran under, the sending is a consult's: a
            // stage record under its claim, as its ruling is.
            if let Some(ran) = ran(role_commit, team_commit, harness, model) {
                if !consult {
                    anyhow::bail!(
                        "a worker sends an issue to a stage only as its stage's consult: a \
                         stage declares its verdict, and the workflow says where it leads"
                    );
                }
                return stage::consult_send(
                    &Repo::here()?,
                    &team,
                    issue,
                    &stage,
                    text.read()?,
                    ran,
                );
            }
            steer::send(&Repo::here()?, &team, issue, &stage, text.read()?)
        }
        Command::Intake { issue, workflow, role, from, patrol, train, blocked_by } => {
            use afkconvoy_core::intake::{By, On};
            let on = match (from, patrol) {
                (Some(issue), _) => Some(On::Issue(issue)),
                (None, Some(patrol)) => Some(On::Patrol(patrol)),
                (None, None) => None,
            };
            let by = match (train, role, on) {
                (true, None, None) => By::Train,
                (true, ..) => anyhow::bail!(
                    "the train worker has no role and is on no issue and no patrol: `--train` \
                     takes none of them ({ROLE_ENV}, {ISSUE_ENV} and {PATROL_ENV} unset)"
                ),
                (false, Some(role), Some(on)) => By::Role { role, on },
                (false, None, _) => anyhow::bail!(
                    "an intake names the role of its worker (--role, or {ROLE_ENV} in the \
                     environment) — or, the train worker's, `--train`"
                ),
                (false, Some(_), None) => anyhow::bail!(
                    "an intake names what its worker is on: the issue (--from, or {ISSUE_ENV} in \
                     the environment) or the patrol (--patrol, or {PATROL_ENV})"
                ),
            };
            let taking = intake::Taking { issue: issue.number(), workflow, by, blocked_by };
            intake::intake(&Repo::here()?, taking)
        }
        Command::Lock { issue, role } => {
            let repo = Repo::here()?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub { dir: repo.dir.clone() };
            let role = role.as_deref().map(str::trim).filter(|role| !role.is_empty());
            lock::lock(&config, &gh, issue.number(), role)
        }
        Command::PatrolNow { name } => patrol::ask(name.trim(), &this_host(), now),
        Command::Over { run } => {
            if let Ok(issue) = std::env::var(ISSUE_ENV) {
                anyhow::bail!(
                    "this worker is on issue #{issue} ({ISSUE_ENV}), whose stage ends with \
                     `afkconvoy verdict`: only a patrol's run and a proposal's are declared over"
                );
            }
            claims::declare_over(&Repo::here()?, run.trim())
        }
        Command::Inspected { found, text, reclaim, name } => {
            inspection::inspected(found, &text.read()?, &reclaim, &name)
        }
        // `main` answers it: a hook's answer is no JSON object.
        Command::Guard => anyhow::bail!("guard answers with its exit code"),
        Command::Exclusive { .. } => {
            anyhow::bail!("exclusive answers with its command's exit code")
        }
        Command::Help { .. } => anyhow::bail!("help answers with plain text"),
        Command::Dispatch { issue, who, agent, start } => dispatch::dispatch(
            &mut Fleet::open(now)?,
            &who.me(now),
            &agent.agent()?,
            issue.number(),
            start,
        ),
        Command::Recovery { issue } => recovery::recovery(&Fleet::open(now)?, issue.number()),
        Command::Close { issue, who } => {
            judge::close(&mut Fleet::open(now)?, &who.me(now), issue.number())
        }
        Command::Escalate { issue, who, reason } => {
            judge::escalate(&mut Fleet::open(now)?, &who.me(now), issue.number(), &reason)
        }
        Command::Park { issue, who } => {
            park::park(&mut Fleet::open(now)?, &who.me(now), issue.number())
        }
        Command::Nudge { issue, who } => {
            judge::nudge(&mut Fleet::open(now)?, &who.me(now), issue.number())
        }
        Command::Restart { issue, who, agent } => {
            watch::restart(&mut Fleet::open(now)?, &who.me(now), &agent.agent()?, issue.number())
        }
        Command::Switch { harness, clear: _, issue, patrol, all, who, agent } => {
            let which = match (issue, patrol, all) {
                (Some(issue), _, _) => switch::Which::Issue(issue),
                (None, Some(patrol), _) => switch::Which::Patrol(patrol.trim().to_string()),
                (None, None, true) => switch::Which::All,
                (None, None, false) => switch::Which::Nothing,
            };
            let mut fleet = Fleet::open(now)?;
            switch::switch(&mut fleet, &who.me(now), &agent.agent()?, harness, which)
        }
        Command::Abandon { who } => abandon::abandon_by_hand(&mut Fleet::open(now)?, &who.me(now)),
        Command::Takeover { list, from, yes, instance, host } => {
            let mut fleet = Fleet::open(now)?;
            let me =
                instance.map(|instance| Me { instance, host: host.unwrap_or_else(this_host), now });
            match (list, from, me) {
                (true, _, me) => Ok(takeover::list(&fleet, me.as_ref())),
                (false, Some(from), Some(me)) => takeover::takeover(&mut fleet, &me, &from, yes),
                (false, Some(_), None) => {
                    anyhow::bail!("takeover --from needs --instance: the fleet to take them as")
                }
                (false, None, _) => anyhow::bail!("takeover takes --list, or --from <instance id>"),
            }
        }
        // `Acting::end` sends it, as it does for every command that acts.
        Command::Wake => Ok(json!({})),
        Command::Scan => Ok(Scan::read(&Repo::here()?)?.json()),
        Command::Claim { issue, who } => {
            let repo = Repo::here()?;
            claims::take(&repo, &mut Scan::read(&repo)?, &who.me(now), issue.number(), Take::Fresh)
        }
        Command::Release { issue, who } => {
            let repo = Repo::here()?;
            claims::release(&repo, &mut Scan::read(&repo)?, &who.instance, issue.number())
        }
        Command::Heartbeat { who } => {
            let repo = Repo::here()?;
            claims::beat(&repo, &mut Scan::read(&repo)?, &who.me(now))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command line of each command, as little as it parses with.
    const LINES: &[&[&str]] = &[
        &["kit"],
        &["config"],
        &["check"],
        &["scaffold", "team", "x"],
        &["flows"],
        &["skill"],
        &["show"],
        &["rehearse", "build"],
        &["doctor"],
        &["plan"],
        &["run"],
        &["panel"],
        &["tick", "--instance", "a-h1-1"],
        &["gate"],
        &["land", "7"],
        &["verdict", "pass", "7", "--stage", "build", "--text", "x"],
        &["rule", "7", "--text", "x"],
        &["send", "7", "--stage", "build", "--text", "x"],
        &["intake", "7", "--workflow", "w", "--role", "r", "--from", "3"],
        &["lock", "7"],
        &["patrol-now", "sweep"],
        &["over", "--run", "sweep"],
        &["inspected", "--found", "heavy", "--text", "x"],
        &["guard"],
        &["exclusive", "--", "true"],
        &["dispatch", "7", "--instance", "a-h1-1"],
        &["recovery", "7"],
        &["close", "7", "--instance", "a-h1-1"],
        &["escalate", "7", "--instance", "a-h1-1", "--reason", "x"],
        &["park", "7", "--instance", "a-h1-1"],
        &["nudge", "7", "--instance", "a-h1-1"],
        &["restart", "7", "--instance", "a-h1-1"],
        &["switch", "7", "--harness", "codex", "--instance", "a-h1-1"],
        &["abandon", "--instance", "a-h1-1"],
        &["takeover", "--from", "b-h2-2", "--instance", "a-h1-1"],
        &["wake"],
        &["cast"],
        &["scan"],
        &["claim", "7", "--instance", "a-h1-1"],
        &["release", "7", "--instance", "a-h1-1"],
        &["heartbeat", "--instance", "a-h1-1"],
        &["help"],
    ];

    /// The commands that act are those whose `--help` says what they log, and
    /// none else: a test of the list `--help` gives, not of `ACTS` alone.
    #[test]
    fn a_command_logs_and_wakes_where_its_help_says_so_and_only_there() {
        let mut cli = cli();
        cli.build();
        let mut listed = vec![];
        for command in cli.get_subcommands_mut() {
            let name = command.get_name().to_string();
            let help = command.render_long_help().to_string();
            let line = LINES.iter().find(|line| line[0] == name);
            let line = line.unwrap_or_else(|| panic!("no command line of `{name}` here"));
            let parsed =
                Cli::try_parse_from(std::iter::once("afkconvoy").chain(line.iter().copied()));
            let acts = parsed.unwrap_or_else(|e| panic!("{name}: {e}")).command.acting().is_some();
            assert_eq!(acts, help.contains("\nWHAT IT LOGS\n"), "{name}");
            assert_eq!(acts, ACTS.contains(&name.as_str()), "{name}");
            listed.push(name);
        }
        assert_eq!(listed.len(), LINES.len(), "{listed:?}");
        // The listing with `--list` changes nothing, and says nothing.
        let list = Cli::try_parse_from(["afkconvoy", "takeover", "--list"]).unwrap();
        assert!(list.command.acting().is_none());
        // What one acts on is named, as its wake names it.
        let named = |line: &[&str]| {
            let cli = Cli::try_parse_from(std::iter::once("afkconvoy").chain(line.iter().copied()));
            cli.unwrap().command.acting().unwrap().named()
        };
        assert_eq!(named(&["park", "--issue", "46", "--instance", "a-h1-1"]), "park #46");
        assert_eq!(named(&["over", "--run", " sweep "]), "over run sweep");
        assert_eq!(named(&["patrol-now", "sweep"]), "patrol-now sweep");
        assert_eq!(
            named(&["takeover", "--from", "b-h2-2", "--instance", "a-h1-1"]),
            "takeover from b-h2-2"
        );
        assert_eq!(named(&["land", "--train"]), "land");
    }
}

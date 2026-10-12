//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::position::Declared;
use afkconvoy_core::role::{CONSULT_ENV, ISSUE_ENV, ROLE_COMMIT_ENV, ROLE_ENV, TEAM_COMMIT_ENV};

use anyhow::Context;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

pub mod abandon;
pub mod advance;
pub mod board;
pub mod claims;
pub mod config;
pub mod copies;
pub mod dispatch;
pub mod fleet;
pub mod gate;
pub mod gh;
pub mod git;
pub mod guard;
pub mod intake;
pub mod judge;
pub mod landing;
pub mod launch;
pub mod lock;
pub mod orca;
pub mod panel;
pub mod park;
pub mod patrol;
pub mod precedent;
pub mod probe;
pub mod recovery;
pub mod role;
pub mod scaffold;
pub mod serve;
pub mod settle;
pub mod stage;
pub mod steer;
pub mod takeover;
pub mod team;
pub mod tend;
pub mod tick;
pub mod tools;
pub mod train;
pub mod wake;
pub mod watch;
pub mod workflow;

use claims::{Me, Scan, Take};
use dispatch::Agent;
use fleet::Fleet;
use git::Repo;
use tick::Pacing;

/// `-h` is the one-screen listing; `--help` is `help.txt`, the text an agent
/// reads to learn the tool. Each subcommand keeps its one line for `-h`, and
/// its `--help` adds `help/<name>.txt`: when to run it, and what it answers.
#[derive(Parser)]
#[command(name = "afkconvoy", version, about, long_about = with_files(include_str!("help.txt")))]
struct Cli {
    /// The time to act at, in epoch seconds [default: the clock's]
    #[arg(long, global = true, hide = true)]
    now: Option<u64>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the repo's config as JSON, defaults filled
    #[command(after_long_help = include_str!("help/config.txt"))]
    Config {
        /// The config file [default: docs/agents/afkconvoy.md of this checkout]
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Read a team and its workflows: what was read, or every fault
    #[command(after_long_help = with_files(include_str!("help/check.txt")))]
    Check {
        #[command(flatten)]
        team: TeamArgs,
    },
    /// Write the skeleton of a team's repo or of a role's, and create it on GitHub if asked
    #[command(after_long_help = include_str!("help/scaffold.txt"))]
    Scaffold {
        /// What to write the skeleton of
        #[arg(value_enum)]
        what: scaffold::What,
        /// The directory to write it in: one not there yet, or empty
        dir: PathBuf,
        /// A team's one role, which its example workflow is performed by, as <name>=<owner/repo>
        #[arg(long, value_parser = scaffold::named)]
        role: Option<scaffold::Named>,
        /// Also commit the skeleton and create this repo on GitHub from it, as owner/repo
        #[arg(long, value_parser = repo)]
        repo: Option<String>,
        /// Create that repo public [default: private]
        #[arg(long, requires = "repo")]
        public: bool,
    },
    /// Draw a team's workflows as graphs, or say where one issue is in its workflow
    #[command(after_long_help = include_str!("help/show.txt"))]
    Show {
        /// The workflow to draw [default: every workflow of the team]
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
    /// Show what a tick would act on, and change nothing
    #[command(after_long_help = include_str!("help/plan.txt"))]
    Plan {
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
        /// Stop after this many ticks
        #[arg(long, hide = true)]
        ticks: Option<u64>,
        /// Milliseconds one second of sleep takes
        #[arg(long, hide = true, default_value_t = 1000)]
        second_ms: u64,
    },
    /// Print the panel — the fleet as a web page — as one rebuild shows it now
    #[command(after_long_help = include_str!("help/panel.txt"))]
    Panel {
        /// The fleet instance whose claims are its own [default: nobody's]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
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
        /// Seconds the gate command is given before it is killed, and red
        #[arg(long, default_value_t = 3600)]
        gate_timeout: u64,
        /// First merge in what the PR lands behind: the landing train, or the base branch
        #[arg(long)]
        train: bool,
    },
    /// Put this worktree's PR on the landing train (a worker's command), or land the train
    #[command(after_long_help = include_str!("help/land.txt"))]
    Land {
        /// The issue the PR closes
        #[arg(long, required_unless_present = "train", conflicts_with = "train")]
        issue: Option<u64>,
        /// Gate the landing train and land it on the base branch (the train worker's command)
        #[arg(long)]
        train: bool,
        /// Seconds the gate command is given before it is killed, and red
        #[arg(long, default_value_t = 3600)]
        gate_timeout: u64,
        /// Seconds GitHub is given to show a landed PR as merged
        #[arg(long, default_value_t = 60)]
        merged_timeout: u64,
    },
    /// Declare the verdict that ends a stage, with the text it produced (a worker's
    /// command, and a human's for a stage nobody holds)
    #[command(after_long_help = include_str!("help/verdict.txt"))]
    Verdict {
        /// What the stage declares: pass, reject, blocked, already-satisfied, or escalate
        #[arg(value_parser = verdict)]
        verdict: Declared,
        #[arg(long)]
        issue: u64,
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
        /// The worker is the stage's consult, letting its `escalate` through [a human is none]
        #[arg(long, env = CONSULT_ENV)]
        consult: bool,
    },
    /// Answer an escalated issue with a ruling: its stage runs again under it (a human's
    /// command, and a consult's for the stage that asked it)
    #[command(after_long_help = include_str!("help/rule.txt"))]
    Rule {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        text: TextArgs,
        /// The stage whose `escalate` a consult rules on [a human names none]
        #[arg(long, requires = "role_commit")]
        stage: Option<String>,
        /// The commit of the role's repo a consult ran under [a human names none]
        #[arg(long, env = ROLE_COMMIT_ENV, requires = "team_commit")]
        role_commit: Option<String>,
        /// The commit of the team's repo a consult ran under [a human names none]
        #[arg(long, env = TEAM_COMMIT_ENV, requires = "role_commit")]
        team_commit: Option<String>,
    },
    /// Send an issue to a named stage of its workflow (a human's command)
    #[command(after_long_help = include_str!("help/send.txt"))]
    Send {
        #[arg(long)]
        issue: u64,
        /// The stage to put it at
        #[arg(long)]
        stage: String,
        #[command(flatten)]
        text: TextArgs,
    },
    /// Take an issue in to a workflow: its flow label, on the record (a worker's command)
    #[command(after_long_help = include_str!("help/intake.txt"))]
    Intake {
        /// The issue to take in
        #[arg(long)]
        issue: u64,
        /// The workflow it is to follow, by name: what follows `flow:` in its flow label
        #[arg(long)]
        workflow: String,
        /// The role of the worker that takes it in
        #[arg(long, env = ROLE_ENV)]
        role: String,
        /// The issue that worker is on
        #[arg(long, env = ISSUE_ENV, conflicts_with = "patrol")]
        from: Option<u64>,
        /// The patrol that worker runs, when it is on no issue
        #[arg(long, env = PATROL_ENV)]
        patrol: Option<String>,
        /// The issues it is to wait for, as numbers separated by commas
        #[arg(long, value_delimiter = ',')]
        blocked_by: Vec<u64>,
    },
    /// Raise a fleet lock on an issue: the fleet works lock issues only until it closes (a worker's command)
    #[command(after_long_help = include_str!("help/lock.txt"))]
    Lock {
        /// The issue that carries the fix, and becomes the lock
        #[arg(long)]
        issue: u64,
        /// The role that raises it [default: the one this worker was started in, if any]
        #[arg(long, env = ROLE_ENV)]
        role: Option<String>,
    },
    /// Allow or refuse the tool call described on stdin (a worker's hook, added at launch)
    #[command(after_long_help = include_str!("help/guard.txt"))]
    Guard,
    /// Start a fresh worker on one issue
    #[command(after_long_help = include_str!("help/dispatch.txt"))]
    Dispatch {
        #[arg(long)]
        issue: u64,
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
        #[arg(long)]
        issue: u64,
    },
    /// Close one of my claims whose issue needed no change
    #[command(after_long_help = include_str!("help/close.txt"))]
    Close {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Hand one of my claims to a human
    #[command(after_long_help = include_str!("help/escalate.txt"))]
    Escalate {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
        /// Why: what a human needs to act on it in one reply
        #[arg(long)]
        reason: String,
    },
    /// Leave one of my claims waiting on the issues its worker declared it blocked by
    #[command(after_long_help = include_str!("help/park.txt"))]
    Park {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Tell the worker of one of my claims, stopped with no PR and no verdict, to carry on
    #[command(after_long_help = include_str!("help/nudge.txt"))]
    Nudge {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Replace the silent worker of one of my claims whose PR is joining the train, once
    #[command(after_long_help = include_str!("help/restart.txt"))]
    Restart {
        #[arg(long)]
        issue: u64,
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
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Release a fleet instance's own claim on one issue
    #[command(after_long_help = include_str!("help/release.txt"))]
    Release {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Refresh a fleet instance's heartbeat, if it is due
    #[command(after_long_help = include_str!("help/heartbeat.txt"))]
    Heartbeat {
        #[command(flatten)]
        who: Who,
    },
}

/// The team a command reads.
#[derive(clap::Args)]
struct TeamArgs {
    /// The team's repo, as owner/repo [default: the team this checkout's config names]
    #[arg(long, value_parser = team_repo)]
    team: Option<String>,
    /// A directory of a team's files, read as they are instead of from GitHub
    #[arg(long, conflicts_with = "team")]
    dir: Option<PathBuf>,
}

impl TeamArgs {
    fn from(self) -> team::From {
        match (self.team, self.dir) {
            (_, Some(dir)) => team::From::Dir(dir),
            (Some(repo), None) => team::From::Repo(repo),
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

/// How a command that starts a worker starts it.
#[derive(clap::Args)]
struct AgentArgs {
    /// The worker launch command, handed to orca as it is — in a bound repo, with what
    /// starts a worker in its role added [default: with no ANTHROPIC_BASE_URL,
    /// `claude --dangerously-skip-permissions`; with one, asked for]
    #[arg(long)]
    worker_command: Option<String>,
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

/// The command line that runs this very binary with `arguments`, for a prompt:
/// a worker's shell need not have it on its PATH.
pub fn own_command(arguments: &str) -> String {
    let exe = std::env::current_exe().ok().and_then(|path| path.to_str().map(str::to_string));
    let exe = exe.unwrap_or_else(|| "afkconvoy".to_string());
    let plain = exe.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-+".contains(&b));
    let exe = if plain { exe } else { format!("'{}'", exe.replace('\'', "'\\''")) };
    format!("{exe} {arguments}")
}

/// A help text with the files and keys of a team and a role put in where it
/// says `{files}`: `help/files.txt` is the one place they are written.
fn with_files(help: &str) -> String {
    help.replace("{files}", include_str!("help/files.txt").trim_end())
}

fn repo(text: &str) -> Result<String, String> {
    if afkconvoy_core::config::is_repo(text) {
        Ok(text.to_string())
    } else {
        Err("a repo is named as owner/repo".to_string())
    }
}

fn team_repo(text: &str) -> Result<String, String> {
    if afkconvoy_core::config::is_repo(text) {
        Ok(text.to_string())
    } else {
        Err("a team is named as its repo: owner/repo".to_string())
    }
}

fn declared(text: &str) -> Result<Declared, String> {
    Declared::read(text).ok_or_else(|| {
        let words: Vec<&str> = Declared::ALL.iter().map(|declared| declared.word()).collect();
        format!("what is declared is one of: {}", words.join(", "))
    })
}

/// One of the five verdicts. A ruling is not one: it answers an escalation.
fn verdict(text: &str) -> Result<Declared, String> {
    let verdicts = || Declared::ALL.into_iter().filter(|declared| *declared != Declared::Ruling);
    verdicts().find(|declared| declared.word() == text).ok_or_else(|| {
        let words: Vec<&str> = verdicts().map(Declared::word).collect();
        format!("a verdict is one of: {}", words.join(", "))
    })
}

fn this_host() -> String {
    gethostname::gethostname().to_string_lossy().into()
}

impl AgentArgs {
    /// Settles the worker launch command (`launch::settle`): nothing is
    /// started on one that resolves to nothing.
    fn agent(self) -> anyhow::Result<Agent> {
        let command = launch::settle(self.worker_command.as_deref())?;
        Ok(Agent { command, ready_timeout: self.ready_timeout })
    }

    /// Makes the startup probe of a start that starts workers → what it
    /// starts them with. The probe is told the command before it is settled:
    /// what the command needs of this host is the probe's to check.
    fn start(self, now: u64) -> anyhow::Result<Agent> {
        let proposed = launch::propose(self.worker_command.as_deref())?;
        probe::announce(&probe::startup(now, Some(&proposed))?);
        Ok(Agent { command: proposed.settle()?, ready_timeout: self.ready_timeout })
    }
}

/// Runs one command line. Prints exactly one JSON object on stdout: the
/// command's answer, or `{"error": …}` with a non-zero exit. An answer that
/// is text — a drawing — is printed as it is. `guard` answers Claude Code,
/// in the way a hook does: with its exit code, and a reason on stderr.
pub fn main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // `--help` and `--version` are asked for by a human: clap prints them.
        Err(e) if !e.use_stderr() => e.exit(),
        Err(e) => return fail(2, e.render().to_string().trim()),
    };
    if matches!(cli.command, Command::Guard) {
        return guard::guard();
    }
    match run(cli.command, cli.now) {
        Ok(Value::String(drawing)) => {
            print!("{drawing}");
            ExitCode::SUCCESS
        }
        Ok(answer) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Err(e) => match e.downcast_ref::<team::Refused>() {
            Some(refused) => {
                println!("{}", json!({ "error": refused.to_string(), "faults": refused.json() }));
                ExitCode::from(1)
            }
            None => fail(1, &format!("{e:#}")),
        },
    }
}

fn fail(code: u8, message: &str) -> ExitCode {
    println!("{}", json!({ "error": message }));
    ExitCode::from(code)
}

fn clock() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

fn run(command: Command, fixed_now: Option<u64>) -> anyhow::Result<Value> {
    let now = fixed_now.unwrap_or_else(clock);
    match command {
        Command::Config { file } => {
            let file = match file {
                Some(file) => file,
                None => config::default_file()?,
            };
            Ok(serde_json::to_value(config::load(&file)?)?)
        }
        Command::Check { team } => team::check(team.from()),
        Command::Scaffold { what, dir, role, repo, public } => {
            let create = repo.map(|repo| scaffold::Create { repo, public });
            scaffold::scaffold(what, &dir, role.as_ref(), create.as_ref())
        }
        Command::Show { workflow, issue, team } => {
            let loaded = team::load(team.from())?;
            match issue {
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
        Command::Plan { instance } => {
            probe::announce(&probe::startup(now, launch::foreseen().as_ref())?);
            let fleet = Fleet::open(now)?;
            let (set, flows) = fleet.rebuild(instance.as_ref())?;
            let mut plan = serde_json::to_value(set)?;
            if let Some(bound) = &fleet.config.bound {
                plan["intake"] = json!({"open": flows.taken_in(), "limit": bound.intake_limit});
            }
            Ok(plan)
        }
        Command::Run { instance, host, agent, panel, ticks, second_ms } => {
            let host = host.unwrap_or_else(this_host);
            let salt = clock() as u32 ^ std::process::id();
            let instance = instance.unwrap_or_else(|| InstanceId::mint(&host, salt));
            let mut me = Me { instance, host, now };
            let pacing = Pacing { ticks, second: Duration::from_millis(second_ms) };
            let agent = agent.start(now)?;
            tick::run(&mut me, &agent, &pacing, panel, || fixed_now.unwrap_or_else(clock))
        }
        Command::Panel { instance, host } => {
            let fleet = Fleet::open(now)?;
            let host = host.unwrap_or_else(this_host);
            let me = instance.map(|instance| Me { instance, host, now });
            let (set, flows) = fleet.rebuild(me.as_ref().map(|me| &me.instance))?;
            let shown = panel::read(&fleet, me.as_ref(), &set, &flows, &[]);
            Ok(Value::String(panel::render(&shown)))
        }
        Command::Tick { who, agent } => {
            let agent = agent.start(now)?;
            Ok(tick::tick(&mut Fleet::open(now)?, &who.me(now), &agent)?.json())
        }
        Command::Gate { gate_timeout, train } => {
            let repo = Repo::here()?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            gate::gate(&repo, &config, train, now, Duration::from_secs(gate_timeout))
        }
        Command::Land { issue, gate_timeout, merged_timeout, .. } => {
            let repo = Repo::here()?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub { dir: repo.dir.clone() };
            match issue {
                Some(issue) => train::land(&repo, &config, &gh, issue, now),
                None => {
                    let limits = landing::Limits {
                        gate: Duration::from_secs(gate_timeout),
                        merged: Duration::from_secs(merged_timeout),
                    };
                    landing::land_train(&repo, &config, &gh, now, &limits)
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
            consult,
        } => {
            let declaring = stage::Declaring {
                issue,
                stage,
                declared: verdict,
                blocked_by,
                text: text.read()?,
                commits: role_commit.zip(team_commit),
                consult,
            };
            stage::verdict(&Repo::here()?, declaring)
        }
        Command::Rule { issue, text, stage, role_commit, team_commit } => {
            // With the commits it ran under, the ruling is a consult's: a
            // stage record under its claim, as a verdict is.
            if let Some(commits) = role_commit.zip(team_commit) {
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
                    commits: Some(commits),
                    consult: true,
                };
                return stage::verdict(&Repo::here()?, declaring);
            }
            let team = team::load(team::From::Config)?.team;
            steer::rule(&Repo::here()?, &team, issue, text.read()?)
        }
        Command::Send { issue, stage, text } => {
            let team = team::load(team::From::Config)?.team;
            steer::send(&Repo::here()?, &team, issue, &stage, text.read()?)
        }
        Command::Intake { issue, workflow, role, from, patrol, blocked_by } => {
            let from = match (from, patrol) {
                (Some(issue), _) => afkconvoy_core::intake::On::Issue(issue),
                (None, Some(patrol)) => afkconvoy_core::intake::On::Patrol(patrol),
                (None, None) => anyhow::bail!(
                    "an intake names what its worker is on: the issue (--from, or {ISSUE_ENV} in \
                     the environment) or the patrol (--patrol, or {PATROL_ENV})"
                ),
            };
            let taking = intake::Taking { issue, workflow, role, from, blocked_by };
            intake::intake(&Repo::here()?, taking)
        }
        Command::Lock { issue, role } => {
            let repo = Repo::here()?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub { dir: repo.dir.clone() };
            let role = role.as_deref().map(str::trim).filter(|role| !role.is_empty());
            lock::lock(&repo, &config, &gh, issue, role)
        }
        // `main` answers it: a hook's answer is no JSON object.
        Command::Guard => anyhow::bail!("guard answers with its exit code"),
        Command::Dispatch { issue, who, agent, start } => {
            dispatch::dispatch(&mut Fleet::open(now)?, &who.me(now), &agent.agent()?, issue, start)
        }
        Command::Recovery { issue } => recovery::recovery(&Fleet::open(now)?, issue),
        Command::Close { issue, who } => judge::close(&mut Fleet::open(now)?, &who.me(now), issue),
        Command::Escalate { issue, who, reason } => {
            judge::escalate(&mut Fleet::open(now)?, &who.me(now), issue, &reason)
        }
        Command::Park { issue, who } => park::park(&mut Fleet::open(now)?, &who.me(now), issue),
        Command::Nudge { issue, who } => judge::nudge(&mut Fleet::open(now)?, &who.me(now), issue),
        Command::Restart { issue, who, agent } => {
            watch::restart(&mut Fleet::open(now)?, &who.me(now), &agent.agent()?, issue)
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
        Command::Wake => wake::wake(&Repo::here()?),
        Command::Scan => Ok(Scan::read(&Repo::here()?)?.json()),
        Command::Claim { issue, who } => {
            let repo = Repo::here()?;
            claims::take(&repo, &mut Scan::read(&repo)?, &who.me(now), issue, Take::Fresh)
        }
        Command::Release { issue, who } => {
            let repo = Repo::here()?;
            claims::release(&repo, &mut Scan::read(&repo)?, &who.instance, issue)
        }
        Command::Heartbeat { who } => {
            let repo = Repo::here()?;
            claims::beat(&repo, &mut Scan::read(&repo)?, &who.me(now))
        }
    }
}

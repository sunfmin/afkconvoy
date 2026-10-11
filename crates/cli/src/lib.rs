//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use afkconvoy_core::claims::InstanceId;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

pub mod abandon;
pub mod board;
pub mod claims;
pub mod config;
pub mod dispatch;
pub mod fleet;
pub mod gate;
pub mod gh;
pub mod git;
pub mod judge;
pub mod landing;
pub mod launch;
pub mod orca;
pub mod park;
pub mod probe;
pub mod recovery;
pub mod settle;
pub mod takeover;
pub mod tend;
pub mod tick;
pub mod train;
pub mod wake;
pub mod watch;

use claims::{Me, Scan, Take};
use dispatch::Agent;
use fleet::Fleet;
use git::Repo;
use tick::Pacing;

#[derive(Parser)]
#[command(name = "afkconvoy", version, about)]
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
    Config {
        /// The config file [default: docs/agents/afkconvoy.md of this checkout]
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Show what a tick would act on, and change nothing
    Plan {
        /// The fleet instance whose claims are its own [default: nobody's]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
    },
    /// Run the fleet: tick after tick, until stopped
    Run {
        /// The fleet instance to run as [default: a new one, named for this host]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        agent: AgentArgs,
        /// Stop after this many ticks
        #[arg(long, hide = true)]
        ticks: Option<u64>,
        /// Milliseconds one second of sleep takes
        #[arg(long, hide = true, default_value_t = 1000)]
        second_ms: u64,
    },
    /// Run one reconciliation pass and exit
    Tick {
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Run the gate in this worktree and put a green run on record (a worker's command)
    Gate {
        /// Seconds the gate command is given before it is killed, and red
        #[arg(long, default_value_t = 3600)]
        gate_timeout: u64,
        /// First merge in what the PR lands behind: the landing train, or the base branch
        #[arg(long)]
        train: bool,
    },
    /// Put this worktree's PR on the landing train (a worker's command), or land the train
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
    /// Start a fresh worker on one issue
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
    Recovery {
        #[arg(long)]
        issue: u64,
    },
    /// Close one of my claims whose issue needed no change
    Close {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Hand one of my claims to a human
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
    Park {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Tell the worker of one of my claims, stopped with no PR and no verdict, to carry on
    Nudge {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Replace the silent worker of one of my claims whose PR is joining the train, once
    Restart {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Give up the landing train, its worker silent past a nudge, with nothing landed
    Abandon {
        #[command(flatten)]
        who: Who,
    },
    /// Take another fleet instance's claims without waiting for the lease
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
    Wake,
    /// List every claim and heartbeat on the remote
    Scan,
    /// Claim one issue for a fleet instance
    Claim {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Release a fleet instance's own claim on one issue
    Release {
        #[arg(long)]
        issue: u64,
        #[command(flatten)]
        who: Who,
    },
    /// Refresh a fleet instance's heartbeat, if it is due
    Heartbeat {
        #[command(flatten)]
        who: Who,
    },
}

/// How a command that starts a worker starts it.
#[derive(clap::Args)]
struct AgentArgs {
    /// The worker launch command, handed to orca as it is [default: with no
    /// ANTHROPIC_BASE_URL, `claude --dangerously-skip-permissions`; with one, asked for]
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
}

/// Runs one command line. Prints exactly one JSON object on stdout: the
/// command's answer, or `{"error": …}` with a non-zero exit.
pub fn main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // `--help` and `--version` are asked for by a human: clap prints them.
        Err(e) if !e.use_stderr() => e.exit(),
        Err(e) => return fail(2, e.render().to_string().trim()),
    };
    match run(cli.command, cli.now) {
        Ok(answer) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(1, &format!("{e:#}")),
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
        Command::Plan { instance } => {
            probe::announce(&probe::startup(now)?);
            Ok(serde_json::to_value(Fleet::open(now)?.rebuild(instance.as_ref())?)?)
        }
        Command::Run { instance, host, agent, ticks, second_ms } => {
            let host = host.unwrap_or_else(this_host);
            let salt = clock() as u32 ^ std::process::id();
            let instance = instance.unwrap_or_else(|| InstanceId::mint(&host, salt));
            let mut me = Me { instance, host, now };
            let pacing = Pacing { ticks, second: Duration::from_millis(second_ms) };
            probe::announce(&probe::startup(now)?);
            tick::run(&mut me, &agent.agent()?, &pacing, || fixed_now.unwrap_or_else(clock))
        }
        Command::Tick { who, agent } => {
            probe::announce(&probe::startup(now)?);
            Ok(tick::tick(&mut Fleet::open(now)?, &who.me(now), &agent.agent()?)?.json())
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

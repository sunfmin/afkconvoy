//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use afkconvoy_core::claims::InstanceId;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

pub mod claims;
pub mod config;
pub mod dispatch;
pub mod fleet;
pub mod gate;
pub mod gh;
pub mod git;
pub mod landing;
pub mod orca;
pub mod settle;
pub mod tend;
pub mod tick;
pub mod train;

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
    },
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
    /// The worker launch command, handed to orca as it is
    #[arg(long)]
    worker_command: String,
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
    fn agent(self) -> Agent {
        Agent { command: self.worker_command, ready_timeout: self.ready_timeout }
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
            Ok(serde_json::to_value(Fleet::open(now)?.rebuild(instance.as_ref())?)?)
        }
        Command::Run { instance, host, agent, ticks, second_ms } => {
            let host = host.unwrap_or_else(this_host);
            let salt = clock() as u32 ^ std::process::id();
            let instance = instance.unwrap_or_else(|| InstanceId::mint(&host, salt));
            let mut me = Me { instance, host, now };
            let pacing = Pacing { ticks, second: Duration::from_millis(second_ms) };
            tick::run(&mut me, &agent.agent(), &pacing, || fixed_now.unwrap_or_else(clock))
        }
        Command::Tick { who, agent } => {
            Ok(tick::tick(&mut Fleet::open(now)?, &who.me(now), &agent.agent())?.json())
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
        Command::Dispatch { issue, who, agent } => {
            dispatch::dispatch(&mut Fleet::open(now)?, &who.me(now), &agent.agent(), issue)
        }
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

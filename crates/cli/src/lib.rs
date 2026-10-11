//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use afkconvoy_core::claims::InstanceId;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

pub mod claims;
pub mod config;
pub mod fleet;
pub mod gh;
pub mod git;

use claims::{Me, Scan, Take};
use fleet::Fleet;
use git::Repo;

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
        let host = self.host.unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into());
        Me { instance: self.instance, host, now }
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
    let now = cli.now.unwrap_or_else(clock);
    match run(cli.command, now) {
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

fn run(command: Command, now: u64) -> anyhow::Result<Value> {
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

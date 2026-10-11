//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

pub mod config;

#[derive(Parser)]
#[command(name = "afkconvoy", version, about)]
struct Cli {
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
    match run(cli.command) {
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

fn run(command: Command) -> anyhow::Result<Value> {
    match command {
        Command::Config { file } => {
            let file = match file {
                Some(file) => file,
                None => config::default_file()?,
            };
            Ok(serde_json::to_value(config::load(&file)?)?)
        }
    }
}

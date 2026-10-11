//! Settles the worker launch command at a start: asks the login shell — the
//! one a worker gets — what the command's first word is, and a human which
//! command, when nobody said and the fleet is on a custom provider.
//!
//! The command goes to orca as it was given. It is put in no file and no
//! environment, and nothing of this process's environment goes with it.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::launch::{
    LaunchCommand, Source, candidates, checked, first_word, no_reply, question, replied, source,
};
use anyhow::{Result, anyhow, bail};

/// How long the login shell is given to answer.
const SHELL_TIMEOUT: Duration = Duration::from_secs(20);

/// The command every worker of this start is started with: `supplied`
/// (`--worker-command`), the stock one, or a human's answer — checked to
/// resolve in a login shell before any worker is started.
pub fn settle(supplied: Option<&str>) -> Result<LaunchCommand> {
    let provider = std::env::var("ANTHROPIC_BASE_URL").ok();
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let command = match source(provider.as_deref(), supplied, terminal) {
        Source::Given(command) => command,
        Source::Ask => ask(
            provider.as_deref().unwrap_or_default(),
            &login_shell("alias").unwrap_or_default(),
            &mut std::io::stdin().lock(),
            &mut std::io::stderr(),
        )?,
        Source::Refuse(why) => bail!(why),
    };
    let resolution = match first_word(&command) {
        Some(word) => login_shell(&format!("type -- '{}'", word.replace('\'', "'\\''"))),
        None => None,
    };
    let command = checked(&command, resolution.as_deref()).map_err(|why| anyhow!(why))?;
    if let Some(warning) = command.warning() {
        eprintln!("warning: {warning}");
    }
    Ok(command)
}

/// Asks which command starts a worker, offering the aliases `alias` printed
/// → the answer.
fn ask(
    provider: &str,
    aliases: &str,
    replies: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<String> {
    let offered = candidates(aliases);
    write!(out, "{}", question(provider, &offered))?;
    out.flush()?;
    let mut reply = String::new();
    replies.read_line(&mut reply)?;
    replied(&reply, &offered).ok_or_else(|| anyhow!(no_reply()))
}

/// Runs `script` in the user's interactive login shell — where aliases and
/// rc-defined functions exist, and what orca gives a worker → what it
/// printed. `None` when it failed, timed out, or exited non-zero: `type`
/// prints "not found" and exits 1, so its output alone proves nothing.
fn login_shell(script: &str) -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|shell| !shell.is_empty());
    let mut child = Command::new(shell.as_deref().unwrap_or("/bin/zsh"))
        .args(["-lic", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut printed = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = printed.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if started.elapsed() >= SHELL_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let text = reader.join().ok()?;
    status.success().then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALIASES: &str = "kim='ANTHROPIC_BASE_URL=https://kimi claude --yolo'\nll='ls -l'\n";

    fn asked(reply: &str) -> (Result<String>, String) {
        let mut out = vec![];
        let answer = ask("https://kimi", ALIASES, &mut reply.as_bytes(), &mut out);
        (answer, String::from_utf8(out).unwrap())
    }

    #[test]
    fn the_human_is_shown_the_aliases_and_picks_one() {
        let (answer, shown) = asked("1\n");
        assert_eq!(answer.unwrap(), "kim");
        assert!(
            shown.contains("1. kim = ANTHROPIC_BASE_URL=https://kimi claude --yolo"),
            "{shown}"
        );
        assert!(!shown.contains("ls -l"), "only the ones that start Claude: {shown}");
    }

    #[test]
    fn a_typed_command_is_the_answer_as_typed() {
        assert_eq!(asked("glm  --model 'big one'\n").0.unwrap(), "glm  --model 'big one'");
    }

    #[test]
    fn no_reply_starts_nothing() {
        for nothing in ["\n", ""] {
            let error = asked(nothing).0.unwrap_err().to_string();
            assert!(error.contains("nothing was started"), "{error}");
        }
    }
}

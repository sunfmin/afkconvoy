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

/// The command a start would give its workers, with what the login shell
/// says its first word is: not yet one a worker may be started with.
pub struct Proposed {
    command: String,
    /// `type`'s answer about the first word; `None` when it found nothing.
    resolution: Option<String>,
}

impl Proposed {
    /// The command, exactly as it was given.
    pub fn as_str(&self) -> &str {
        &self.command
    }

    /// Whether its first word resolves in a login shell.
    pub fn resolves(&self) -> bool {
        self.resolution.is_some()
    }

    /// Why it cannot start a worker, when it cannot.
    pub fn fault(&self) -> Option<String> {
        checked(&self.command, self.resolution.as_deref()).err()
    }

    /// The command as one a worker may be started with, warning a human of
    /// what they should know about it. An error is its `fault`.
    pub fn settle(self) -> Result<LaunchCommand> {
        let command =
            checked(&self.command, self.resolution.as_deref()).map_err(|why| anyhow!(why))?;
        if let Some(warning) = command.warning() {
            eprintln!("warning: {warning}");
        }
        Ok(command)
    }
}

/// The command every worker of this start is started with: `supplied`
/// (`--worker-command`), the stock one, or a human's answer — checked to
/// resolve in a login shell before any worker is started.
pub fn settle(supplied: Option<&str>) -> Result<LaunchCommand> {
    propose(supplied)?.settle()
}

/// The command `settle` would check: `supplied`, the stock one, or a human's
/// answer, with its first word looked up in a login shell.
pub fn propose(supplied: Option<&str>) -> Result<Proposed> {
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
    Ok(resolved(command))
}

/// The command a `run` started here with no `--worker-command` would give
/// its workers, for a start that starts none (`plan`): the stock one, or —
/// on a custom provider, where a human would be asked — unknown.
pub fn foreseen() -> Option<Proposed> {
    let provider = std::env::var("ANTHROPIC_BASE_URL").ok();
    match source(provider.as_deref(), None, false) {
        Source::Given(command) => Some(resolved(command)),
        Source::Ask | Source::Refuse(_) => None,
    }
}

fn resolved(command: String) -> Proposed {
    let resolution = first_word(&command)
        .and_then(|word| login_shell(&format!("type -- '{}'", word.replace('\'', "'\\''"))))
        .filter(|answer| !answer.trim().is_empty());
    Proposed { command, resolution }
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

/// Runs `script` in the user's interactive login shell → what it printed.
/// `None` when it failed, timed out, or exited non-zero: `type` prints "not
/// found" and exits 1, so its output alone proves nothing.
fn login_shell(script: &str) -> Option<String> {
    let (succeeded, printed) = ask_login_shell(script)?;
    succeeded.then_some(printed)
}

/// Runs `script` in the user's interactive login shell — where aliases and
/// rc-defined functions exist, and what orca gives a worker: it inherits
/// nothing from the shell the fleet was started in → whether it exited zero,
/// and what it printed. `None` when it could not run or timed out.
pub fn ask_login_shell(script: &str) -> Option<(bool, String)> {
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
    Some((status.success(), reader.join().ok()?))
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

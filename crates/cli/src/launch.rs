//! Settles the worker launch commands of a start, one for each harness its
//! workers run: asks the login shell — the one a worker gets — what a
//! command's first word is, and a human which command, when nobody said and
//! the fleet is on a custom provider.
//!
//! A command goes to orca as it was given. It is put in no file and no
//! environment, and nothing of this process's environment goes with it.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::gate::key;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::launch::{
    LaunchCommand, Source, Supplied, candidates, checked, first_word, no_reply, of_harness,
    question, replied, source, supplied,
};
use anyhow::{Result, anyhow, bail};

use crate::config;
use crate::topic::{Explain, See};

/// How long the login shell is given to answer.
const SHELL_TIMEOUT: Duration = Duration::from_secs(20);

/// The command a start would give the workers of one harness, with what the
/// login shell says its first word is: not yet one a worker may be started
/// with.
pub struct Proposed {
    harness: Harness,
    command: String,
    /// `type`'s answer about the first word; `None` when it found nothing.
    resolution: Option<String>,
}

impl Proposed {
    /// The harness whose workers it would start.
    pub fn harness(&self) -> Harness {
        self.harness
    }

    /// The command, exactly as it was given.
    pub fn as_str(&self) -> &str {
        &self.command
    }

    /// Whether its first word resolves in a login shell.
    pub fn resolves(&self) -> bool {
        self.resolution.is_some()
    }

    /// What the login shell prints when the command is asked which flags it
    /// takes, as its harness asks (`Harness::flags_question`); `None` when
    /// that failed.
    pub fn flags(&self) -> Option<String> {
        login_shell(&self.harness.flags_question(&self.command))
    }

    /// Why it cannot start a worker, when it cannot.
    pub fn fault(&self) -> Option<String> {
        self.checked().err()
    }

    /// The command as one a worker may be started with. An error is its
    /// `fault`.
    fn checked(&self) -> Result<LaunchCommand, String> {
        checked(&self.command, self.resolution.as_deref())
    }
}

/// The worker launch commands of one start: what `--worker-command` said,
/// and the command of each harness as it was proposed — looked up, and a
/// human asked, the first time that harness's is wanted, and once.
pub struct Launches {
    supplied: Supplied,
    /// The default harness of the repo here: the one the bare form names.
    default: Harness,
    /// Whether the repo here is bound: an unbound repo knows of no harness,
    /// and a fault of its one command names none.
    bound: bool,
    /// Whether this is a start that starts workers. One that starts none
    /// (`plan`, `doctor`) asks no human, and a command only a human could
    /// say is unknown to it.
    starts: bool,
    proposed: Vec<(Harness, OnceCell<Option<Proposed>>)>,
}

impl Launches {
    /// The commands of a start given the values `given` to
    /// `--worker-command`. The default harness is the one the config of the
    /// repo here names — Claude Code where no config can be read: whatever
    /// kept it from being read is the startup probe's to report, after the
    /// commands that read it were looked for. An error is `supplied`'s.
    pub fn new(given: &[String], starts: bool) -> Result<Launches> {
        let mut launches = Launches::foreseen();
        let supplied = supplied(given, launches.default);
        launches.supplied = supplied.map_err(|why| See::Troubleshooting.refusal(why))?;
        launches.starts = starts;
        Ok(launches)
    }

    /// The commands a `run` started here with no `--worker-command` would
    /// give its workers, for a start that starts none (`plan`).
    pub fn foreseen() -> Launches {
        let config = config::default_file().and_then(|file| config::load(&file)).ok();
        Launches {
            supplied: Supplied::default(),
            default: config.as_ref().map_or(Harness::DEFAULT, |config| config.harness()),
            bound: config.is_some_and(|config| config.bound.is_some()),
            starts: false,
            proposed: Harness::ALL.map(|harness| (harness, OnceCell::new())).into(),
        }
    }

    pub fn default(&self) -> Harness {
        self.default
    }

    pub fn bound(&self) -> bool {
        self.bound
    }

    pub fn starts(&self) -> bool {
        self.starts
    }

    /// Whether `--worker-command` said the command of `harness`.
    pub fn says(&self, harness: Harness) -> bool {
        self.supplied.of(harness).is_some()
    }

    /// The command the workers of `harness` would be started with: what
    /// `--worker-command` said for it, its stock one, or a human's answer,
    /// with its first word looked up in a login shell. `None` only of a
    /// start that starts no worker: on a custom provider, where a human
    /// would be asked, it is unknown. An error is nobody to ask, or no reply.
    pub fn of(&self, harness: Harness) -> Result<Option<&Proposed>> {
        let cell = self.proposed.iter().find(|(named, _)| *named == harness).map(|(_, cell)| cell);
        let cell = cell.ok_or_else(|| anyhow!("`{}` is no harness", harness.key()))?;
        if cell.get().is_none() {
            let proposed = self.propose(harness).see(See::Troubleshooting)?;
            if let Some(Ok(command)) = proposed.as_ref().map(Proposed::checked)
                && let Some(warning) = command.warning().filter(|_| self.starts)
            {
                eprintln!("warning: {warning}");
            }
            let _ = cell.set(proposed);
        }
        Ok(cell.get().and_then(Option::as_ref))
    }

    fn propose(&self, harness: Harness) -> Result<Option<Proposed>> {
        let provider = harness.provider_variable().and_then(|name| std::env::var(name).ok());
        let terminal =
            self.starts && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        let command =
            match source(harness, provider.as_deref(), self.supplied.of(harness), terminal) {
                Source::Given(command) => command,
                Source::Ask => ask(
                    provider.as_deref().unwrap_or_default(),
                    harness,
                    &login_shell("alias").unwrap_or_default(),
                    &mut std::io::stdin().lock(),
                    &mut std::io::stderr(),
                )?,
                Source::Refuse(why) if self.starts => bail!(why),
                Source::Refuse(_) => return Ok(None),
            };
        let resolution = first_word(&command)
            .and_then(|word| login_shell(&format!("type -- '{}'", word.replace('\'', "'\\''"))))
            .filter(|answer| !answer.trim().is_empty());
        Ok(Some(Proposed { harness, command, resolution }))
    }

    /// The command every worker of `harness` is started with, checked to
    /// resolve in a login shell: no worker is started on one that does not.
    pub fn command(&self, harness: Harness) -> Result<LaunchCommand> {
        let proposed = self.of(harness)?;
        let proposed = proposed.ok_or_else(|| See::Troubleshooting.refusal(no_reply()))?;
        let checked = proposed.checked();
        checked.map_err(|why| See::Troubleshooting.refusal(self.said(&why, harness, &[])))
    }

    /// `fault`, of the command of `harness`, as a human is told it: in a
    /// bound repo, with the harness and what calls for it (`callers`; none
    /// known: the harness alone).
    pub fn said(&self, fault: &str, harness: Harness, callers: &[String]) -> String {
        match callers {
            _ if !self.bound => fault.to_string(),
            [] => format!(
                "{fault} — it is the worker launch command of the harness `{}`",
                harness.key()
            ),
            callers => of_harness(fault, harness, callers),
        }
    }

    /// What tells one start's commands from another's, for a startup probe
    /// that checked them (`gate::key`: a command is written to no file): the
    /// default harness's, and what `--worker-command` said. `None`: the
    /// default's is unknown.
    pub fn key(&self) -> Result<Option<u64>> {
        let Some(default) = self.of(self.default)? else { return Ok(None) };
        let mut text = default.as_str().to_string();
        for (harness, command) in self.supplied.each() {
            text.push_str(&format!("\n{}={command}", harness.key()));
        }
        Ok(Some(key(&text)))
    }

    /// What a `run` logs of its commands: the first word of the default
    /// harness's, and that of each other one `--worker-command` said, by the
    /// harness's key. A first word alone: what follows may name what a human
    /// keeps to themselves.
    pub fn logged(&self) -> (String, BTreeMap<String, String>) {
        let word = |command: &str| first_word(command).unwrap_or_default().to_string();
        let default = self.of(self.default).ok().flatten().map(|command| word(command.as_str()));
        let others = self.supplied.each().filter(|(harness, _)| *harness != self.default);
        let others = others.map(|(harness, command)| (harness.key().to_string(), word(command)));
        (default.unwrap_or_default(), others.collect())
    }
}

/// Asks which command starts a worker of `harness`, offering the aliases
/// `alias` printed → the answer.
fn ask(
    provider: &str,
    harness: Harness,
    aliases: &str,
    replies: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<String> {
    let offered = candidates(aliases, harness);
    write!(out, "{}", question(provider, harness, &offered))?;
    out.flush()?;
    let mut reply = String::new();
    replies.read_line(&mut reply)?;
    replied(&reply, &offered).ok_or_else(|| anyhow!(no_reply()))
}

/// The models `harness` knows on this host, as it says them to a login shell
/// — the one a worker gets — when its stock worker launch command is asked
/// (`Harness::models_question`), with what this host keeps for it. `None`: it
/// has no way to say. An error is the question that failed, or listed none.
pub fn models(harness: Harness) -> Option<Result<Vec<String>, String>> {
    // The logins are in the directory kept with the host user's skills and
    // rules as they are in the one kept without.
    let kept = crate::kept::keep(harness, false).ok()?;
    let question = harness.models_question(harness.stock(), &kept.display().to_string())?;
    let listed = login_shell(&question).and_then(|answer| harness.models(&answer));
    Some(listed.ok_or(question))
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
        let answer =
            ask("https://kimi", Harness::ClaudeCode, ALIASES, &mut reply.as_bytes(), &mut out);
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

//! Reads this host's worker launch commands from its host kit and checks
//! their first words in the login shell a worker gets. Commands are passed
//! to orca as written, under nice with the worker's own TMPDIR.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::gate::key;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::launch::{LaunchCommand, checked, first_word, of_harness};
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

/// One start's snapshot of the host kit, with shell resolutions checked once.
pub struct Launches {
    kit: Result<crate::kit::Kit, String>,
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
    /// Read the host kit for a start that may start workers.
    pub fn new(starts: bool) -> Result<Launches> {
        let mut launches = Self::foreseen();
        if let Err(why) = &launches.kit {
            bail!("{why}");
        }
        launches.starts = starts;
        Ok(launches)
    }

    pub fn foreseen() -> Launches {
        let config = config::default_file().and_then(|file| config::load(&file)).ok();
        Launches {
            kit: crate::kit::load().map_err(|error| format!("{error:#}")),
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

    /// Whether the host kit holds this harness.
    pub fn says(&self, harness: Harness) -> bool {
        self.kit.as_ref().is_ok_and(|kit| kit.commands.contains_key(harness.key()))
    }

    /// The host kit's command, with its first word resolved in the login shell.
    /// An absent harness is an error; no stock command is substituted.
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
        let kit = self.kit.as_ref().map_err(|why| anyhow!("{why}"))?;
        let command = kit.commands.get(harness.key()).cloned().ok_or_else(|| anyhow!(
            "the host kit has no worker launch command for the harness `{}`: run `afkconvoy kit`",
            harness.key()
        ))?;
        let resolution = first_word(&command)
            .and_then(|word| login_shell(&format!("type -- '{}'", word.replace('\'', "'\\''"))))
            .filter(|answer| !answer.trim().is_empty());
        Ok(Some(Proposed { harness, command, resolution }))
    }

    /// The command every worker of `harness` is started with, checked to
    /// resolve in a login shell: no worker is started on one that does not.
    pub fn command(&self, harness: Harness) -> Result<LaunchCommand> {
        let proposed = self.of(harness)?;
        let proposed = proposed.ok_or_else(|| {
            See::Troubleshooting
                .refusal("no worker launch command in the host kit: run `afkconvoy kit`")
        })?;
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

    /// Fingerprint the entire kit so any edit invalidates a standing probe.
    pub fn key(&self) -> Result<Option<u64>> {
        let Some(default) = self.of(self.default)? else { return Ok(None) };
        let mut text = default.as_str().to_string();
        for (harness, command) in
            self.kit.as_ref().into_iter().flat_map(|kit| kit.commands.iter()).filter_map(
                |(name, command)| Harness::read(name).map(|harness| (harness, command.as_str())),
            )
        {
            text.push_str(&format!("\n{}={command}", harness.key()));
        }
        Ok(Some(key(&text)))
    }

    /// First words alone for the run log; commands remain in the host kit.
    pub fn logged(&self) -> (String, BTreeMap<String, String>) {
        let word = |command: &str| first_word(command).unwrap_or_default().to_string();
        let default = self.of(self.default).ok().flatten().map(|command| word(command.as_str()));
        let others = self
            .kit
            .as_ref()
            .into_iter()
            .flat_map(|kit| kit.commands.iter())
            .filter_map(|(name, command)| {
                Harness::read(name).map(|harness| (harness, command.as_str()))
            })
            .filter(|(harness, _)| *harness != self.default);
        let others = others.map(|(harness, command)| (harness.key().to_string(), word(command)));
        (default.unwrap_or_default(), others.collect())
    }
}

/// Whether the harness program is installed on this host's PATH.
pub fn on_path(program: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(program))
        .any(|file| {
            use std::os::unix::fs::PermissionsExt;
            file.is_file()
                && file.metadata().is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
        })
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

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
    bypass: afkconvoy_core::proxy::Lists,
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
            bypass: afkconvoy_core::proxy::Lists::default(),
            default: config.as_ref().map_or(Harness::DEFAULT, |config| config.harness()),
            bound: config.is_some_and(|config| config.bound.is_some()),
            starts: false,
            proposed: Harness::ALL.map(|harness| (harness, OnceCell::new())).into(),
        }
    }

    /// The host's proxy bypass additions, keyed by harness or bare for the default.
    pub fn with_no_proxy(mut self, given: &[String]) -> Result<Self> {
        self.bypass = afkconvoy_core::proxy::lists(given, self.default)
            .map_err(|why| See::Troubleshooting.refusal(why))?;
        Ok(self)
    }

    /// Wraps the entire start, including role setup, in the worker shell's
    /// proxy environment. The opaque worker launch command stays intact.
    pub fn worker_line(&self, harness: Harness, command: &str) -> String {
        afkconvoy_core::proxy::line(command, afkconvoy_core::proxy::addition(&self.bypass, harness))
    }

    /// Per harness, what a worker's login shell would export now.
    pub fn proxy_json(&self) -> serde_json::Value {
        // A login rc file may print anything before the answer. Frame the
        // two values so those lines cannot become proxy bypass hosts.
        let inherited =
            login_shell("printf 'AFKCONVOY_PROXY\\000%s\\000%s' \"${no_proxy-}\" \"${NO_PROXY-}\"")
                .unwrap_or_default();
        let values = inherited.split_once("AFKCONVOY_PROXY\0").map_or("", |(_, values)| values);
        let (lower, upper) = values.split_once('\0').unwrap_or(("", ""));
        serde_json::Value::Array(Harness::ALL.into_iter().map(|harness| {
            let addition = afkconvoy_core::proxy::addition(&self.bypass, harness);
            let (lower, upper) = afkconvoy_core::proxy::effective(lower, upper, addition);
            serde_json::json!({"harness": harness, "addition": addition, "no_proxy": lower, "NO_PROXY": upper})
        }).collect())
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
        let command = kit.command(harness).map(str::to_string).ok_or_else(|| anyhow!(
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
        for (harness, command) in self.kit.as_ref().into_iter().flat_map(|kit| kit.commands()) {
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
            .flat_map(|kit| kit.commands())
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
/// — the one a worker gets — when its worker launch command `command` is
/// asked (`Harness::models_question`), with what this host keeps for it.
/// `None`: it has no way to say. An error is the question that failed, or
/// listed none.
pub fn models(harness: Harness, command: &str) -> Option<Result<Vec<String>, String>> {
    // The logins are in the directory kept with the host user's skills and
    // rules as they are in the one kept without.
    let kept = crate::kept::keep(harness, false).ok()?;
    let question = harness.models_question(command, &kept.display().to_string())?;
    let listed = login_shell(&question).and_then(|answer| harness.models(&answer));
    Some(listed.ok_or(question))
}

/// The one-line prompt a model is probed with.
const PROBE_PROMPT: &str = "Reply with the single word OK.";

/// How long a model is given to answer its probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// What comes before a probe's answer in what the login shell prints: what
/// its rc files print comes before it.
const PROBE_MARK: &str = "AFKCONVOY_PROBE_ANSWER";

/// Sends `model` of `harness` the one-line probe through the worker launch
/// command `command`, in the login shell a worker gets, with what this host
/// keeps for it → whether it answered within the time limit, and how fast.
pub fn probe(harness: Harness, command: &str, model: Option<&str>) -> crate::kit::Answer {
    use crate::kit::Answer;
    let Ok(kept) = crate::kept::keep(harness, false) else { return Answer::Silent };
    let kept = kept.display().to_string();
    let Some(question) = harness.prompt_question(command, &kept, model, PROBE_PROMPT) else {
        return Answer::Unsendable;
    };
    let started = Instant::now();
    let script = format!("printf '%s\\n' {PROBE_MARK} && {question}");
    match ask_login_shell_within(&script, PROBE_TIMEOUT) {
        Some((true, printed)) if answered(&printed) => {
            Answer::Answered(started.elapsed().as_secs_f64())
        }
        _ => Answer::Silent,
    }
}

/// Whether what a probe printed holds an answer after its mark.
fn answered(printed: &str) -> bool {
    printed.split_once(PROBE_MARK).is_some_and(|(_, answer)| !answer.trim().is_empty())
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
    ask_login_shell_within(script, SHELL_TIMEOUT)
}

/// `ask_login_shell`, given `timeout` to answer.
pub(crate) fn ask_login_shell_within(script: &str, timeout: Duration) -> Option<(bool, String)> {
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
            None if started.elapsed() >= timeout => {
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

    #[test]
    fn a_probe_answered_only_with_words_after_its_mark() {
        assert!(answered("rc noise\nAFKCONVOY_PROBE_ANSWER\nOK\n"));
        assert!(!answered("rc noise\nAFKCONVOY_PROBE_ANSWER\n  \n"));
        assert!(!answered("OK"));
    }
}

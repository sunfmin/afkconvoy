//! Reads this host's worker launch commands from its host kit and checks
//! their first words in the login shell a worker gets. Commands are passed
//! to orca as written, under nice with the worker's own TMPDIR.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::gate::key;
use afkconvoy_core::harness::{AnsweredAs, Harness};
use afkconvoy_core::launch::{LaunchCommand, checked, first_word, of_harness};
use anyhow::{Result, anyhow, bail};

use crate::config;
use crate::topic::{Explain, See};

/// How long the login shell is given to answer.
pub(crate) const SHELL_TIMEOUT: Duration = Duration::from_secs(20);

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
        let kit = crate::kit::load().map_err(|error| format!("{error:#}"));
        // The host kit's first is what a worker nothing casts runs.
        let first = kit.as_ref().ok().and_then(|kit| kit.first()).map(|first| first.harness);
        Launches {
            kit,
            bypass: afkconvoy_core::proxy::Lists::default(),
            default: first.unwrap_or(Harness::DEFAULT),
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

/// Whether `harness`'s program is found in an orca terminal — the shell a
/// worker gets (`orca::ask`) — as `type` finds it there. An error says why it
/// is taken as not installed: it was not found, or could not be looked for.
pub fn found(harness: Harness) -> Result<(), String> {
    let question = format!("type -- '{}'", harness.program());
    match crate::orca::ask(&question, SHELL_TIMEOUT) {
        Ok(Some((true, _))) => Ok(()),
        Ok(Some((false, _))) => Err(format!("`{question}` found nothing")),
        Ok(None) => Err(format!("`{question}` did not answer in time")),
        Err(error) => Err(format!("{error:#}: {question}")),
    }
}

/// The models `harness` knows on this host, as it says them in an orca
/// terminal — the shell a worker gets (`orca::ask`) — when its worker launch
/// command `command` is asked (`Harness::models_question`), with what this
/// host keeps for it: those its own login runs. An error says why it listed
/// none — the question failed, timed out, or answered with no model — and
/// the question.
pub fn models(harness: Harness, command: &str) -> Result<Vec<crate::kit::Listed>, String> {
    // The logins are in the directory kept with the host user's skills and
    // rules as they are in the one kept without.
    let kept = crate::kept::keep(harness, false).map_err(|error| format!("{error:#}"))?;
    let question = harness.models_question(command, &kept.display().to_string());
    let why = match crate::orca::ask(&question, SHELL_TIMEOUT) {
        Ok(Some((true, printed))) => match harness.models(&printed) {
            Some(listed) => {
                let shown = harness.shown(&printed);
                let shown_as = |name: &str| {
                    shown.iter().find(|(listed, _)| listed == name).map(|(_, shown)| shown.clone())
                };
                let each = |name: String| crate::kit::Listed { shown: shown_as(&name), name };
                return Ok(listed.into_iter().map(each).collect());
            }
            None => {
                if let Some(why) = harness.listing_failure(&printed) {
                    return Err(format!("{why}: {question}"));
                }
                "it listed none"
            }
        },
        Ok(Some((false, _))) => "it failed",
        Ok(None) => "it did not answer in time",
        Err(error) => return Err(format!("{error:#}: {question}")),
    };
    Err(format!("{why}: {question}"))
}

/// The one-line prompt a model is probed with.
const PROBE_PROMPT: &str = "Reply with the single word OK.";

/// How long a model is given to answer its probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Sends `model` of `harness` the one-line probe (`Harness::probe_question`)
/// through the worker launch command `command`, with what this host keeps for
/// it, in an orca terminal — the shell a worker gets (`orca::ask`) → whether
/// it answered within the time limit, how fast, and which model answered
/// (`answered_as`); why it could not be asked, when it could not.
pub fn probe(harness: Harness, command: &str, model: Option<&str>) -> crate::kit::Answer {
    use crate::kit::Answer;
    let kept = match crate::kept::keep(harness, false) {
        Ok(kept) => kept,
        Err(error) => return Answer::Unasked(format!("{error:#}")),
    };
    let question =
        harness.probe_question(command, &kept.display().to_string(), model, PROBE_PROMPT);
    let started = Instant::now();
    match crate::orca::ask(&question, PROBE_TIMEOUT) {
        Ok(Some((true, printed))) if !printed.trim().is_empty() => {
            let seconds = started.elapsed().as_secs_f64();
            Answer::Answered(seconds, answered_as(harness, &printed, &kept))
        }
        Ok(_) => Answer::Silent,
        Err(error) => Answer::Unasked(format!("{error:#}")),
    }
}

/// Which model answered a probe of `harness` that printed `printed`
/// (`Harness::answered_as`): from the session file it names, under `kept`,
/// where it names one — which is then deleted. `None`: nothing says.
fn answered_as(harness: Harness, printed: &str, kept: &std::path::Path) -> Option<String> {
    match harness.answered_as(printed) {
        AnsweredAs::Said(id) => Some(id),
        AnsweredAs::Unsaid => None,
        AnsweredAs::InSession { under, ends } => {
            let session = found_under(&kept.join(under), &ends)?;
            let answered = std::fs::read_to_string(&session)
                .ok()
                .and_then(|text| harness.answered_in_session(&text));
            let _ = std::fs::remove_file(&session);
            answered
        }
    }
}

/// The file under `dir`, at any depth, whose name ends with `ends`.
fn found_under(dir: &std::path::Path, ends: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = found_under(&path, ends) {
                return Some(found);
            }
        } else if path.file_name().and_then(|name| name.to_str()).is_some_and(|n| n.ends_with(ends))
        {
            return Some(path);
        }
    }
    None
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

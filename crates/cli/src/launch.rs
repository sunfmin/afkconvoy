//! Reads this host's worker launch commands from its host kit and checks
//! their first words in the login shell a worker gets. Commands are passed
//! to orca as written, under nice with the worker's own TMPDIR.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use afkconvoy_core::gate::key;
use afkconvoy_core::harness::{AnsweredAs, Harness};
use afkconvoy_core::launch::{LaunchCommand, checked, first_word, of_harness};
use anyhow::{Result, anyhow, bail};

use crate::config;
use crate::git::Repo;
use crate::look::Reading;
use crate::topic::See;

/// How long the login shell is given to answer.
pub(crate) const SHELL_TIMEOUT: Duration = Duration::from_secs(20);

/// One doctor's harness budget. While it stands, each harness program is
/// started at most once in this process, including every checkout of
/// `doctor --all`. A tick never takes one, so its starts stay unlimited.
pub(crate) struct HarnessBudget;

static DEPTH: AtomicUsize = AtomicUsize::new(0);

struct Start {
    question: String,
    result: OnceLock<(bool, String)>,
}

static STARTED: Mutex<BTreeMap<String, Arc<Start>>> = Mutex::new(BTreeMap::new());

impl Drop for HarnessBudget {
    fn drop(&mut self) {
        if DEPTH.fetch_sub(1, Ordering::AcqRel) == 1 {
            STARTED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        }
    }
}

/// Begins a doctor's harness budget. Dropping the outermost one ends it.
pub(crate) fn budget_once() -> HarnessBudget {
    if DEPTH.fetch_add(1, Ordering::AcqRel) == 0 {
        STARTED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
    }
    HarnessBudget
}

/// What `once_per_program` did with `program`.
pub(crate) enum Once {
    /// `ask` ran, or the same question already had this answer.
    Ran(bool, String),
    /// `program` already started for a different question. It was not started again.
    Already,
}

/// Runs `ask` at most once per `program` while a doctor budget stands.
/// The same question is answered from the first start. A different question
/// does not start the program again. With no budget, `ask` always runs.
pub(crate) fn once_per_program(
    program: &str,
    question: &str,
    ask: impl FnOnce() -> (bool, String),
) -> Once {
    if DEPTH.load(Ordering::Acquire) == 0 {
        let (ok, printed) = ask();
        return Once::Ran(ok, printed);
    }
    let start = {
        let mut started = STARTED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = started.get(program) {
            if existing.question != question {
                return Once::Already;
            }
            existing.clone()
        } else {
            let start = Arc::new(Start { question: question.to_string(), result: OnceLock::new() });
            started.insert(program.to_string(), Arc::clone(&start));
            start
        }
    };
    // The lock is not held across `ask`: another checkout waits on this
    // start instead of starting the program again.
    let (ok, printed) = start.result.get_or_init(ask);
    Once::Ran(*ok, printed.clone())
}

/// The command a start would give the workers of one harness, with what the
/// login shell says its first word is: not yet one a worker may be started
/// with.
#[derive(Clone)]
pub struct Proposed {
    harness: Harness,
    command: String,
    /// `type`'s answer about the first word; `None` when it found nothing.
    resolution: Option<String>,
    /// Where its login shell was asked (`Launches::dir`).
    dir: PathBuf,
    /// The flags question's answer, shared by every clone of this resolution.
    /// A probe records what the ask found without asking the shell again.
    flags_answer: Arc<OnceLock<Option<String>>>,
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

    /// Where its login shell was asked, and is asked again about it: the
    /// checkout of the start it is for.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the login shell prints when the command is asked which flags it
    /// takes, as its harness asks (`Harness::flags_question`); `None` when
    /// that failed.
    pub fn flags(&self) -> Option<String> {
        self.flags_answer
            .get_or_init(|| login_shell(&self.dir, &self.harness.flags_question(&self.command)))
            .clone()
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

/// Reads the latest host kit at each launch; shell resolutions are reused only
/// while the command itself is unchanged.
pub struct Launches {
    bypass: afkconvoy_core::proxy::Lists,
    /// The default harness of the repo it is for: the one the bare form names.
    default: Harness,
    /// Whether that repo is bound: an unbound repo knows of no harness,
    /// and a fault of its one command names none.
    bound: bool,
    /// Whether this is a start that starts workers. One that starts none
    /// (`plan`, `doctor`) asks no human, and a command only a human could
    /// say is unknown to it.
    starts: bool,
    /// The checkout of that repo the start is in, where the login shell is
    /// asked: a worker's shell starts in a worktree of it, with the files
    /// that set up a toolchain for the repo there too, and nothing is asked
    /// in the directory the process happens to run in.
    dir: PathBuf,
    proposed: RefCell<Vec<Proposed>>,
    /// What the commands it resolved warn of, until whoever reports them
    /// takes them (`warned`).
    warned: RefCell<Vec<String>>,
}

impl Launches {
    /// Read the host kit for a start in the checkout `repo` that may start
    /// workers.
    pub fn new(repo: &Repo, starts: bool) -> Result<Launches> {
        let mut launches = Self::foreseen(repo);
        crate::kit::load()?;
        launches.starts = starts;
        Ok(launches)
    }

    /// Read the host kit for a command in the checkout `repo` that starts no
    /// worker.
    pub fn foreseen(repo: &Repo) -> Launches {
        let config = config::load(&config::file_of(repo)).ok();
        let kit = crate::kit::load().map_err(|error| format!("{error:#}"));
        // The host kit's first is what a worker nothing casts runs.
        let first = kit.as_ref().ok().and_then(|kit| kit.first()).map(|first| first.harness);
        Launches {
            bypass: afkconvoy_core::proxy::Lists::default(),
            default: first.unwrap_or(Harness::DEFAULT),
            bound: config.is_some_and(|config| config.bound.is_some()),
            starts: false,
            dir: repo.dir.clone(),
            proposed: RefCell::default(),
            warned: RefCell::default(),
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
        let asked = "printf 'AFKCONVOY_PROXY\\000%s\\000%s' \"${no_proxy-}\" \"${NO_PROXY-}\"";
        let inherited = login_shell(&self.dir, asked).unwrap_or_default();
        let values = inherited.split_once("AFKCONVOY_PROXY\0").map_or("", |(_, values)| values);
        let (lower, upper) = values.split_once('\0').unwrap_or(("", ""));
        serde_json::Value::Array(Harness::ALL.into_iter().map(|harness| {
            let addition = afkconvoy_core::proxy::addition(&self.bypass, harness);
            let (lower, upper) = afkconvoy_core::proxy::effective(lower, upper, addition);
            serde_json::json!({"harness": harness, "addition": addition, "no_proxy": lower, "NO_PROXY": upper})
        }).collect())
    }

    pub fn default(&self) -> Harness {
        crate::kit::load()
            .ok()
            .and_then(|kit| kit.first())
            .map_or(self.default, |first| first.harness)
    }

    pub fn bound(&self) -> bool {
        self.bound
    }

    pub fn starts(&self) -> bool {
        self.starts
    }

    /// The host kit's command, with its first word resolved in the login shell.
    /// A missing command is discovered from the harness's stock command in
    /// the worker's login shell unless the operator removed that harness.
    /// Read-only commands never write the kit.
    pub fn of(&self, harness: Harness) -> Result<Option<Proposed>> {
        let kit = crate::kit::load()?;
        if self.starts && kit.is_removed(harness) {
            return Ok(None);
        }
        let explicit = kit.command(harness).filter(|command| !command.trim().is_empty());
        let command = explicit.unwrap_or(harness.stock());
        if let Some(proposed) = self
            .proposed
            .borrow()
            .iter()
            .find(|proposed| proposed.harness == harness && proposed.command == command)
        {
            return Ok(Some(proposed.clone()));
        }
        let mut proposed = self.propose(harness, command.to_string());
        if explicit.is_none() && self.starts {
            self.validate(&proposed)?;
            let merged = crate::kit::discover(harness, command)?;
            if merged != command {
                proposed = self.propose(harness, merged);
            } else {
                self.warned.borrow_mut().push(format!("discovered {} worker launch command from its harness default in the login shell: {command}", harness.key()));
            }
        }
        if let Ok(command) = proposed.checked()
            && let Some(warning) = command.warning().filter(|_| self.starts)
        {
            self.warned.borrow_mut().push(warning.to_string());
        }
        let mut cached = self.proposed.borrow_mut();
        cached.retain(|old| old.harness != harness);
        cached.push(proposed.clone());
        Ok(Some(proposed))
    }

    /// Identity of the exact base command resolved for this launch. No
    /// credentials or command text are copied into worker health records.
    pub(crate) fn command_key(&self, harness: Harness) -> Option<String> {
        self.proposed
            .borrow()
            .iter()
            .find(|p| p.harness == harness)
            .map(|p| crate::health::command_key(&p.command))
    }

    fn propose(&self, harness: Harness, command: String) -> Proposed {
        let resolution = first_word(&command)
            .and_then(|word| {
                login_shell(&self.dir, &format!("type -- '{}'", word.replace('\'', "'\\''")))
            })
            .filter(|answer| !answer.trim().is_empty());
        Proposed {
            harness,
            command,
            resolution,
            dir: self.dir.clone(),
            flags_answer: Arc::new(OnceLock::new()),
        }
    }

    fn validate(&self, proposed: &Proposed) -> Result<()> {
        proposed.checked().map_err(|why| {
            anyhow!(
                "{why}; install {} or set its command in the host kit",
                proposed.harness.program()
            )
        })?;
        if self.adds_flags(proposed.harness) {
            proposed
                .harness
                .takes(proposed.as_str(), proposed.flags().as_deref())
                .map_err(|why| anyhow!(why))?;
        }
        if let Some((tool, found)) = crate::tools::harness(proposed)
            && let Some(why) = afkconvoy_core::tools::refusal(&[(tool, found)], None, None)
        {
            bail!("{why}");
        }
        Ok(())
    }

    /// The command every worker of `harness` is started with, checked to
    /// resolve in a login shell: no worker is started on one that does not.
    pub fn command(&self, harness: Harness) -> Result<LaunchCommand> {
        let proposed = self.of(harness)?;
        let proposed = proposed.ok_or_else(|| {
            if crate::kit::load().is_ok_and(|kit| kit.is_removed(harness)) {
                anyhow!(
                    "the harness `{}` was removed by the operator; restore it with `afkconvoy switch --harness {}`",
                    harness.key(),
                    harness.key()
                )
            } else {
                See::Troubleshooting
                    .refusal("no worker launch command in the host kit: run `afkconvoy kit`")
            }
        })?;
        self.validate(&proposed)?;
        let checked = proposed.checked();
        checked.map_err(|why| See::Troubleshooting.refusal(self.said(&why, harness, &[])))
    }

    /// Whether this launch adds harness flags: every worker in a role, and
    /// outside one wherever its worktree trust or asking guard needs flags.
    pub fn adds_flags(&self, harness: Harness) -> bool {
        self.bound || harness.trust_is_flags() || harness == Harness::Dsh
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

    /// What the commands resolved since it was last asked warn of: a start
    /// that starts workers says them where its other warnings go.
    pub fn warned(&self) -> Vec<String> {
        self.warned.take()
    }

    /// Fingerprint of the kit's launch commands, so any edit invalidates a
    /// standing probe. A command that starts no worker hashes the commands
    /// the kit holds and asks the login shell nothing: resolving a word
    /// does not change that text, and a doctor trusts a standing probe
    /// without starting a harness. A command that starts workers resolves,
    /// and may discover a missing command, before it hashes what it checked.
    pub fn key(&self) -> Result<Option<u64>> {
        if !self.starts {
            return Ok(Some(key(&self.held_commands()?)));
        }
        let Some(default) = self.of(self.default())? else { return Ok(None) };
        let mut text = default.as_str().to_string();
        for (harness, command) in crate::kit::load()?.commands() {
            text.push_str(&format!("\n{}={command}", harness.key()));
        }
        Ok(Some(key(&text)))
    }

    /// The launch commands a fingerprint is of, as the kit holds them: the
    /// default harness's command, then each held command. The same text
    /// `of` hashes once a missing command has been discovered.
    fn held_commands(&self) -> Result<String> {
        let kit = crate::kit::load()?;
        let harness = self.default();
        let command = kit
            .command(harness)
            .filter(|command| !command.trim().is_empty())
            .unwrap_or_else(|| harness.stock());
        let mut text = command.to_string();
        for (harness, command) in kit.commands() {
            text.push_str(&format!("\n{}={command}", harness.key()));
        }
        Ok(text)
    }

    /// First words alone for the run log; commands remain in the host kit.
    pub fn logged(&self) -> (String, BTreeMap<String, String>) {
        let word = |command: &str| first_word(command).unwrap_or_default().to_string();
        let default = self.of(self.default()).ok().flatten().map(|command| word(command.as_str()));
        let kit = crate::kit::load();
        let others = kit
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

/// Whether `harness`'s program is found in the login shell a worker gets
/// (`ask_host`) — as `type` finds it there, with no terminal opened. An
/// error says why it is taken as not installed: it was not found, the shell
/// did not answer in time, or the shell could not be started.
pub fn found(harness: Harness) -> Result<(), String> {
    let question = format!("type -- '{}'", harness.program());
    match ask_host(&question, SHELL_TIMEOUT) {
        Ok(Some((true, _))) => Ok(()),
        Ok(Some((false, _))) => Err(format!("`{question}` found nothing")),
        Ok(None) => Err(format!("`{question}` did not answer in time")),
        Err(error) => Err(format!("{error:#}: {question}")),
    }
}

/// The models `harness` knows on this host, as it says them in the login
/// shell a worker gets (`ask_host`) — no terminal opened — when its worker
/// launch command `command` is asked (`Harness::models_question`), with what
/// this host keeps for it: those its own login runs. An error says why it
/// listed none — the question failed, timed out, or answered with no model —
/// and the question.
pub fn models(harness: Harness, command: &str) -> Result<Vec<crate::kit::Listed>, String> {
    // The logins are in the directory kept with the host user's skills and
    // rules as they are in the one kept without.
    let kept = crate::kept::keep(harness, false).map_err(|error| format!("{error:#}"))?;
    let question = harness.models_question(command, &kept.display().to_string());
    let why = match ask_host(&question, SHELL_TIMEOUT) {
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

/// What a worker-health recovery probe came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// It answered, in this many seconds, as this model where the answer says.
    Answered(f64, Option<String>),
    /// It said nothing within the time limit, or failed.
    Silent,
    /// It was never asked, for this reason.
    Unasked(String),
}

/// Sends `model` of `harness` the one-line probe (`Harness::probe_question`)
/// through the worker launch command `command`, with what this host keeps for
/// it, in the login shell a worker gets (`ask_host`) — no terminal opened →
/// whether it answered within the time limit, how fast, and which model
/// answered (`answered_as`); why it could not be asked, when the shell could
/// not be started. An empty answer, a non-zero exit, or a timeout is silent.
/// Kit writes never call this: only worker health and recovery do.
pub fn probe(harness: Harness, command: &str, model: Option<&str>) -> Answer {
    let kept = match crate::kept::keep(harness, false) {
        Ok(kept) => kept,
        Err(error) => return Answer::Unasked(format!("{error:#}")),
    };
    let question =
        harness.probe_question(command, &kept.display().to_string(), model, PROBE_PROMPT);
    let started = Instant::now();
    match ask_host(&question, PROBE_TIMEOUT) {
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

/// Runs `script` in the user's interactive login shell, started in `dir` →
/// what it printed. `None` when it failed, timed out, or exited non-zero:
/// `type` prints "not found" and exits 1, so its output alone proves nothing.
fn login_shell(dir: &Path, script: &str) -> Option<String> {
    let (succeeded, printed) = ask_login_shell(dir, script)?;
    succeeded.then_some(printed)
}

/// Runs `script` in the user's interactive login shell — where aliases and
/// rc-defined functions exist, and what orca gives a worker: it inherits
/// nothing from the shell the fleet was started in — started in `dir`, a
/// checkout of the repo it is asked for, as a worker's is started in a
/// worktree of it → whether it exited zero, and what it printed. `None` when
/// it could not run or timed out.
pub fn ask_login_shell(dir: &Path, script: &str) -> Option<(bool, String)> {
    ANSWERED.read(&asked(dir, script), || ask_login_shell_within(dir, script, SHELL_TIMEOUT))
}

/// The login shell's answer to one question in one directory, asked once in
/// this look (docs/adr/0043). A tick asked `type` and the flags question of
/// its default harness twice, once for the fleet's view of what it can start
/// and once for the agent that starts it: a login shell each. In a sample of
/// the slow gate's tests, 787 of 2,186 questions were asked again in the
/// process that had asked them (sunfmin/control#319). A harness installed or
/// logged in meanwhile is seen at the next look.
pub(crate) static ANSWERED: Reading<Option<(bool, String)>> = Reading::per_look("login shell");

/// The key `ANSWERED` keeps the answer to `script`, asked in `dir`, by.
fn asked(dir: &Path, script: &str) -> String {
    format!("{}\0{script}", dir.display())
}

/// `ask_login_shell`, given `timeout` to answer.
pub(crate) fn ask_login_shell_within(
    dir: &Path,
    script: &str,
    timeout: Duration,
) -> Option<(bool, String)> {
    let (succeeded, printed) = ask_shell(dir, script, timeout, None)?;
    Some((succeeded, String::from_utf8_lossy(&printed).into_owned()))
}

/// The user's login shell: `$SHELL`, or zsh, macOS's, where it is unset.
fn login_shell_program() -> String {
    let shell = std::env::var("SHELL").ok().filter(|shell| !shell.is_empty());
    shell.unwrap_or_else(|| "/bin/zsh".to_string())
}

/// The home directory a fresh login starts in, or `/` where this process has
/// none. A question is of the host, never of the directory a command was run
/// from.
fn login_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// What a fresh login brings in, and nothing this process set for itself.
fn fresh_login_env() -> Vec<(OsString, OsString)> {
    let given = LOGIN_GIVES
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(*name), value)));
    given.chain([(OsString::from("PATH"), OsString::from(LOGIN_PATH))]).collect()
}

/// Asks `script` of the login shell a worker gets: `$SHELL -lic` (`/bin/zsh`
/// where `SHELL` is unset), started in the home directory from the
/// environment a fresh login brings in (`fresh_login_env`), never this
/// process's. No terminal is opened. stdout and stderr are one stream. The
/// question's own exit status and what it printed are between its marks
/// (`question::answer`); rc noise before the first mark is not an answer.
/// `Ok(None)` when it did not end within `timeout`. An error when the shell
/// could not be started — that is not a silent answer.
pub(crate) fn ask_host(script: &str, timeout: Duration) -> Result<Option<(bool, String)>> {
    let shell = login_shell_program();
    let typed = afkconvoy_core::question::typed(script);
    let env = fresh_login_env();
    match run_login_shell(&login_home(), &shell, &typed, timeout, &env) {
        Err(error) => bail!("the login shell `{shell}` could not be started: {error}"),
        Ok(None) => Ok(None),
        Ok(Some(printed)) => {
            let text = String::from_utf8_lossy(&printed);
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            Ok(afkconvoy_core::question::answer(&lines))
        }
    }
}

/// Runs `script` as `shell -lic` in `dir`, with `env` alone, stdout and
/// stderr one stream → the bytes it printed. `Ok(None)` when it did not end
/// within `timeout`; its process group is killed. `Err` when it could not
/// be started.
fn run_login_shell(
    dir: &Path,
    shell: &str,
    script: &str,
    timeout: Duration,
    env: &[(OsString, OsString)],
) -> std::io::Result<Option<Vec<u8>>> {
    let (stdout, stderr, read) = crate::process::merged_stdio()?;
    let mut command = Command::new(shell);
    command
        .args(["-lic", script])
        .current_dir(dir)
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);
    let mut child = crate::process::spawn(&mut command)?;
    // Command keeps the write ends (`Stdio::Fd`). They have to close here,
    // or the reader never sees the end of what the child printed.
    drop(command);
    let printed = std::fs::File::from(read);
    let exited = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&exited);
    let reader = std::thread::spawn(move || {
        crate::process::read_bounded(printed, &flag).unwrap_or_default()
    });
    let started = Instant::now();
    let ended = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if started.elapsed() >= timeout => {
                stop_group(&mut child);
                break false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                stop_group(&mut child);
                let _ = finish_reader(&exited, reader);
                return Err(error);
            }
        }
    };
    let bytes = finish_reader(&exited, reader);
    Ok(ended.then_some(bytes))
}

/// Stops `reader` once the child is gone. A grandchild still holding the
/// pipe cannot keep the question waiting past the drain bound.
fn finish_reader(exited: &AtomicBool, reader: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    exited.store(true, Ordering::Release);
    reader.join().unwrap_or_default()
}

/// Kills `child` and every process in its group. A timed-out question must
/// not leave the harness it started.
fn stop_group(child: &mut Child) {
    let pid = child.id();
    if pid > 0 {
        // SAFETY: a negative pid names the process group, which this child
        // leads (`process_group(0)`). A group that is already gone is fine.
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `script` in the user's interactive login shell, started in `dir`
/// with `env` alone where it is given, else with this process's
/// environment → whether it exited zero, and the bytes it printed. `None`
/// when it could not run or did not end within `timeout`.
fn ask_shell(
    dir: &Path,
    script: &str,
    timeout: Duration,
    env: Option<&[(OsString, OsString)]>,
) -> Option<(bool, Vec<u8>)> {
    let mut shell = Command::new(login_shell_program());
    shell.args(["-lic", script]).current_dir(dir);
    if let Some(env) = env {
        shell.env_clear().envs(env.iter().map(|(name, value)| (name, value)));
    }
    let mut child = crate::process::spawn(
        shell.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()),
    )
    .ok()?;
    let printed = child.stdout.take()?;
    let exited = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&exited);
    let reader = std::thread::spawn(move || {
        crate::process::read_bounded(printed, &flag).unwrap_or_default()
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = finish_reader(&exited, reader);
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    Some((status.success(), finish_reader(&exited, reader)))
}

/// What login gives every shell of this user, whatever session started it:
/// the part of a session's environment a fresh login shell starts from.
const LOGIN_GIVES: [&str; 7] =
    ["HOME", "USER", "LOGNAME", "SHELL", "SSH_AUTH_SOCK", "LANG", "__CF_USER_TEXT_ENCODING"];

/// The PATH a fresh login shell starts from, before its rc files add to it.
const LOGIN_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// What a shell keeps of itself in its environment: no process it starts
/// is given them as the shell holds them.
const SHELL_KEEPS: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

/// The word the login shell prints before its environment: rc files may
/// print anything first.
const ENV_FRAME: &str = "AFKCONVOY_ENV";

/// The environment of a fresh login shell of this user: `$SHELL -lic`
/// (/bin/zsh where SHELL is unset), started in the home directory from
/// `LOGIN_GIVES` and `LOGIN_PATH`, less what the shell keeps for itself
/// (`SHELL_KEEPS`). Nothing a session set for itself reaches it: no
/// CLAUDE_CODE_*, ORCA_*, AFKCONVOY_* or TMPDIR. An error where the shell did
/// not answer within `SHELL_TIMEOUT`.
pub fn login_environment() -> Result<Vec<(OsString, OsString)>> {
    let given = fresh_login_env();
    let home = login_home();
    let question = format!("printf '{ENV_FRAME}\\000'; /usr/bin/env -0");
    let shell = login_shell_program();
    let fix = "fix what your shell's rc files do without a terminal, then run again";
    let Some((_, printed)) = ask_shell(&home, &question, SHELL_TIMEOUT, Some(&given)) else {
        bail!("`{shell} -lic` did not answer within {}s: {fix}", SHELL_TIMEOUT.as_secs());
    };
    login_answered(&printed)
        .ok_or_else(|| anyhow!("`{shell} -lic` printed no environment, as `env -0` does: {fix}"))
}

/// The environment a login shell printed after `ENV_FRAME`, one `NAME=value`
/// a NUL, less what the shell keeps for itself; `None` where it printed no
/// frame.
fn login_answered(printed: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    use std::os::unix::ffi::OsStrExt;
    let frame = [ENV_FRAME.as_bytes(), b"\0"].concat();
    let at = printed.windows(frame.len()).position(|window| window == frame)?;
    let pairs = printed[at + frame.len()..].split(|byte| *byte == 0);
    let pairs = pairs.filter_map(|pair| {
        let equals = pair.iter().position(|byte| *byte == b'=').filter(|at| *at > 0)?;
        let (name, value) = (&pair[..equals], &pair[equals + 1..]);
        let name = std::ffi::OsStr::from_bytes(name);
        let kept = SHELL_KEEPS.iter().any(|kept| name == *kept);
        (!kept).then(|| (name.to_os_string(), std::ffi::OsStr::from_bytes(value).to_os_string()))
    });
    Some(pairs.collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_shells_environment_is_read_after_its_frame_less_what_the_shell_keeps() {
        let printed = b"a line an rc file printed\nAFKCONVOY_ENV=no\0\
            AFKCONVOY_ENV\0PWD=/Users/me\0PATH=/opt/bin:/usr/bin\0SHLVL=1\0_=/usr/bin/env\0\
            NOTE=two\nlines=and an equals\0OLDPWD=/\0EMPTY=\0";
        let read = login_answered(printed).unwrap();
        let read: Vec<(&str, &str)> = read
            .iter()
            .map(|(name, value)| (name.to_str().unwrap(), value.to_str().unwrap()))
            .collect();
        assert_eq!(
            read,
            [("PATH", "/opt/bin:/usr/bin"), ("NOTE", "two\nlines=and an equals"), ("EMPTY", "")]
        );
        assert_eq!(login_answered(b"PATH=/usr/bin\0"), None, "no frame, no environment");
    }

    /// A shell that runs `-lic` by executing the script, with none of a login's
    /// rc files: the question's marks are the script's own.
    fn shell_that_runs(dir: &Path) -> PathBuf {
        let shell = dir.join("shell");
        std::fs::write(&shell, "#!/bin/sh\nexec /bin/sh -c \"$2\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        shell
    }

    #[test]
    fn a_host_question_joins_both_streams_and_inherits_nothing_of_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell_that_runs(dir.path());
        let script = "printf 'out\\n'; printf 'err\\n' >&2; printf 'only=%s\\n' \"$ONLY\"; \
                      printf 'cargo=%s\\n' \"$CARGO_MANIFEST_DIR\"; exit 3";
        let env = [(OsString::from("ONLY"), OsString::from("1"))];
        let answer = run_login_shell(
            dir.path(),
            shell.to_str().unwrap(),
            &afkconvoy_core::question::typed(script),
            Duration::from_secs(5),
            &env,
        )
        .unwrap()
        .unwrap();
        let text = String::from_utf8_lossy(&answer);
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        assert_eq!(
            afkconvoy_core::question::answer(&lines),
            Some((false, "out\nerr\nonly=1\ncargo=".to_string())),
            "{text}"
        );
    }

    #[test]
    fn a_host_question_that_prints_nothing_is_an_empty_answer() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell_that_runs(dir.path());
        let answer = run_login_shell(
            dir.path(),
            shell.to_str().unwrap(),
            &afkconvoy_core::question::typed(":"),
            Duration::from_secs(5),
            &[],
        )
        .unwrap()
        .unwrap();
        let lines: Vec<String> =
            String::from_utf8_lossy(&answer).lines().map(str::to_string).collect();
        assert_eq!(afkconvoy_core::question::answer(&lines), Some((true, String::new())));
    }

    #[test]
    fn a_host_question_that_does_not_end_is_none_and_its_group_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell_that_runs(dir.path());
        let started = Instant::now();
        let env = [(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))];
        let answer = run_login_shell(
            dir.path(),
            shell.to_str().unwrap(),
            &afkconvoy_core::question::typed("sleep 30"),
            Duration::from_millis(300),
            &env,
        )
        .unwrap();
        assert_eq!(answer, None);
        assert!(started.elapsed() < Duration::from_secs(5), "the sleep was left running");
    }

    #[test]
    fn a_grandchild_holding_stdout_does_not_keep_the_login_question_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell_that_runs(dir.path());
        let pidfile = dir.path().join("holder");
        let script = format!("sleep 30 & echo $! > '{}'; printf done", pidfile.display());
        let started = Instant::now();
        let answer = run_login_shell(
            dir.path(),
            shell.to_str().unwrap(),
            &afkconvoy_core::question::typed(&script),
            Duration::from_secs(5),
            &[(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
        )
        .unwrap()
        .unwrap();
        if let Ok(pid) = std::fs::read_to_string(&pidfile) {
            let _ = Command::new("kill").arg(pid.trim()).status();
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "the question waited {:?} after the shell exited",
            started.elapsed()
        );
        let text = String::from_utf8_lossy(&answer);
        assert!(text.contains("done"), "{text}");
    }

    #[test]
    fn a_doctor_budget_starts_a_program_once() {
        let _budget = budget_once();
        let mut starts = 0;
        let first = once_per_program("unit-probe", "login", || {
            starts += 1;
            (true, "ok".to_string())
        });
        let again = once_per_program("unit-probe", "login", || {
            starts += 1;
            (false, "again".to_string())
        });
        let other = once_per_program("unit-probe", "flags", || {
            starts += 1;
            (true, "flags".to_string())
        });
        assert!(matches!(first, Once::Ran(true, ref printed) if printed == "ok"));
        assert!(matches!(again, Once::Ran(true, ref printed) if printed == "ok"));
        assert!(matches!(other, Once::Already));
        assert_eq!(starts, 1);
    }

    #[test]
    fn a_login_shell_question_is_asked_once_a_look() {
        let dir = Path::new("/no/such/checkout");
        let found = Some((true, "agent is /usr/local/bin/agent".to_string()));
        let look = Arc::new(crate::look::Look::new());
        look.seed(&ANSWERED, &asked(dir, "type -- 'agent'"), found.clone());
        look.enter(|| {
            assert_eq!(ask_login_shell(dir, "type -- 'agent'"), found, "answered from the look");
            // No shell starts in a directory that is not there.
            assert_eq!(ask_login_shell(dir, "agent --help"), None, "another question is asked");
            assert_eq!(look.held(&ANSWERED, &asked(dir, "agent --help")), Some(None));
            crate::look::begin();
        });
        assert_eq!(
            look.held(&ANSWERED, &asked(dir, "type -- 'agent'")),
            None,
            "the next look asks"
        );
    }

    #[test]
    fn a_host_question_whose_shell_cannot_start_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let error =
            run_login_shell(dir.path(), "/no/such/login-shell", ":", Duration::from_secs(1), &[]);
        assert!(error.is_err(), "{error:?}");
    }
}

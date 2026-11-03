//! orca, as a subprocess: it owns every worktree, branch and terminal a worker
//! lives in. A removal it already recorded as failed is finished here once the
//! directory can go (`finish_failed_removals`): listing the worktrees again is
//! what makes orca drop that record. Nothing here creates a worktree any other
//! way.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use afkconvoy_core::casting::CASTER_WORKTREE;
use afkconvoy_core::config::RepoName;
use afkconvoy_core::cut;
use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::harness::{Harness, Keystroke};
use afkconvoy_core::launch::{TrustGap, Waited};
use afkconvoy_core::legacy::TRAIN_WORKTREE;
use afkconvoy_core::line::niced;
use afkconvoy_core::stopped::Nudged;
use afkconvoy_core::tools::{Found, add_checkout};
use afkconvoy_core::tracker::Tracker;
use afkconvoy_core::worker::{WorkerState, is_worktree_named, short_branch, worker_state};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::{REMOTE, Repo};
use crate::look::Reading;
use crate::worker_tmp;

/// The worker's instructions, kept in the worktree's own git dir: never staged
/// by the worker's `git add -A`, gone when the worktree is.
pub(crate) const BRIEF_FILE: &str = "convoy-worker-prompt.md";
/// When the worker here was nudged (epoch seconds), beside the brief.
const NUDGE_FILE: &str = "convoy-nudged";
/// Each operator's note that reached the worker's model (`afkconvoy tell`).
pub(crate) const TOLD_FILE: &str = "convoy-told";
/// The one since this worker has waited on its own background work (epoch
/// seconds), beside the brief. The tick line, worker health and a later tick
/// all name this file. It is gone once the turn is open, or the worker is
/// seen waiting on none.
const BACKGROUND_FILE: &str = "convoy-background";
/// When the worker here was started (epoch seconds), beside the brief.
const START_FILE: &str = "convoy-started";
/// Where a stage that writes nothing started here (`guard::WritesNothing`).
const WRITES_NOTHING_FILE: &str = "convoy-writes-nothing";
/// The comment of the `blocked` verdict a worker was last started again over.
const ANSWERED_FILE: &str = "convoy-answered";
/// What the last turn asked for here answered, or the process asking for one
/// now (`turn_answer`).
pub(crate) const TURN_FILE: &str = "convoy-turn.json";
/// What the turns the tick asked for here printed, one after another.
pub(crate) const TURN_LOG_FILE: &str = "convoy-turn.log";
/// What a bound repo's worker here was started under (`Under`).
const UNDER_FILE: &str = "convoy-started-under";
/// The harness the worker here was last started on, as its key.
const HARNESS_FILE: &str = "convoy-harness";
/// The tracker of the fleet that started the worker here: the repo it
/// names, or nothing where it is the checkout's own (`keep_tracker`).
pub(crate) const TRACKER_FILE: &str = "convoy-tracker";
/// What was taken down as the worker here turned hot (`evidence`).
const EVIDENCE_DIR: &str = "convoy-hot";
/// What the cut in a checkout is, and what it did so far (`cut::Record`).
const CUT_FILE: &str = "convoy-cut.json";
/// What each step of the cut in a checkout printed, one after another.
const CUT_LOG_FILE: &str = "convoy-cut.log";
/// More worktrees than one machine runs workers in.
const PS_LIMIT: &str = "1000";
/// What orca says holds up a worker asking whether its folder is trusted.
const TRUST_PROMPT: &str = "agent-trust-workspace";
/// How long each accepted line is observed for a turn start. Orca observes
/// the same request throughout this bound and never resends it.
const SUBMIT_SECONDS: u64 = 30;
/// How long a brief handed to a worker that just started waits for its
/// prompt to settle and its harness to leave a startup working episode.
/// Any other line checks the input within a second.
const BRIEF_SETTLE_SECONDS: u64 = 20;
/// How often a quiet screen's harness state is read during that bound.
const INPUT_POLL: Duration = Duration::from_millis(200);

/// Monotonic time for continuation termination, supplied by the integration
/// executable when the terminal is a stand-in. Process discovery, signals,
/// runtime refresh and the decision to permit a replacement remain real.
#[doc(hidden)]
pub trait TerminationClock {
    fn now(&self) -> Duration;
    fn sleep(&self, interval: Duration);
}

thread_local! {
    static TERMINATION_CLOCK: RefCell<Option<Rc<dyn TerminationClock>>> = RefCell::new(None);
}

/// Runs an entry point with a clock on this thread, restoring its predecessor
/// even on panic. The shipped executable installs none and exposes no switch.
#[doc(hidden)]
pub fn with_termination_clock<T>(clock: Rc<dyn TerminationClock>, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<Rc<dyn TerminationClock>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TERMINATION_CLOCK.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(TERMINATION_CLOCK.with(|slot| slot.replace(Some(clock))));
    run()
}

struct TerminationTime {
    began: Instant,
    clock: Option<Rc<dyn TerminationClock>>,
}

impl TerminationTime {
    fn new() -> Self {
        Self { began: Instant::now(), clock: TERMINATION_CLOCK.with(|slot| slot.borrow().clone()) }
    }

    fn now(&self) -> Duration {
        self.clock.as_ref().map_or_else(|| self.began.elapsed(), |clock| clock.now())
    }

    fn sleep(&self, interval: Duration) {
        match &self.clock {
            Some(clock) => clock.sleep(interval),
            None => std::thread::sleep(interval),
        }
    }
}

/// What ending a worker came to: its terminals, process tree and agent
/// confirmed gone, or still there when the bounded effort ran out, saying so.
#[must_use]
pub(crate) enum Ended {
    Confirmed,
    Unconfirmed(String),
}

/// A process left after terminal close, and the bounded effort to end it.
/// The command is read without its environment: the process-tree listing
/// includes environment variables, which must never be copied to the log.
struct Ending {
    description: String,
    since: Duration,
    forced: bool,
    sent: bool,
    logged: bool,
    error: Option<String>,
}

impl Ending {
    fn observe(proc: &crate::hot::Proc, clock: &TerminationTime) -> Self {
        let pid = proc.pid.to_string();
        let metadata = crate::process::output(Command::new("ps").args([
            "-ww",
            "-p",
            &pid,
            "-o",
            "etime=,command=",
        ]))
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
        let description =
            match metadata.as_deref().and_then(|row| row.split_once(char::is_whitespace)) {
                Some((ran, command)) => format!("pid {pid}, command {}, ran {ran}", command.trim()),
                None => format!(
                    "pid {pid}, command {}, elapsed time unavailable",
                    proc.line.split_whitespace().next().unwrap_or("unavailable")
                ),
            };
        Self {
            description,
            since: clock.now(),
            forced: false,
            sent: false,
            logged: false,
            error: None,
        }
    }

    fn signal(&mut self, pid: u32, forced: bool) {
        self.forced = forced;
        let signal = if forced { "-KILL" } else { "-TERM" };
        let sent = crate::process::output(Command::new("kill").args([signal, &pid.to_string()]));
        match sent {
            Ok(out) if out.status.success() => self.sent = true,
            Ok(out) => {
                self.error =
                    Some(format!("{signal}: {}", String::from_utf8_lossy(&out.stderr).trim()))
            }
            Err(error) => self.error = Some(format!("{signal}: {error}")),
        }
    }

    fn log_ended(&mut self, worktree: &Path, command: &str) {
        if !self.sent || self.logged {
            return;
        }
        self.logged = true;
        let signal = if self.forced { "SIGKILL" } else { "SIGTERM" };
        let entry = crate::log::Entry {
            at: crate::clock(),
            instance: String::new(),
            host: crate::this_host(),
            what: crate::log::What::Command {
                command: command.to_string(),
                on: Some(worktree.display().to_string()),
                by: "fleet".to_string(),
                outcome: format!("ended old worker {}, after {signal}", self.description),
                failed: false,
            },
        };
        // Logging is soft, as for every command that acts.
        if let Err(error) = crate::log::Log::of(&Repo::new(worktree.to_path_buf()))
            .and_then(|log| log.append(&entry))
        {
            eprintln!("warning: worker termination left no line in the tick log: {error:#}");
        }
    }
}

thread_local! {
    /// One read-only view per tick, dropped even when the tick fails. Each
    /// fleet runs on its own thread, so another checkout never shares it.
    static OBSERVATION: RefCell<Option<Result<Value, String>>> = const { RefCell::new(None) };
}

/// Holds a host-wide runtime observation for one tick or read-only command.
/// Errors are shared too: an unreachable Orca never becomes an empty host.
pub(crate) struct Observation(bool);

impl Observation {
    pub(crate) fn begin() -> Self {
        if OBSERVATION.with(|slot| slot.borrow().is_some()) {
            return Self(false);
        }
        let observed = runtime_live().map_err(|e| format!("{e:#}"));
        OBSERVATION.with(|slot| slot.replace(Some(observed)));
        Self(true)
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        if self.0 {
            OBSERVATION.with(|slot| slot.borrow_mut().take());
        }
    }
}

fn runtime_live() -> Result<Value> {
    let ps = orca(&["worktree", "ps", "--limit", PS_LIMIT])?;
    if ps["truncated"] == true {
        bail!("orca worktree ps was cut short at {PS_LIMIT} rows");
    }
    ps["worktrees"].as_array().context("orca worktree ps returned no rows")?;
    Ok(ps)
}

fn runtime() -> Result<Value> {
    match OBSERVATION.with(|slot| slot.borrow().clone()) {
        Some(observed) => observed.map_err(|e| anyhow!(e)),
        None => runtime_live(),
    }
}

fn runtime_row(ps: &Value, path: &Path) -> Value {
    ps["worktrees"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["path"].as_str().is_some_and(|p| same_dir(Path::new(p), path)))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Whether orca's runtime row still shows this agent running. Orca keeps an
/// ended session's row as `done` with no live pty, across restarts and for a
/// worktree cut again at the same path: that one is over.
fn agent_running(agent: &Value) -> bool {
    match agent["state"].as_str() {
        Some("exited" | "stopped" | "closed" | "gone") => false,
        Some("done") => agent["hasLivePty"] == true,
        _ => true,
    }
}

/// The top-level runtime observation belonging to exactly this terminal.
fn pane_agent(row: &Value, terminal: &Value) -> Value {
    let pane = format!(
        "{}:{}",
        terminal["tabId"].as_str().unwrap_or(""),
        terminal["leafId"].as_str().unwrap_or("")
    );
    row["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|agent| agent["paneKey"] == pane && agent["parentPaneKey"].is_null())
        .cloned()
        .unwrap_or(Value::Null)
}

/// The newest top-level agent speaks for a worktree's harness and outcome.
/// Child panes cannot change either.
fn main_agent(row: &Value) -> Value {
    row["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["state"].is_string() && a["parentPaneKey"].is_null())
        .max_by_key(|a| a["stateStartedAt"].as_u64().unwrap_or(0))
        .cloned()
        .unwrap_or(Value::Null)
}

pub(crate) fn agent_harness(agent: &Value) -> Option<Harness> {
    match agent["agentType"].as_str()? {
        "claude" => Some(Harness::ClaudeCode),
        key => Harness::read(key),
    }
}

/// One `orca … --json` call → its `result` object. An error when orca cannot
/// be run, exits non-zero, or answers `ok: false` — never an empty answer.
pub fn orca(args: &[&str]) -> Result<Value> {
    let asked = Asked::of(args);
    // A write forgets each reading that names it, whatever it answered.
    crate::look::wrote(&[&["orca"], args].concat());
    let asked = asked?;
    if !asked.exited_ok || asked.answer["ok"] != true {
        return Err(asked.failure());
    }
    Ok(asked.answer["result"].clone())
}

/// One `orca … --json` call, as it came back.
struct Asked {
    what: String,
    exited_ok: bool,
    answer: Value,
    stdout: String,
    stderr: String,
}

/// Reads Orca's JSON, replacing each unpaired UTF-16 surrogate escape with
/// U+FFFD. Orca can split a pair when it cuts terminal text. Whole pairs and
/// literal backslashes keep their meaning; all other JSON errors still fail.
fn read_answer(bytes: &[u8]) -> serde_json::Result<Value> {
    serde_json::from_slice(bytes).or_else(|_| {
        let mut repaired = bytes.to_vec();
        let mut quoted = false;
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'"' => quoted = !quoted,
                b'\\' if quoted => {
                    if let Some(unit) = unicode_escape(&bytes[at..]) {
                        if (0xd800..=0xdbff).contains(&unit)
                            && unicode_escape(&bytes[at + 6..])
                                .is_some_and(|next| (0xdc00..=0xdfff).contains(&next))
                        {
                            at += 12;
                            continue;
                        }
                        if (0xd800..=0xdfff).contains(&unit) {
                            repaired[at + 2..at + 6].copy_from_slice(b"fffd");
                        }
                        at += 6;
                    } else {
                        // An escaped quote cannot end a string, nor can an
                        // escaped backslash begin a Unicode escape.
                        at += 2;
                    }
                    continue;
                }
                _ => {}
            }
            at += 1;
        }
        serde_json::from_slice(&repaired)
    })
}

/// One complete JSON Unicode escape at the start of `bytes`, if present.
fn unicode_escape(bytes: &[u8]) -> Option<u16> {
    let digits = bytes.strip_prefix(br"\u")?.get(..4)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

/// Orca refused a prompt because the harness cannot accept it. Keep this
/// distinct from a terminal or transport failure, including its full reason.
#[derive(Debug)]
pub struct PromptBlocked(pub String);
impl std::fmt::Display for PromptBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "orca terminal send failed: agent_prompt_blocked: {}", self.0)
    }
}
impl std::error::Error for PromptBlocked {}

/// The idle check timed out before any line was sent. Readiness at start
/// still fails on this; a ready worker can be handed its brief later.
#[derive(Debug)]
pub(crate) struct IdleTimeout;
impl std::fmt::Display for IdleTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "orca terminal wait failed: timeout")
    }
}
impl std::error::Error for IdleTimeout {}

impl Asked {
    fn of(args: &[&str]) -> Result<Asked> {
        let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let out = crate::process::output(
            Command::new("orca").args(args).arg("--json").current_dir(asked_from()),
        )
        .with_context(|| format!("orca {what} could not run"))?;
        Ok(Asked {
            what,
            exited_ok: out.status.success(),
            answer: read_answer(&out.stdout).unwrap_or(Value::Null),
            stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }

    fn failure(&self) -> anyhow::Error {
        if self.what == "terminal send" && self.answer["error"]["code"] == "agent_prompt_blocked" {
            let error = &self.answer["error"];
            let reason = error["details"]["reason"]
                .as_str()
                .or_else(|| error["message"].as_str())
                .unwrap_or("agent_prompt_blocked");
            return PromptBlocked(reason.to_string()).into();
        }
        let said = match self.answer["error"]["code"].as_str() {
            Some(code) => code,
            None if self.stderr.is_empty() => &self.stdout,
            None => &self.stderr,
        };
        anyhow!("orca {} failed: {said}", self.what)
    }

    /// Preserve the transport evidence too: an answered wait may exit nonzero,
    /// and an unavailable screen must not become an empty successful read.
    fn evidence(&self) -> String {
        startup_redacted(&format!(
            "exit success: {}\nstdout:\n{}\nstderr:\n{}",
            self.exited_ok, self.stdout, self.stderr
        ))
    }
}

/// A startup screen or command may echo a provider credential. Compare only
/// secret-valued environment entries; never include the environment itself.
fn startup_redacted(text: &str) -> String {
    let mut secrets: Vec<String> = std::env::vars()
        .filter(|(name, value)| {
            let name = name.to_ascii_uppercase();
            !value.is_empty()
                && ["TOKEN", "SECRET", "PASSWORD", "API_KEY", "APIKEY", "AUTHORIZATION"]
                    .iter()
                    .any(|suffix| name == *suffix || name.ends_with(&format!("_{suffix}")))
        })
        .flat_map(|(_, value)| {
            let json = serde_json::to_string(&value).unwrap_or_default();
            let escaped = json
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(&json)
                .to_string();
            [value.clone(), escaped, afkconvoy_core::line::Line::new().word(&value).done()]
        })
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets.into_iter().fold(text.to_string(), |text, secret| text.replace(&secret, "[redacted]"))
}

/// Where orca is run from: the host user's home, or the root on a host with
/// none. What orca takes from where it is run — the worktree a call names
/// none of — is then never the directory afkconvoy happens to run in, which
/// no command is about (docs/adr/0023).
fn asked_from() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.filter(|home| home.is_dir()).unwrap_or_else(|| PathBuf::from("/"))
}

/// Whether orca is running, read once in this look. Adding a repo does
/// not forget this: orca is running then, and the look has not ended.
pub(crate) static RUNNING: Reading<bool> = Reading::per_look("orca running");

/// Whether orca is running, read once in this look. An orca that cannot say
/// is not one a worker can be started through. A second ask waits on that
/// read instead of starting `orca` again.
pub fn running() -> bool {
    RUNNING.read("", || orca(&["status"]).is_ok_and(|status| status["runtime"]["state"] == "ready"))
}

/// What a start finds of an installed orca: whether it is running, and then
/// whether it knows the checkout `repo`. It only asks.
pub fn found(repo: &Repo) -> Result<Found> {
    if !running() {
        return Ok(Found::NotRunning);
    }
    // No git: another fault's to name.
    let Ok(main) = repo.main_checkout() else {
        return Ok(Found::Ready);
    };
    Ok(match Source::at(&main)? {
        Some(_) => Found::Ready,
        None => Found::KnowsNoCheckout(main.display().to_string()),
    })
}

/// The repo orca cuts this checkout's worktrees from.
pub struct Source {
    pub id: String,
}

/// Orca's host-wide repo listing, read once in this look. Adding a repo orca
/// did not list forgets this listing alone: the look has not ended.
pub(crate) static REPOS: Reading<Result<Value, String>> =
    Reading::per_look("orca repo list").forgotten_by(&[&["orca", "repo", "add"]]);

/// Orca's host-wide repo listing, read once in this look.
fn repo_list() -> Result<Value> {
    REPOS
        .read("", || orca(&["repo", "list"]).map_err(|error| format!("{error:#}")))
        .map_err(|error| anyhow!(error))
}

/// Orca's worktree list, read once in this look. Creating or removing a
/// worktree forgets this list alone: the look has not ended, and the other
/// readings stay the ones already read.
pub(crate) static WORKTREES: Reading<Result<Value, String>> =
    Reading::per_look("orca worktree list")
        .forgotten_by(&[&["orca", "worktree", "create"], &["orca", "worktree", "rm"]]);

/// Orca's worktree list, one read for this look. A second ask waits on the
/// first instead of listing again.
fn worktree_list() -> Result<Value> {
    WORKTREES.read("", list_worktrees).map_err(|error| anyhow!(error))
}

/// Asks orca for the worktree list anew, which is what makes orca drop a
/// removal failure it recorded.
fn worktree_list_again() -> Result<Value> {
    WORKTREES.again("", list_worktrees).map_err(|error| anyhow!(error))
}

fn list_worktrees() -> Result<Value, String> {
    orca(&["worktree", "list"]).map_err(|error| format!("{error:#}"))
}

impl Source {
    /// The orca repo whose path is this checkout's main worktree.
    pub fn of(repo: &Repo) -> Result<Source> {
        let main = repo.main_checkout()?;
        Source::at(&main)?.ok_or_else(|| {
            let path = main.display().to_string();
            anyhow!(
                "orca knows no repo at {path} — add this checkout once with `{}`",
                add_checkout(&path)
            )
        })
    }

    /// The orca repo whose path is `main`; `None` when orca knows none there.
    fn at(main: &Path) -> Result<Option<Source>> {
        let listed = repo_list()?;
        let rows = listed["repos"].as_array().cloned().unwrap_or_default();
        let found = rows
            .iter()
            .find(|row| row["path"].as_str().is_some_and(|path| same_dir(Path::new(path), main)));
        Ok(found.and_then(|row| row["id"].as_str()).map(|id| Source { id: id.to_string() }))
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The `head` orca lists for the worktree at `path`; empty when it lists none.
pub(crate) fn listed_head(path: &Path) -> Result<String> {
    let listed = worktree_list()?;
    let rows = listed["worktrees"].as_array().cloned().unwrap_or_default();
    let head = rows.iter().find_map(|row| {
        let listed = row["path"].as_str()?;
        same_dir(Path::new(listed), path).then(|| row["head"].as_str().unwrap_or("").to_string())
    });
    Ok(head.unwrap_or_default())
}

/// Whether `head` is a commit this repo has, and is on `origin/<base>` or,
/// where that remote ref exists, on `origin/<branch>`. An empty head, a head
/// that is not a commit here, or a remote that cannot be read, is not.
pub(crate) fn commit_is_on_remote(repo: &Repo, base: &str, branch: &str, head: &str) -> bool {
    let head = head.trim();
    if head.is_empty() {
        return false;
    }
    let object = format!("{head}^{{commit}}");
    if !repo.try_git(&["cat-file", "-e", &object]).is_ok_and(|ran| ran.ok) {
        return false;
    }
    let base_ref = format!("refs/remotes/{REMOTE}/{base}");
    let branch_ref = format!("refs/remotes/{REMOTE}/{branch}");
    let has_base = remote_ref(repo, &base_ref);
    let has_branch = !branch.is_empty() && remote_ref(repo, &branch_ref);
    if !has_base && !has_branch {
        return false;
    }
    let mut nots = Vec::new();
    if has_base {
        nots.push(base_ref);
    }
    if has_branch {
        nots.push(branch_ref);
    }
    let mut args = vec!["rev-list", "--count", head, "--not"];
    args.extend(nots.iter().map(String::as_str));
    match repo.try_git(&args) {
        Ok(ran) if ran.ok => ran.stdout == "0",
        _ => false,
    }
}

fn remote_ref(repo: &Repo, name: &str) -> bool {
    repo.try_git(&["rev-parse", "-q", "--verify", name]).is_ok_and(|ran| ran.ok)
}

/// Finishes removals orca already answered success for while the directory
/// remained. Git drops the worktree's registration and then stops on a
/// directory the owner cannot write; orca records that and still answers
/// success. A later tick removes the directory, including one git no longer
/// lists, and asks orca for the list so it drops the failure.
///
/// A worktree a process is still working in stays until that process is gone.
/// One whose head is empty, not a commit here, or holds a commit the remote
/// does not, stays. Only this checkout's rows. Asking orca to remove the
/// worktree again is not how a leftover goes: orca refuses a directory git
/// no longer lists. A candidate worktree is not one of these rows.
pub(crate) fn finish_failed_removals(repo: &Repo, base: &str) -> Result<()> {
    let source = Source::of(repo)?;
    let listed = worktree_list()?;
    let rows = listed["worktrees"].as_array().cloned().unwrap_or_default();
    // Every process of the host is listed only for a row that needs it: a
    // tick that finds no failed removal starts no `ps` (sunfmin/control#311).
    let mut busy = None;
    let mut finished = false;
    let mut retired = false;
    for row in &rows {
        if row["repoId"].as_str() != Some(source.id.as_str())
            || row["isMainWorktree"] == true
            || row["isArchived"] == true
        {
            continue;
        }
        if !row["removalError"].as_str().is_some_and(|error| !error.is_empty()) {
            continue;
        }
        let Some(path) = row["path"].as_str().map(PathBuf::from).filter(|path| path.is_dir())
        else {
            continue;
        };
        if busy.is_none() {
            busy = Some(crate::hot::working_directories()?);
        }
        if crate::hot::occupies(busy.as_deref().unwrap_or_default(), &path) {
            continue;
        }
        let branch = short_branch(row["branch"].as_str().unwrap_or(""));
        let head = row["head"].as_str().unwrap_or("");
        if !commit_is_on_remote(repo, base, branch, head) {
            continue;
        }
        let _ = worker_tmp::retire(&path);
        retired = true;
        crate::copies::removable(&path);
        if std::fs::remove_dir_all(&path).is_err() || path.exists() {
            eprintln!(
                "warning: {} is still there and will be removed on a later tick",
                path.display()
            );
            continue;
        }
        finished = true;
    }
    if retired {
        let _ = worker_tmp::reclaim();
    }
    if finished {
        worktree_list_again()?;
    }
    Ok(())
}

/// Keeps the stock watch's own directory known to orca, so its terminal can
/// start there without the operator registering it. An operator's directory
/// is never added or changed here.
pub fn watch_directory(directory: &Path) -> Result<()> {
    if Source::at(directory)?.is_none() {
        orca(&["repo", "add", "--path", &directory.display().to_string()])?;
    }
    Ok(())
}

/// What a bound repo's worker was started as, and under which commits of its
/// role's repo and of the team's: what `doctor` holds against their heads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Under {
    pub stage: String,
    pub role: String,
    pub role_commit: String,
    pub team_commit: String,
}

/// One worktree orca knows on this machine, of whichever repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnHost {
    pub path: PathBuf,
    /// The main checkout of its repo, where orca says one.
    pub repo: Option<PathBuf>,
    pub issue: Option<u64>,
    /// The landing train's, the caster's, a watch's or a cut's: the fleet's own.
    pub fleets: bool,
    /// A worker holds it: a terminal of it is live.
    pub held: bool,
}

/// The worktrees of `listed` — `orca worktree list` — each with its repo of
/// `repos` and whether `ps` says a terminal of it is live.
fn on_host(repos: &Value, listed: &Value, ps: &Value) -> Vec<OnHost> {
    let rows = |value: &Value, key: &str| value[key].as_array().cloned().unwrap_or_default();
    let repos = rows(repos, "repos");
    let live = rows(ps, "worktrees");
    let path_of = |row: &Value| row["path"].as_str().map(PathBuf::from);
    rows(listed, "worktrees")
        .iter()
        .filter(|row| row["isMainWorktree"] != true && row["isArchived"] != true)
        .filter_map(|row| {
            let path = path_of(row)?;
            let repo = repos
                .iter()
                .find(|repo| repo["id"].is_string() && repo["id"] == row["repoId"])
                .and_then(path_of);
            let held = live.iter().any(|ps| {
                path_of(ps).is_some_and(|live| same_dir(&live, &path))
                    && ps["liveTerminalCount"].as_u64().unwrap_or(0) > 0
            });
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            let fleets = is_worktree_named(name, TRAIN_WORKTREE)
                || is_worktree_named(name, CASTER_WORKTREE)
                || is_worktree_named(name, crate::host_watch::CHECKOUT)
                || is_worktree_named(name, cut::CHECKOUT);
            Some(OnHost { issue: row["linkedIssue"].as_u64(), path, repo, fleets, held })
        })
        .collect()
}

/// How long a send waits for the harness's session to record the turn.
/// Tests set this to zero and check once; production waits out the submit bound.
fn turn_wait_seconds() -> u64 {
    std::env::var("AFKCONVOY_TURN_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SUBMIT_SECONDS)
}

/// How long a note typed at a worker waits for its harness to give it to
/// its model: a harness that takes a line only at the end of its turn takes
/// it once the step it is on ends. Tests set it short.
pub(crate) fn note_wait_seconds() -> u64 {
    std::env::var("AFKCONVOY_NOTE_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(NOTE_WAIT_SECONDS)
}

const NOTE_WAIT_SECONDS: u64 = 600;
const NOTE_POLL: Duration = Duration::from_millis(500);

/// How a note reached a worker's model (`Worktree::hand_note`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handed {
    /// Typed, and its session held it this many seconds later.
    Taken { seconds: u64 },
    /// Its session held it already: typed by an earlier run.
    Already,
    /// The terminal took it, now or at an earlier tick, and its harness has
    /// not given it to its model yet (`Worktree::hand_update`): a harness
    /// running a tool hands over its queued input when that tool ends.
    Queued,
}

/// A worker's worktree on this machine.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    /// The branch orca named for it — never assumed from the worktree's name.
    pub branch: String,
    /// The worktree's own git dir, once it was asked for (`note`).
    git_dir: OnceLock<PathBuf>,
}

impl Worktree {
    pub(crate) fn at(path: PathBuf, branch: String) -> Worktree {
        Worktree { path, branch, git_dir: OnceLock::new() }
    }

    /// Has orca create a worktree and branch at the commit `sha`, linked to
    /// `issue` — and asserts the worktree contains that commit, however orca
    /// resolved it: a worker started on a base that is commits behind builds
    /// on files that have already moved.
    pub fn cut(source: &Source, name: &str, sha: &str, issue: Option<u64>) -> Result<Worktree> {
        let repo = format!("id:{}", source.id);
        let issue = issue.map(|number| number.to_string());
        let mut args = vec![
            "worktree",
            "create",
            "--repo",
            &repo,
            "--name",
            name,
            "--no-parent",
            "--base-branch",
            sha,
        ];
        if let Some(number) = &issue {
            args.extend(["--issue", number]);
        }
        // The list this look already read does not contain the worktree
        // created here: the create forgets it (`WORKTREES`), so a later
        // question, including a host-runner move after the tick, sees it.
        let made = orca(&args)?;
        let path = made["worktree"]["path"].as_str().map(PathBuf::from);
        let Some(path) = path.filter(|path| path.is_dir()) else {
            bail!("orca worktree create returned no usable path: {}", made["worktree"]["path"]);
        };
        let branch = short_branch(made["worktree"]["branch"].as_str().unwrap_or("")).to_string();
        if branch.is_empty() {
            bail!("orca worktree create named no branch for {}", path.display());
        }
        let checkout = Repo::new(path.clone());
        if !checkout.try_git(&["merge-base", "--is-ancestor", sha, "HEAD"])?.ok {
            checkout.git(&["merge", "--quiet", "--ff-only", sha])?;
        }
        Ok(Worktree::at(path, branch))
    }

    /// The worktree orca links to issue `number` on this machine: the most
    /// recently active, when a stale leftover sits beside a live one.
    pub fn of_issue(source: &Source, number: u64) -> Result<Option<Worktree>> {
        let listed = worktree_list()?;
        let rows = listed["worktrees"].as_array().cloned().unwrap_or_default();
        let mut hits: Vec<&Value> = rows
            .iter()
            .filter(|row| {
                row["linkedIssue"].as_u64() == Some(number)
                    && row["repoId"].as_str() == Some(source.id.as_str())
                    && row["isMainWorktree"] != true
                    && row["isArchived"] != true
            })
            .collect();
        hits.sort_by_key(|row| row["lastActivityAt"].as_u64().unwrap_or(0));
        Ok(hits.last().and_then(|row| {
            Some(Worktree::at(
                PathBuf::from(row["path"].as_str()?),
                short_branch(row["branch"].as_str().unwrap_or("")).to_string(),
            ))
        }))
    }

    /// Every worktree orca links to an issue of this checkout on this machine,
    /// with that issue.
    pub fn linked(source: &Source) -> Result<Vec<(u64, Worktree)>> {
        let listed = worktree_list()?;
        let rows = listed["worktrees"].as_array().cloned().unwrap_or_default();
        Ok(rows
            .iter()
            .filter(|row| {
                row["repoId"].as_str() == Some(source.id.as_str())
                    && row["isMainWorktree"] != true
                    && row["isArchived"] != true
            })
            .filter_map(|row| {
                let path = PathBuf::from(row["path"].as_str()?);
                let branch = short_branch(row["branch"].as_str().unwrap_or("")).to_string();
                Some((row["linkedIssue"].as_u64()?, Worktree::at(path, branch)))
            })
            .filter(|(_, worktree)| worktree.path.is_dir())
            .collect())
    }

    /// Every worktree of this checkout on this machine a worker may be in,
    /// with the issue orca links it to where it links one: the landing
    /// train's left out, whose gate runs are the fleet's own and no worker's.
    pub fn of_workers(source: &Source) -> Result<Vec<(Option<u64>, Worktree)>> {
        Ok(Self::of_sessions(source)?
            .into_iter()
            .filter(|(_, worktree)| !worktree.is_fleets())
            .collect())
    }

    /// Every session worktree, including the fleet's train, caster and
    /// watch: launch evidence applies to those sessions too.
    pub fn of_sessions(source: &Source) -> Result<Vec<(Option<u64>, Worktree)>> {
        let listed = worktree_list()?;
        let rows = listed["worktrees"].as_array().cloned().unwrap_or_default();
        Ok(rows
            .iter()
            .filter(|row| {
                row["repoId"].as_str() == Some(source.id.as_str())
                    && row["isMainWorktree"] != true
                    && row["isArchived"] != true
            })
            .filter_map(|row| {
                let path = PathBuf::from(row["path"].as_str()?);
                let branch = short_branch(row["branch"].as_str().unwrap_or("")).to_string();
                Some((row["linkedIssue"].as_u64(), Worktree::at(path, branch)))
            })
            .filter(|(_, worktree)| worktree.path.is_dir())
            .collect())
    }

    /// Every worktree orca knows on this machine, of every repo — so of every
    /// fleet of the host — each with the main checkout of its repo, the issue
    /// it is linked to, and whether a worker holds it: a terminal of it is
    /// live. A main checkout is no worktree of a fleet's, and is left out.
    pub fn on_host() -> Result<Vec<OnHost>> {
        let repos = repo_list()?;
        let listed = worktree_list()?;
        let ps = runtime()?;
        Ok(on_host(&repos, &listed, &ps))
    }

    /// Whether this is the landing train's worktree, the caster's, an
    /// watch's or a cut's: the fleet's own, and no worker's.
    fn is_fleets(&self) -> bool {
        let name = self.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        is_worktree_named(name, TRAIN_WORKTREE)
            || is_worktree_named(name, CASTER_WORKTREE)
            || is_worktree_named(name, crate::host_watch::CHECKOUT)
            || is_worktree_named(name, cut::CHECKOUT)
    }

    /// The checkout `repo`, as a worker's worktree.
    pub fn of(repo: &Repo) -> Result<Worktree> {
        let branch = repo.try_git(&["symbolic-ref", "--short", "HEAD"])?;
        let branch = if branch.ok { branch.stdout } else { String::new() };
        Ok(Worktree::at(repo.dir.clone(), branch))
    }

    /// Whether the fleet put a worker here: it keeps a note of every worker
    /// it starts — its brief, and when it started — in the worktree's own git
    /// dir (`note`), and in no other checkout's. Being a linked worktree says
    /// nothing of it: a human's own checkout may be one, orca's or not.
    pub fn holds_a_worker(&self) -> bool {
        let noted = |name| self.note(name).is_ok_and(|note| note.exists());
        noted(BRIEF_FILE) || noted(START_FILE)
    }

    /// The worktree orca links to issue `number`, and what the worker in it
    /// is doing: gone, where there is no worktree.
    pub fn worker_of(
        source: &Source,
        number: u64,
        now: u64,
    ) -> Result<(Option<Worktree>, WorkerState)> {
        let worktree = Worktree::of_issue(source, number)?;
        let state = match &worktree {
            Some(worktree) => worktree.state(now)?,
            None => WorkerState::Gone,
        };
        Ok((worktree, state))
    }

    /// The landing train's worktree on this machine.
    pub fn of_train(source: &Source) -> Result<Option<Worktree>> {
        Worktree::named(source, TRAIN_WORKTREE)
    }

    /// The worktree created under `name` on this machine: linked to no issue,
    /// it is known by the name its directory was created under.
    pub fn named(source: &Source, created_as: &str) -> Result<Option<Worktree>> {
        let listed = worktree_list()?;
        let rows = listed["worktrees"].as_array().cloned().unwrap_or_default();
        let mut hits: Vec<&Value> = rows
            .iter()
            .filter(|row| {
                let path = Path::new(row["path"].as_str().unwrap_or(""));
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                is_worktree_named(name, created_as)
                    && path.is_dir()
                    && row["repoId"].as_str() == Some(source.id.as_str())
                    && row["isMainWorktree"] != true
                    && row["isArchived"] != true
            })
            .collect();
        hits.sort_by_key(|row| row["lastActivityAt"].as_u64().unwrap_or(0));
        Ok(hits.last().and_then(|row| {
            Some(Worktree::at(
                PathBuf::from(row["path"].as_str()?),
                short_branch(row["branch"].as_str().unwrap_or("")).to_string(),
            ))
        }))
    }

    /// What the worker here is doing. A live terminal comes from orca; the
    /// turn comes from the harness's session for this worktree. A page of
    /// `ps` that was cut short is an error: an orca that cannot say whether
    /// a terminal is live is never "no worker there".
    pub fn state(&self, now: u64) -> Result<WorkerState> {
        let row = self.health_report()?;
        let terminals = row["liveTerminalCount"].as_u64().unwrap_or(0);
        if terminals > 0 && crate::health::has_signal(self)? {
            return Ok(WorkerState::Idle { quiet: None });
        }
        if terminals == 0 {
            return Ok(WorkerState::Gone);
        }
        let harness = agent_harness(&main_agent(&row)).or(self.recorded_harness()?);
        let since = self.handed(None)?;
        let turn = crate::session::read_turn(
            &self.path,
            harness.unwrap_or(Harness::DEFAULT),
            since,
            None,
        )?;
        Ok(worker_state(terminals, turn, now))
    }

    /// Actual structured runtime observations for diagnosis. A transport
    /// failure is an error, never evidence that an agent has stopped.
    pub fn health_report(&self) -> Result<Value> {
        Ok(runtime_row(&runtime()?, &self.path))
    }

    /// Orca's state, outcome, agent type and timestamps for this worker.
    pub fn agent_report(&self) -> Result<Value> {
        let agent = main_agent(&self.health_report()?);
        if agent.is_null() {
            return Ok(Value::Null);
        }
        Ok(serde_json::json!({"state": agent["state"],
            "outcome": agent["mainAgent"]["outcome"], "agentType": agent["agentType"],
            "stateStartedAt": agent["stateStartedAt"], "programStatus": agent["programStatus"]}))
    }

    /// A terminal just created or confirmed closed supersedes this tick's
    /// old row. Keep no agent or output from the session it replaced. A
    /// newly created terminal is present, with its start time kept by start,
    /// so later steps cannot mistake the continuation for a gone worker.
    fn observed_terminals(&self, count: u64) {
        OBSERVATION.with(|slot| {
            if let Some(Ok(ps)) = slot.borrow_mut().as_mut()
                && let Some(rows) = ps["worktrees"].as_array_mut()
            {
                let at = rows.iter().position(|row| {
                    row["path"].as_str().is_some_and(|p| same_dir(Path::new(p), &self.path))
                });
                let at = at.unwrap_or_else(|| {
                    rows.push(serde_json::json!({"path": self.path}));
                    rows.len() - 1
                });
                rows[at]["agents"] = serde_json::json!([]);
                rows[at]["lastOutputAt"] = Value::Null;
                rows[at]["liveTerminalCount"] = serde_json::json!(count);
            }
        });
    }

    /// The terminal a worker here would be in: the one that wrote last.
    pub fn terminal(&self) -> Result<Option<String>> {
        let listed = orca(&["terminal", "list", "--worktree", &self.selector()])?;
        let rows = listed["terminals"].as_array().cloned().unwrap_or_default();
        let last = rows.iter().max_by_key(|row| row["lastOutputAt"].as_u64().unwrap_or(0));
        Ok(last.and_then(|row| row["handle"].as_str()).map(str::to_string))
    }

    /// A note about the worker here, kept in the worktree's own git dir: this
    /// machine's, like the worker itself, and gone when the worktree is. It
    /// is found once (`Repo::git_dir`).
    pub(crate) fn note(&self, name: &str) -> Result<PathBuf> {
        if self.git_dir.get().is_none() {
            let _ = self.git_dir.set(Repo::new(self.path.clone()).git_dir()?);
        }
        Ok(self.git_dir.get().expect("set above").join(name))
    }

    fn forget(&self, name: &str) -> Result<()> {
        let file = self.note(name)?;
        if file.exists() {
            std::fs::remove_file(&file)
                .with_context(|| format!("cannot remove {}", file.display()))?;
        }
        Ok(())
    }

    /// Closes every terminal here and keeps the worktree: the worker whose
    /// work here is over is ended, whatever it is doing. One with no terminal
    /// is as wanted.
    pub fn close_terminals(&self) -> Result<()> {
        self.close_any().map(|_| ())
    }

    /// `close_terminals` → whether there was a terminal here to close.
    fn close_any(&self) -> Result<bool> {
        if self.terminal()?.is_none() {
            return Ok(false);
        }
        orca(&["terminal", "close", "--worktree", &self.selector(), "--all"])?;
        if OBSERVATION.with(|slot| slot.borrow().is_some()) && self.terminal()?.is_none() {
            // A closed worker's old identity cannot override the replacement
            // that starts later in this tick. Other workers keep their view.
            self.observed_terminals(0);
        }
        Ok(true)
    }

    /// End the old agent before a continuation can rewrite its brief or
    /// start another writer. Include descendants that escaped the terminal.
    pub(crate) fn stop_for_continuation(&self) -> Result<()> {
        match self.end_worker("start", "no replacement was started")? {
            Ended::Confirmed => Ok(()),
            Ended::Unconfirmed(why) => bail!("{why}"),
        }
    }

    /// Ends the session the fleet started here for an issue whose work has
    /// landed, before settling removes the worktree: its terminals and
    /// process tree, confirmed gone. One not yet confirmed is `Unconfirmed`,
    /// not an error: settling waits for it, and a later tick tries again.
    pub(crate) fn end_settled(&self) -> Result<Ended> {
        self.end_worker("tick", "a later tick removes it")
    }

    /// Closes every terminal here and ends the worker's process tree, each
    /// end confirmed: one still there after SIGKILL is `Unconfirmed`, saying
    /// `consequence`. Each process ended is a line of the tick log, under
    /// `command`.
    fn end_worker(&self, command: &str, consequence: &str) -> Result<Ended> {
        let before = crate::hot::listed_checked()?;
        let pids: Vec<_> = crate::hot::tree(&before, &self.path).iter().map(|p| p.pid).collect();
        self.close_terminals()?;
        let clock = TerminationTime::new();
        let until = clock.now() + Duration::from_secs(5);
        let mut ending = BTreeMap::<u32, Ending>::new();
        loop {
            let processes = crate::hot::listed_checked()?;
            let writers = crate::hot::tree(&processes, &self.path);
            for proc in &processes {
                if pids.contains(&proc.pid)
                    || ending.contains_key(&proc.pid)
                    || writers.iter().any(|writer| writer.pid == proc.pid)
                {
                    let record = ending.entry(proc.pid).or_insert_with(|| {
                        let mut record = Ending::observe(proc, &clock);
                        record.signal(proc.pid, false);
                        record
                    });
                    if !record.forced
                        && clock.now().saturating_sub(record.since) >= Duration::from_secs(1)
                    {
                        record.signal(proc.pid, true);
                    }
                }
            }
            for (pid, record) in &mut ending {
                if !processes.iter().any(|proc| proc.pid == *pid) {
                    record.log_ended(&self.path, command);
                }
            }
            let old_alive =
                processes.iter().any(|p| pids.contains(&p.pid) || ending.contains_key(&p.pid));
            // Termination must be confirmed after close, never against the
            // tick's earlier observation of the running agent.
            let report = runtime_row(&runtime_live()?, &self.path);
            let agents = report["agents"].as_array();
            let agent_alive = agents.is_some_and(|agents| agents.iter().any(agent_running));
            if self.terminal()?.is_none() && writers.is_empty() && !old_alive && !agent_alive {
                return Ok(Ended::Confirmed);
            }
            if clock.now() >= until {
                let survivors = ending
                    .iter()
                    .filter(|(pid, _)| processes.iter().any(|proc| proc.pid == **pid))
                    .map(|(_, record)| match &record.error {
                        Some(error) => format!("{} ({error})", record.description),
                        None => record.description.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                return Ok(Ended::Unconfirmed(format!(
                    "old worker termination is unconfirmed in {}; {consequence}{}",
                    self.path.display(),
                    if survivors.is_empty() {
                        String::new()
                    } else {
                        format!("; survived SIGKILL: {survivors}")
                    }
                )));
            }
            clock.sleep(Duration::from_millis(100));
        }
    }

    /// Clears the worktree and keeps it: its terminals closed, what it held
    /// discarded for the commit `sha`, what its worker was told forgotten.
    /// What git ignores — everything a gate run built — stays.
    pub fn clear(&self, sha: &str) -> Result<()> {
        // One with no terminal is as wanted.
        let _ = orca(&["terminal", "close", "--worktree", &self.selector(), "--all"]);
        let repo = Repo::new(self.path.clone());
        repo.git(&["reset", "--quiet", "--hard", sha])?;
        repo.git(&["clean", "--quiet", "-fd"])?;
        self.forget(START_FILE)?;
        self.forget(BACKGROUND_FILE)?;
        self.forget(NUDGE_FILE)
    }

    /// Whether the worker here was nudged, and when.
    pub fn nudged(&self) -> Result<Nudged> {
        let at = std::fs::read_to_string(self.note(NUDGE_FILE)?).ok();
        Ok(match at.and_then(|at| at.trim().parse().ok()) {
            Some(at) => Nudged::At(at),
            None => Nudged::Never,
        })
    }

    /// The one since this worker has waited on its own background work.
    ///
    /// `waiting` is what one read of its screen and the exclusive queue just
    /// answered. `earlier` is a moment already known — when it asked the
    /// queue — and the stamp stays the earliest of that and the moment first
    /// written. `None`, and the record gone, when it waits on none: the turn
    /// is open, or that read showed nothing. Nothing else keeps a since.
    pub fn background_since(
        &self,
        waiting: bool,
        earlier: Option<u64>,
        now: u64,
    ) -> Result<Option<u64>> {
        if !waiting {
            self.forget(BACKGROUND_FILE)?;
            return Ok(None);
        }
        let file = self.note(BACKGROUND_FILE)?;
        let since = self.background_seen(earlier, now);
        if self.background_at() != Some(since) {
            std::fs::write(&file, since.to_string())
                .with_context(|| format!("cannot write {}", file.display()))?;
        }
        Ok(Some(since))
    }

    /// The since `background_since` would keep for a worker waiting on
    /// background work of its own, read and not written: what `doctor`
    /// judges a silence by without moving it.
    pub fn background_seen(&self, earlier: Option<u64>, now: u64) -> u64 {
        match (self.background_at(), earlier) {
            (Some(seen), Some(earlier)) => seen.min(earlier),
            (Some(seen), None) => seen,
            (None, Some(earlier)) => earlier,
            (None, None) => now,
        }
    }

    /// The one since, read and not written. `None` when the turn is open or
    /// the worker was last seen waiting on none.
    pub fn background_at(&self) -> Option<u64> {
        let file = self.note(BACKGROUND_FILE).ok()?;
        std::fs::read_to_string(file).ok()?.trim().parse().ok()
    }

    /// Keeps that the note `comment` reached the model of the worker here at
    /// `at`: one line each, beside its nudge.
    pub fn record_told(&self, at: u64, comment: &str) -> Result<()> {
        use std::io::Write;
        let file = self.note(TOLD_FILE)?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .and_then(|mut told| writeln!(told, "{at} {comment}"))
            .with_context(|| format!("cannot write {}", file.display()))
    }

    pub fn record_nudge(&self, at: u64) -> Result<()> {
        let file = self.note(NUDGE_FILE)?;
        std::fs::write(&file, at.to_string())
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// The directory what shows the worker here hot is taken down in.
    pub fn evidence_dir(&self) -> Result<PathBuf> {
        self.note(EVIDENCE_DIR)
    }

    /// What the cut in this checkout is and did so far, where it says; `None`
    /// where nothing does, or what does cannot be read.
    pub fn cut_record(&self) -> Result<Option<cut::Record>> {
        let said = std::fs::read_to_string(self.cut_record_file()?).unwrap_or_default();
        Ok(serde_json::from_str(&said).ok())
    }

    /// The file that keeps this cut's record, in its checkout's own git dir.
    pub fn cut_record_file(&self) -> Result<PathBuf> {
        self.note(CUT_FILE)
    }

    /// Keeps what the cut in this checkout is and did so far: the last it
    /// said stands. It is written whole, then put in place: a tick reading it
    /// meanwhile reads the one before, never half of this one.
    pub fn record_cut(&self, record: &cut::Record) -> Result<()> {
        let file = self.cut_record_file()?;
        let written = self.note(&format!("{CUT_FILE}.new"))?;
        std::fs::write(&written, serde_json::to_string(record)?)
            .with_context(|| format!("cannot write {}", written.display()))?;
        std::fs::rename(&written, &file).with_context(|| format!("cannot write {}", file.display()))
    }

    /// The file what each step of the cut in this checkout prints is kept in.
    pub fn cut_log(&self) -> Result<PathBuf> {
        self.note(CUT_LOG_FILE)
    }

    /// The commit the branch here is at.
    pub fn tip(&self) -> Result<String> {
        Repo::new(self.path.clone()).git(&["rev-parse", "HEAD"])
    }

    /// The branch checked out here now; `None` on a detached HEAD.
    pub fn branch_now(&self) -> Result<Option<String>> {
        let named = Repo::new(self.path.clone()).try_git(&["symbolic-ref", "--short", "HEAD"])?;
        Ok(named.ok.then_some(named.stdout))
    }

    /// Where the stage that writes nothing started here, when one did.
    pub fn writes_nothing(&self) -> Result<Option<WritesNothing>> {
        let note = std::fs::read_to_string(self.note(WRITES_NOTHING_FILE)?).unwrap_or_default();
        Ok(WritesNothing::read(&note))
    }

    /// Keeps where a stage that writes nothing starts here: what `started`
    /// makes of the branch's tip. A start kept under the same claim stands:
    /// the worker that continues a stage answers for the one before it.
    pub fn keep_writes_nothing(&self, started: impl FnOnce(String) -> WritesNothing) -> Result<()> {
        let starting = started(self.tip()?);
        if self.writes_nothing()?.is_some_and(|kept| kept.claim == starting.claim) {
            return Ok(());
        }
        let file = self.note(WRITES_NOTHING_FILE)?;
        std::fs::write(&file, starting.note())
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// What the worker here was started under, when a bound repo's was.
    pub fn under(&self) -> Result<Option<Under>> {
        let kept = std::fs::read_to_string(self.note(UNDER_FILE)?).unwrap_or_default();
        Ok(serde_json::from_str(&kept).ok())
    }

    /// Keeps what a bound repo's worker starts under here.
    pub fn keep_under(&self, under: &Under) -> Result<()> {
        let file = self.note(UNDER_FILE)?;
        std::fs::write(&file, serde_json::to_string(under)?)
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// The harness the start recorded. Kept separately so doctor can name
    /// a disagreement without rewriting the evidence.
    pub fn recorded_harness(&self) -> Result<Option<Harness>> {
        let kept = std::fs::read_to_string(self.note(HARNESS_FILE)?).unwrap_or_default();
        Ok(Harness::read(kept.trim()))
    }

    /// The harness actually running: Orca's agent type wins over the record.
    pub fn harness(&self) -> Result<Option<Harness>> {
        Ok(agent_harness(&self.agent_report()?).or(self.recorded_harness()?))
    }

    /// Keeps the harness the worker here is started on.
    pub fn keep_harness(&self, harness: Harness) -> Result<()> {
        let file = self.note(HARNESS_FILE)?;
        std::fs::write(&file, harness.key())
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// Keeps the tracker of the fleet starting the worker here, before it
    /// starts: a command the worker runs reads its fleet's tracker here
    /// (`tracker`), whether or not the checkout list holds its checkout yet.
    pub fn keep_tracker(&self, tracker: &Tracker) -> Result<()> {
        let file = self.note(TRACKER_FILE)?;
        let named = tracker.named().map(RepoName::as_str).unwrap_or_default();
        std::fs::write(&file, named).with_context(|| format!("cannot write {}", file.display()))
    }

    /// The tracker the fleet that started the worker here kept
    /// (`keep_tracker`): the repo it names, `Some(None)` for the checkout's
    /// own, `None` where none was kept. One that names no repo is an error.
    pub fn tracker(&self) -> Result<Option<Option<RepoName>>> {
        let file = self.note(TRACKER_FILE)?;
        let kept = match std::fs::read_to_string(&file) {
            Ok(kept) => kept,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", file.display()));
            }
        };
        match kept.trim() {
            "" => Ok(Some(None)),
            named => match RepoName::read(named) {
                Some(named) => Ok(Some(Some(named))),
                None => bail!(
                    "{} says the tracker of the worker here is {named:?}, which names no repo, \
                     `<owner>/<repo>`",
                    file.display()
                ),
            },
        }
    }

    /// The comment of the `blocked` verdict last answered here, when one was.
    pub fn answered(&self) -> Result<Option<String>> {
        let said = std::fs::read_to_string(self.note(ANSWERED_FILE)?).ok();
        Ok(said.map(|url| url.trim().to_string()).filter(|url| !url.is_empty()))
    }

    pub fn record_answered(&self, said: &str) -> Result<()> {
        let file = self.note(ANSWERED_FILE)?;
        std::fs::write(&file, said).with_context(|| format!("cannot write {}", file.display()))
    }

    /// The last `lines` lines of what `terminal` shows.
    pub fn screen(&self, terminal: &str, lines: usize) -> Result<Vec<String>> {
        let limit = lines.to_string();
        let read =
            orca(&["terminal", "read", "--terminal", terminal, "--screen", "--limit", &limit])?;
        let tail = read["terminal"]["tail"].as_array().cloned().unwrap_or_default();
        Ok(tail.iter().filter_map(|line| line.as_str().map(str::to_string)).collect())
    }

    /// Has orca remove the worktree, its terminals with it — and its branch,
    /// which orca deletes unless it holds changes nothing has merged — and
    /// its worker's `TMPDIR` with it (`worker_tmp`): turned away first, for a
    /// server the worker started, and reclaimed once nothing runs with it.
    ///
    /// Directories the owner cannot write are opened first (`copies::removable`):
    /// git deletes the gitdir and then stops on one, and orca records that and
    /// still answers success. A directory still there is removed, and the list
    /// is read again so orca drops the failure. An orca error removes nothing
    /// further: the caller keeps the claim and a later tick tries again.
    pub fn remove(&self) -> Result<()> {
        // One that cannot be turned costs a server the worker started its
        // directory, and no worktree is kept for that.
        let _ = worker_tmp::retire(&self.path);
        crate::copies::removable(&self.path);
        orca(&["worktree", "rm", "--worktree", &self.selector(), "--force"])?;
        if self.path.exists() {
            crate::copies::removable(&self.path);
            std::fs::remove_dir_all(&self.path)
                .with_context(|| format!("cannot remove {}", self.path.display()))?;
        }
        if self.path.exists() {
            bail!("{} is still there", self.path.display());
        }
        // The directory is gone. Orca drops a failure it recorded during the
        // removal only when it is asked for the list again.
        worktree_list_again()?;
        let _ = worker_tmp::reclaim();
        Ok(())
    }

    fn selector(&self) -> String {
        format!("path:{}", self.path.display())
    }

    /// Where the instructions of the worker here are kept.
    pub fn brief(&self) -> Result<PathBuf> {
        self.note(BRIEF_FILE)
    }

    /// Writes the worker's instructions where its brief pointer says → the file.
    pub fn write_brief(&self, prompt: &str) -> Result<PathBuf> {
        let brief = self.brief()?;
        std::fs::write(&brief, prompt)
            .with_context(|| format!("cannot write the worker's brief {}", brief.display()))?;
        Ok(brief)
    }

    /// Starts a worker here with the worker launch command, handed to orca
    /// verbatim but run under `nice` with its own `TMPDIR` (`niced`) → its
    /// terminal's handle. Whatever terminal was here first is
    /// closed, so the worker's is the only one in the worktree: two agents in
    /// one worktree would fight, and orca opens a bare shell in every worktree
    /// it cuts. A new worker was never nudged, and waits on no background
    /// work, whatever the one before it did, and was started `now`.
    /// Where `title` is given, orca shows it as the terminal's title.
    pub fn start(&self, command: &str, title: Option<&str>, now: u64) -> Result<String> {
        // A failed close must not turn a continuation into a second writer.
        // Check again even after an acknowledged close: Orca may still be
        // ending the previous terminal. Leave the worker's progress markers
        // intact until its replacement can actually be started. Where there
        // was no terminal to close, the look that found none is that check.
        let closed = if self.recorded_harness()?.is_some() {
            self.stop_for_continuation()?;
            true
        } else {
            self.close_any()?
        };
        if closed && self.terminal()?.is_some() {
            bail!(
                "the previous worker in {} still has a terminal after close; \
                 no replacement was started",
                self.path.display()
            );
        }
        self.forget(NUDGE_FILE)?;
        self.forget(BACKGROUND_FILE)?;
        let command = niced(&worker_tmp::make(&self.path)?.display().to_string(), command);
        let selector = self.selector();
        let mut args = vec!["terminal", "create", "--worktree", &selector, "--command", &command];
        if let Some(title) = title {
            args.extend(["--title", title]);
        }
        let made = orca(&args)?;
        let handle = made["terminal"]["handle"].as_str();
        let handle =
            handle.ok_or_else(|| anyhow!("orca terminal create returned no terminal handle"))?;
        self.observed_terminals(1);
        self.forget(crate::health::SIGNAL_FILE)?;
        let file = self.note(START_FILE)?;
        std::fs::write(&file, now.to_string())
            .with_context(|| format!("cannot write {}", file.display()))?;
        crate::turn_answer::forget_wait(self)?;
        Ok(handle.to_string())
    }

    /// Starts a watch's terminal here, titled `title`, running `line`: beside
    /// whatever terminals are here already, the operator's own among them.
    pub fn start_watch(&self, line: &str, title: &str) -> Result<String> {
        let command = niced(&worker_tmp::make(&self.path)?.display().to_string(), line);
        let made = orca(&[
            "terminal",
            "create",
            "--worktree",
            &self.selector(),
            "--title",
            title,
            "--command",
            &command,
        ])?;
        made["terminal"]["handle"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("orca terminal create returned no terminal handle"))
    }

    /// Whether `terminal`, of the terminals here, is live.
    pub fn live(&self, terminal: &str) -> Result<bool> {
        let listed = orca(&["terminal", "list", "--worktree", &self.selector()])?;
        let rows = listed["terminals"].as_array().cloned().unwrap_or_default();
        Ok(rows.iter().any(|row| row["handle"] == terminal))
    }

    /// Public terminal metadata for watch recovery. An incomplete query is
    /// never evidence that a session disappeared.
    pub(crate) fn watch_terminals(&self) -> Result<Vec<Value>> {
        let listed = orca(&["terminal", "list", "--worktree", &self.selector()])?;
        anyhow::ensure!(listed["truncated"] != true, "orca's terminal list is truncated");
        let rows = listed["terminals"]
            .as_array()
            .ok_or_else(|| anyhow!("orca terminal list returned no terminals array"))?;
        anyhow::ensure!(
            listed["hostScope"]["omittedHostIds"].as_array().is_none_or(|ids| ids.is_empty()),
            "orca's terminal list omits execution hosts"
        );
        anyhow::ensure!(
            rows.iter().all(|row| {
                (row["worktreePath"].is_null()
                    || row["worktreePath"] == self.path.display().to_string())
                    && (row["executionHostId"].is_null() || row["executionHostId"] == "local")
            }),
            "watch terminal ownership cannot be confirmed on this local directory and host"
        );
        Ok(rows.clone())
    }

    /// A session sharing a directory with the operator uses only its own
    /// pane's runtime report, and the session `harness` recorded for this
    /// directory. The directory need not be git.
    pub(crate) fn terminal_health(
        &self,
        terminal: &str,
        harness: Harness,
        now: u64,
    ) -> Result<(WorkerState, Value)> {
        let shown = orca(&["terminal", "show", "--terminal", terminal])?;
        let row = self.health_report()?;
        let agent = pane_agent(&row, &shown["terminal"]);
        let since = self.handed(None).ok().flatten();
        let turn = crate::session::read_turn(&self.path, harness, since, None)?;
        Ok((worker_state(1, turn, now), agent))
    }

    /// Whether `harness` recorded a turn for this directory that started at
    /// or after `earliest`. Waits out the submit bound, checking as soon as
    /// the session has it.
    pub(crate) fn prove(&self, harness: Harness, earliest: u64) -> Result<bool> {
        self.poll_turn(harness, earliest, true)
    }

    /// One look, for a send Orca already failed: the turn is either written
    /// or it is not.
    pub(crate) fn saw_turn(&self, harness: Harness, earliest: u64) -> Result<bool> {
        self.poll_turn(harness, earliest, false)
    }

    fn poll_turn(&self, harness: Harness, earliest: u64, wait: bool) -> Result<bool> {
        let seconds = if wait { turn_wait_seconds() } else { 0 };
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let since = Some(earliest.saturating_sub(2));
        loop {
            if crate::session::read_turn(&self.path, harness, since, Some(earliest))?.is_some() {
                return Ok(true);
            }
            if seconds == 0 || Instant::now() >= deadline {
                return Ok(false);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            std::thread::sleep(Duration::from_millis(200).min(left));
        }
    }

    /// Delivers to a watch with the same input check and session proof as a
    /// worker. Its directory need not be git, so it keeps no worktree notes.
    pub(crate) fn tell_watch(&self, terminal: &str, line: &str, harness: Harness) -> Result<()> {
        let earliest = crate::clock().saturating_sub(2);
        self.tell_unchecked(terminal, line, BRIEF_SETTLE_SECONDS, Some(harness), earliest)
    }

    /// Types `line` once at the harness in `terminal`, at work or at its
    /// prompt, and waits until that harness's session holds `mark` in a
    /// message its model was given as the user's (`session::holds_note`):
    /// a line seen only in its input queue is not delivered. When the
    /// harness's screen shows the line queued, and that harness names a key
    /// that sends it now, the key is pressed once. A session that already
    /// holds it — a rerun after the line was taken — is typed nothing. Past
    /// the bound it fails with the screen, and types no second line. `since`
    /// drops session files written before it.
    pub(crate) fn hand_note(
        &self,
        terminal: &str,
        line: &str,
        mark: &str,
        since: u64,
    ) -> Result<Handed> {
        self.hand_note_wait(terminal, line, mark, since, None)
    }

    /// A tick polls briefly. Once accepted, keep the send receipt so a later
    /// tick waits for the queued line rather than typing a duplicate. A line
    /// the terminal took but the harness has not given its model within the
    /// poll is `Handed::Queued`, no failure: its caller bounds how long it
    /// may stay queued. A refused line, or no harness at its input, fails.
    pub(crate) fn hand_update(
        &self,
        terminal: &str,
        line: &str,
        mark: &str,
        since: u64,
        receipt: &std::path::Path,
    ) -> Result<Handed> {
        self.hand_note_wait(terminal, line, mark, since, Some(receipt))
    }

    fn hand_note_wait(
        &self,
        terminal: &str,
        line: &str,
        mark: &str,
        since: u64,
        receipt: Option<&std::path::Path>,
    ) -> Result<Handed> {
        let row = runtime_row(&runtime_live()?, &self.path);
        let agent = if row["liveTerminalCount"].as_u64().unwrap_or(0) > 1 {
            let shown = orca(&["terminal", "show", "--terminal", terminal])?;
            pane_agent(&row, &shown["terminal"])
        } else {
            main_agent(&row)
        };
        let state = agent["programStatus"]["state"].as_str().or(agent["state"].as_str());
        // A harness at work has no idle prompt to wait for: its input takes
        // the line, and the harness hands it over mid-turn or after it.
        if state != Some("working") {
            let (ready, _) =
                self.wait_for_input(terminal, Duration::from_secs(1)).with_context(|| {
                    self.prompt_context(
                        terminal,
                        "no running worker harness confirmed; no line was sent",
                    )
                })?;
            if ready != Waited::Ready {
                return Err(self.prompt_failure(
                    terminal,
                    "no running worker harness at its input; no line was sent",
                ));
            }
        }
        let harness =
            agent_harness(&agent).or(self.recorded_harness()?).unwrap_or(Harness::DEFAULT);
        if crate::session::holds_note(&self.path, harness, since, mark)? {
            return Ok(Handed::Already);
        }
        let accepted = receipt
            .and_then(|file| std::fs::read_to_string(file).ok())
            .is_some_and(|saved| saved == terminal);
        if !accepted {
            let sent =
                orca(&["terminal", "send", "--terminal", terminal, "--text", line, "--enter"])?;
            if sent["send"]["accepted"] != true {
                return Err(self.prompt_failure(
                    terminal,
                    &format!("the terminal did not take the line: {}", sent["send"]),
                ));
            }
            if let Some(file) = receipt {
                std::fs::write(file, terminal)?;
            }
        }
        let typed = Instant::now();
        let bound = Duration::from_secs(if receipt.is_some() { 1 } else { note_wait_seconds() });
        let mut sent_now = false;
        loop {
            if crate::session::holds_note(&self.path, harness, since, mark)? {
                return Ok(Handed::Taken { seconds: typed.elapsed().as_secs() });
            }
            if !sent_now {
                // A screen that cannot be read is not a queue. The bound
                // still applies, and the line is not typed again.
                if let Ok(screen) = self.queue_screen(terminal)
                    && let Some(key) = harness.send_queued(&screen, line)
                {
                    self.press_key(terminal, key)?;
                    sent_now = true;
                    continue;
                }
            }
            if typed.elapsed() >= bound {
                if receipt.is_some() {
                    return Ok(Handed::Queued);
                }
                return Err(self.prompt_failure(
                    terminal,
                    &format!(
                        "{} did not give its model the line within {}s: it is not delivered, \
                         and no second line was typed",
                        harness.name(),
                        bound.as_secs()
                    ),
                ));
            }
            std::thread::sleep(NOTE_POLL.min(bound.saturating_sub(typed.elapsed())));
        }
    }

    /// How many rendered rows are read when looking for a queued line. The
    /// queue sits at the bottom of the screen, with the composer.
    const QUEUE_SCREEN: usize = 40;

    /// The rendered screen, plus the composer draft when orca reports one.
    /// The draft is not terminal output; a queued line can sit there.
    fn queue_screen(&self, terminal: &str) -> Result<Vec<String>> {
        let limit = Self::QUEUE_SCREEN.to_string();
        let read =
            orca(&["terminal", "read", "--terminal", terminal, "--screen", "--limit", &limit])?;
        let tail = read["terminal"]["tail"].as_array().cloned().unwrap_or_default();
        let mut lines =
            tail.iter().filter_map(|line| line.as_str().map(str::to_string)).collect::<Vec<_>>();
        if let Some(draft) = read["terminal"]["draft"].as_str() {
            lines.extend(draft.lines().map(str::to_string));
        }
        Ok(lines)
    }

    /// Types `key` at `terminal`, with no line of the operator's.
    pub(crate) fn press_key(&self, terminal: &str, key: Keystroke) -> Result<()> {
        let mut args = vec!["terminal", "send", "--terminal", terminal];
        if let Some(text) = key.text {
            args.extend(["--text", text]);
        }
        if key.enter {
            args.push("--enter");
        }
        let sent = orca(&args)?;
        if sent["send"]["accepted"] != true {
            return Err(self.prompt_failure(
                terminal,
                &format!("the terminal did not take the key: {}", sent["send"]),
            ));
        }
        Ok(())
    }

    /// Checks this pane's input without typing or waiting for an active
    /// model turn. A queued watch round must leave busy input untouched.
    pub(crate) fn watch_input_ready(&self, terminal: &str) -> Result<bool> {
        match self.wait_for_input(terminal, Duration::from_millis(100)) {
            Ok((Waited::Ready, _)) => Ok(true),
            Ok(_) => Ok(false),
            Err(error) if error.downcast_ref::<IdleTimeout>().is_some() => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// A saved request key binds the exact text and terminal incarnation.
    /// Repeating this observes/replays that mutation, never another input.
    pub(crate) fn send_watch_round(
        &self,
        terminal: &str,
        line: &str,
        request: &str,
    ) -> Result<Value> {
        orca(&[
            "terminal",
            "send",
            "--terminal",
            terminal,
            "--text",
            line,
            "--enter",
            "--retry-request",
            request,
        ])
    }

    /// Closes `terminal` alone, of the terminals here.
    pub fn close_terminal(&self, terminal: &str) -> Result<()> {
        orca(&["terminal", "close", "--terminal", terminal])?;
        Ok(())
    }

    /// When the worker here was last handed its work by the fleet
    /// (`stopped::silence`): started, or — `since` — whatever it was handed
    /// after that.
    pub fn handed(&self, since: Option<u64>) -> Result<Option<u64>> {
        let started = std::fs::read_to_string(self.note(START_FILE)?).ok();
        Ok(started
            .and_then(|at| at.trim().parse().ok())
            .max(since)
            .max(crate::turn_answer::handed(self)?))
    }

    /// Waits until the agent in `terminal` is ready for a prompt. A trust
    /// question names a gap in the launch configuration; it is never answered
    /// at the terminal, where the worker's brief could become the answer.
    /// `command` is the line given to `start`, including network settings;
    /// its `niced` wrapper is derived from the same worktree-owned TMPDIR.
    pub fn await_ready(
        &self,
        terminal: &str,
        timeout_seconds: u64,
        harness: Harness,
        command: &str,
    ) -> Result<()> {
        let (ready, waited) =
            self.wait(terminal, Duration::from_secs(timeout_seconds)).with_context(|| {
                self.prompt_context(terminal, "cannot confirm the worker is ready for its brief")
            })?;
        match ready {
            Waited::Ready => Ok(()),
            Waited::AtTrustPrompt => Err(TrustGap {
                harness,
                command: startup_redacted(&niced(
                    &worker_tmp::of(&self.path).display().to_string(),
                    command,
                )),
                worktree: self.path.display().to_string(),
                terminal: terminal.to_string(),
                seconds: timeout_seconds,
                startup_wait: waited.evidence(),
                startup_screen: match Asked::of(&[
                    "terminal",
                    "read",
                    "--terminal",
                    terminal,
                    "--screen",
                    "--limit",
                    "200",
                ]) {
                    Ok(read) => read.evidence(),
                    Err(error) => startup_redacted(&format!("screen read failed: {error:#}")),
                },
            }
            .into()),
            Waited::NotReady => Err(self.prompt_failure(
                terminal,
                &format!("the worker was not ready for a prompt within {timeout_seconds}s"),
            )),
        }
    }

    /// One wait for the agent in `terminal`. orca exits non-zero on a wait it
    /// answered but that was not satisfied: what it answered is still read.
    fn wait(&self, terminal: &str, within: Duration) -> Result<(Waited, Asked)> {
        let timeout = within.as_millis().max(1).to_string();
        let asked = Asked::of(&[
            "terminal",
            "wait",
            "--terminal",
            terminal,
            "--for",
            "tui-idle",
            "--timeout-ms",
            &timeout,
        ])?;
        let wait = &asked.answer["result"]["wait"];
        if asked.answer["ok"] != true || !wait.is_object() {
            if asked.answer["error"]["code"] == "timeout" {
                return Err(IdleTimeout.into());
            }
            return Err(asked.failure());
        }
        let ready = if wait["blockedReason"] == TRUST_PROMPT {
            Waited::AtTrustPrompt
        } else if wait["satisfied"] == true && wait["blockedReason"].is_null() {
            Waited::Ready
        } else {
            Waited::NotReady
        };
        Ok((ready, asked))
    }

    /// A quiet screen can belong to a working harness: Codex reports a
    /// startup episode even with its animations off. Read its state afresh,
    /// outside the tick's cached observation, until it leaves that episode.
    /// Both checks share one bound; a harness with no report uses tui-idle.
    /// The agent of that read is the one the caller already paid for.
    fn wait_for_input(&self, terminal: &str, within: Duration) -> Result<(Waited, Value)> {
        let began = Instant::now();
        loop {
            let remaining = within.saturating_sub(began.elapsed());
            if remaining.is_zero() {
                return Ok((Waited::NotReady, Value::Null));
            }
            let (ready, _) = self.wait(terminal, remaining)?;
            if ready != Waited::Ready {
                return Ok((ready, Value::Null));
            }
            let row = runtime_row(&runtime_live()?, &self.path);
            let agent = if row["liveTerminalCount"].as_u64().unwrap_or(0) > 1 {
                // A watch shares its directory with the operator. A busy
                // session beside it cannot hold back this terminal's input.
                let shown = orca(&["terminal", "show", "--terminal", terminal])?;
                pane_agent(&row, &shown["terminal"])
            } else {
                main_agent(&row)
            };
            let state = agent["programStatus"]["state"].as_str().or(agent["state"].as_str());
            if state != Some("working") {
                return Ok((Waited::Ready, agent));
            }
            std::thread::sleep(INPUT_POLL.min(within.saturating_sub(began.elapsed())));
        }
    }

    /// A failed delivery includes the current screen, even when no known
    /// rule names its menu. An unreadable screen cannot hide the failure.
    fn prompt_failure(&self, terminal: &str, reason: &str) -> anyhow::Error {
        PromptBlocked(self.prompt_context(terminal, reason)).into()
    }

    fn prompt_context(&self, terminal: &str, reason: &str) -> String {
        let screen = match self.screen(terminal, 20) {
            Ok(lines) => lines.join("\n"),
            Err(error) => format!("screen unavailable: {error:#}"),
        };
        format!(
            "{reason}; worktree {} (terminal {terminal})\nScreen's last lines:\n{screen}",
            self.path.display()
        )
    }

    /// Types one line only at the harness's input, then requires that
    /// harness's session to show this submission started a turn. A line the
    /// terminal did not accept is a failed delivery, and the session is not
    /// read for it. The screen's last lines stay on the error as evidence.
    pub fn tell(&self, terminal: &str, line: &str) -> Result<()> {
        self.tell_at(terminal, line, crate::clock())
    }

    pub(crate) fn clear_turn(&self) -> Result<()> {
        self.forget(TURN_FILE)
    }

    pub fn tell_at(&self, terminal: &str, line: &str, now: u64) -> Result<()> {
        let earliest = crate::clock().saturating_sub(2);
        crate::harness_down::tell(self, terminal, now, || {
            self.tell_unchecked(terminal, line, 1, None, earliest)
        })?;
        crate::turn_answer::forget_wait(self)
    }

    fn tell_unchecked(
        &self,
        terminal: &str,
        line: &str,
        settle_seconds: u64,
        harness: Option<Harness>,
        earliest: u64,
    ) -> Result<()> {
        let (ready, agent) = self
            .wait_for_input(terminal, Duration::from_secs(settle_seconds))
            .with_context(|| {
                self.prompt_context(
                    terminal,
                    "no running worker harness confirmed; no line was sent",
                )
            })?;
        if ready != Waited::Ready {
            return Err(self.prompt_failure(
                terminal,
                "no running worker harness at its input; no line was sent",
            ));
        }
        // The input check already read this agent. Asking again would be a
        // second `worktree ps` before the line is sent.
        let harness = match harness {
            Some(harness) => harness,
            None => agent_harness(&agent).or(self.recorded_harness()?).unwrap_or(Harness::DEFAULT),
        };
        let sent = orca(&["terminal", "send", "--terminal", terminal, "--text", line, "--enter"]);
        let sent = match sent {
            Err(error) => {
                return if self.saw_turn(harness, earliest)? { Ok(()) } else { Err(error) };
            }
            Ok(sent) => sent,
        };
        if sent["send"]["accepted"] != true {
            return Err(PromptBlocked(
                sent["send"]["blockedReason"]
                    .as_str()
                    .unwrap_or("terminal did not accept the line")
                    .to_string(),
            )
            .into());
        }
        if self.prove(harness, earliest)? {
            return Ok(());
        }
        let prompt = &sent["send"]["prompt"];
        Err(self.prompt_failure(
            terminal,
            &format!(
                "the line started no turn within {}s; delivery receipt: {prompt}",
                turn_wait_seconds()
            ),
        ))
    }

    /// Hands the worker in `terminal` the brief at `brief`.
    pub fn deliver(&self, terminal: &str, brief: &Path) -> Result<()> {
        // The harness its start recorded: how the brief is typed is its own.
        let harness = match self.recorded_harness()? {
            Some(harness) => harness,
            None => self.harness()?.unwrap_or(Harness::DEFAULT),
        };
        let line = harness.brief_line(&brief.display().to_string());
        // A retry of the brief counts the turn the first send already started.
        let earliest = self.handed(None)?.unwrap_or_else(|| crate::clock().saturating_sub(2));
        crate::harness_down::tell(self, terminal, crate::clock(), || {
            self.tell_unchecked(terminal, &line, BRIEF_SETTLE_SECONDS, Some(harness), earliest)
        })?;
        // What a watch starts on first; a record it cannot keep loses no
        // worker its brief.
        let _ = crate::health::delivered(self);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_done_agent_with_no_live_pty_has_ended_and_one_with_a_pty_has_not() {
        let ended = serde_json::json!({"state": "done", "mainAgent": {"state": "done"}});
        assert!(!super::agent_running(&ended), "orca's leftover row of a session gone");
        let idle = serde_json::json!({"state": "done", "hasLivePty": true});
        assert!(super::agent_running(&idle), "a session at its prompt still writes");
        assert!(super::agent_running(&serde_json::json!({"state": "working"})));
        assert!(!super::agent_running(&serde_json::json!({"state": "exited"})));
    }

    use super::*;

    #[test]
    fn every_lone_surrogate_in_an_answer_becomes_one_replacement_character() {
        for unit in 0xd800..=0xdfff {
            let text = format!(r#"{{"ok":true,"result":{{"text":"\u{unit:04x}"}}}}"#);
            assert_eq!(
                read_answer(text.as_bytes()).unwrap(),
                serde_json::json!({"ok": true, "result": {"text": "�"}}),
                "{text}"
            );
        }
    }

    #[test]
    fn repairing_an_answer_keeps_pairs_keys_nesting_and_literal_escapes() {
        let text = r#"{"ok":true,"result":{"\uD800key":["⏺\udf13","\ud83c\udf13","\ud800\ud800\udc00","\udfff\uDBFF\uDFFF","literal: \\udf13","quote: \"\udf17","slash: \\\udf13"]},"error":null}"#;
        assert_eq!(
            read_answer(text.as_bytes()).unwrap(),
            serde_json::json!({
                "ok": true, "result": {"�key": ["⏺�", "🌓", "�𐀀", "�\u{10ffff}",
                    r"literal: \udf13", "quote: \"�", "slash: \\�"]}, "error": null
            })
        );
        for high in [0xd800_u32, 0xda00, 0xdbff] {
            for low in [0xdc00_u32, 0xdd00, 0xdfff] {
                let text = format!(r#"["\u{high:04X}\u{low:04x}","\udf13"]"#);
                let character =
                    char::from_u32(0x10000 + ((high - 0xd800) << 10) + low - 0xdc00).unwrap();
                assert_eq!(
                    read_answer(text.as_bytes()).unwrap(),
                    serde_json::json!([character.to_string(), "�"]),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn reading_an_answer_keeps_valid_json_and_rejects_other_malformed_json() {
        let text = br#"{"ok":false,"error":{"code":"blocked","details":{"reason":"\ud83c\udf13"}},"result":[null,true,false,-12,1.25,18446744073709551615,"\\ud800"]}"#;
        assert_eq!(read_answer(text).unwrap(), serde_json::from_slice::<Value>(text).unwrap());
        for text in [
            "not JSON",
            r#"{"ok":true,"text":"\udf13",}"#,
            r#"{"ok":true,"text":"\udf13\q"}"#,
            r#"{"text":"\ud800\u12"}"#,
            r#"{"text":"\u+800"}"#,
            r#"{"text":"\uD8XZ"}"#,
            r#"{"text":"\udf13}"#,
            r#"{"text":"\udf13"} trailing"#,
            r#"{"text":\udf13}"#,
            "\"\\udf13\n\"",
            "\"\\udf13\\",
            "[\"\\udf13\"",
        ] {
            assert!(read_answer(text.as_bytes()).is_err(), "accepted malformed JSON: {text}");
        }
        assert!(read_answer(b"\"\\udf13\xff\"").is_err());
    }

    #[test]
    fn every_worktree_of_the_host_is_listed_with_its_repo_and_whether_a_worker_holds_it() {
        let repos = serde_json::json!({"repos": [
            {"id": "r1", "path": "/src/widgets"}, {"id": "r2", "path": "/src/gadgets"}]});
        let listed = serde_json::json!({"worktrees": [
            {"path": "/src/widgets", "repoId": "r1", "isMainWorktree": true},
            {"path": "/w/issue-7", "repoId": "r1", "linkedIssue": 7},
            {"path": "/w/issue-8", "repoId": "r1", "linkedIssue": 8},
            {"path": "/w/convoy-train", "repoId": "r1"},
            {"path": "/w/old", "repoId": "r2", "isArchived": true},
            {"path": "/w/issue-3", "repoId": "r2", "linkedIssue": 3}]});
        let ps = serde_json::json!({"worktrees": [
            {"path": "/w/issue-7", "liveTerminalCount": 1},
            {"path": "/w/issue-8", "liveTerminalCount": 0}]});
        let on = |path: &str, repo: &str, issue, fleets, held| OnHost {
            path: path.into(),
            repo: Some(repo.into()),
            issue,
            fleets,
            held,
        };
        assert_eq!(
            on_host(&repos, &listed, &ps),
            [
                on("/w/issue-7", "/src/widgets", Some(7), false, true),
                on("/w/issue-8", "/src/widgets", Some(8), false, false),
                on("/w/convoy-train", "/src/widgets", None, true, false),
                on("/w/issue-3", "/src/gadgets", Some(3), false, false),
            ]
        );
    }
}

//! Finds the commands a fleet cannot work without (`afkconvoy_core::tools`):
//! `git`, `gh` and `orca` on this process's `PATH`, which runs them — orca
//! also where Orca's SSH remote runtime puts its CLI (`reach_orca`) — a
//! harness and every login in a login shell, which is where a worker runs; and of an orca
//! that is there, whether it is running and knows the checkout a command is
//! about. It only looks: nothing is installed, started or added and nobody is
//! logged in.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use afkconvoy_core::tools::{ALWAYS, Found, ORCA_REMOTE_CLI, Tool, refusal, remote_host};
use anyhow::{Result, bail};

use crate::git::{REMOTE, Repo};
use crate::launch::{Launches, Proposed, ask_login_shell};
use crate::look::Reading;
use crate::orca;

/// What this host has of the commands a start needs.
pub struct Looked {
    /// Each tool needed, and what was found of it.
    pub found: Vec<(Tool, Found)>,
    /// The host `gh` was asked about: the one the repo's remote is on.
    pub host: Option<String>,
    /// Why an orca that is running could not say whether it knows this
    /// checkout: it is in `found` as not running, which is all a start can
    /// make of it.
    pub orca_unasked: Option<String>,
}

/// Looks for every command a start in the checkout `repo` needs, asks each
/// whether it is logged in, and orca whether it is running and knows `repo`.
/// `launch` is the worker launch command, when the start knows it.
pub fn look(repo: &Repo, launch: Option<&Proposed>) -> Looked {
    // No git, a remote that is a path: `gh` is asked about no host.
    let remote = repo.try_git(&["remote", "get-url", REMOTE]);
    let remote = remote.ok().filter(|ran| ran.ok).map(|ran| ran.stdout);
    let host = remote.as_deref().and_then(remote_host);

    let mut found = vec![];
    let mut orca_unasked = None;
    for tool in ALWAYS {
        let state = match tool.login_question(host) {
            _ if !on_path(tool.name()) => Found::Missing,
            None if tool == Tool::Orca => orca::found(repo).unwrap_or_else(|error| {
                orca_unasked = Some(format!("{error:#}"));
                Found::NotRunning
            }),
            None => Found::Ready,
            Some(question) => asked(&repo.dir, tool, &question),
        };
        found.push((tool, state));
    }
    found.extend(launch.and_then(harness));
    Looked { found, host: host.map(str::to_string), orca_unasked }
}

/// What this host has of the harness `launch` starts, when it runs that
/// harness by its own name (`Harness::runs_itself`): there where a worker is started,
/// and logged in there — asked in the checkout `launch` is for. `None`: a
/// wrapper, which is asked nothing.
pub fn harness(launch: &Proposed) -> Option<(Tool, Found)> {
    let harness = launch.harness();
    if !harness.runs_itself(launch.as_str()) {
        return None;
    }
    let tool = Tool::Harness(harness);
    let state = match tool.login_question(None) {
        _ if !launch.resolves() => Found::Missing,
        None => Found::Ready,
        Some(question) => asked(launch.dir(), tool, &question),
    };
    if state == Found::Ready
        && let Some(needed) = harness.needs().map(Tool::from)
    {
        let question = format!("command -v {}", needed.name());
        let (ok, _) = ask_login_shell(launch.dir(), &question).unwrap_or_default();
        if !ok {
            return Some((needed, Found::Missing));
        }
    }
    Some((tool, state))
}

/// What `tool` is found to be by its answer to `question`, in a login shell
/// started in `dir`.
fn asked(dir: &Path, tool: Tool, question: &str) -> Found {
    if tool.starts_workers() {
        // One doctor starts this program once when its first answer is ready.
        // A logged-out answer earns one more ask, which may renew a session;
        // every later reader of this question gets that second answer.
        return match crate::launch::once_per_program(tool.name(), question, || {
            let first = ask_login_shell(dir, question).unwrap_or_default();
            if tool.logged_in(first.0, &first.1) {
                first
            } else {
                crate::launch::ask_login_shell_again(dir, question).unwrap_or_default()
            }
        }) {
            crate::launch::Once::Ran(succeeded, printed) => classified(tool, succeeded, &printed),
            crate::launch::Once::Already => Found::Ready,
        };
    }
    if tool == Tool::Gh {
        // One answer for the life of this process (`GH_LOGIN`).
        return gh_found(dir, question);
    }
    let (succeeded, printed) = ask_login_shell(dir, question).unwrap_or_default();
    classified(tool, succeeded, &printed)
}

fn classified(tool: Tool, succeeded: bool, printed: &str) -> Found {
    if tool.logged_in(succeeded, printed) {
        Found::Ready
    } else {
        Found::LoggedOut(afkconvoy_core::tools::LoginAnswer::new(succeeded, printed))
    }
}

/// One answer to a `gh` login question. It stands for the process's life:
/// every checkout of one doctor asks the same question, and nobody changes
/// gh's login under a running runner. The question names the host, so two
/// hosts stay two answers.
pub(crate) static GH_LOGIN: Reading<Found> = Reading::per_process("gh login");

fn gh_found(dir: &Path, question: &str) -> Found {
    GH_LOGIN.read(question, || gh_found_once(dir, question))
}

fn gh_found_once(dir: &Path, question: &str) -> Found {
    // gh auth status itself spends quota, and looks up env-token users
    // through GraphQL. The common entry verifies an override's identity
    // before any GitHub effect; don't let this redundant probe block it.
    if crate::gh_quota::credential_present() {
        return Found::Ready;
    }
    let gh = crate::gh::GitHub::new(dir.to_path_buf(), crate::gh::Tracker::Own);
    if let Ok(scope) = crate::gh_quota::Scope::of(&gh)
        && quota_waits(&scope)
    {
        return Found::Ready;
    }
    let (succeeded, printed) = ask_login_shell(dir, question).unwrap_or_default();
    if !succeeded && let Ok(scope) = crate::gh_quota::Scope::of(&gh) {
        let _ = scope.observe(&gh);
        if quota_waits(&scope) {
            return Found::Ready;
        }
    }
    classified(Tool::Gh, succeeded, &printed)
}

fn quota_waits(scope: &crate::gh_quota::Scope) -> bool {
    ["core", "graphql", "secondary"].into_iter().any(|name| {
        scope.read(name).is_ok_and(|r| r.remaining == Some(0) && r.reset > crate::gh_quota::now())
    })
}

/// Checks that this host has every command a start in the checkout `repo`
/// needs, each logged in, and that orca is running and knows `repo`.
/// Of `launches`, the default harness's worker launch command is checked
/// here, and its harness looked for, when the start knows it: every other
/// harness's is `role::probe`'s, which reads the team that calls for them. An error names every fault at
/// once, each with the line a human runs to fix it — and a host kit that
/// cannot be read only once no command is missing: a host with no git or no
/// orca has no host kit yet either, and is told first what to install.
pub fn check(repo: &Repo, launches: &Launches) -> Result<()> {
    let mut selected = None;
    let mut failures = vec![];
    if let Ok((kit, _)) = crate::kit::running() {
        for held in kit.harnesses {
            match crate::role::startable(launches, held.harness, "startup candidate") {
                Ok(()) => {
                    selected = Some(held.harness);
                    break;
                }
                Err(e) => failures.push(format!("{e:#}")),
            }
        }
    }
    let default = selected.unwrap_or_else(|| launches.default());
    let read = launches.of(default);
    let launch = read.as_ref().ok().and_then(|launch| launch.as_ref());
    let looked = look(repo, launch);
    if let Some(why) = looked.orca_unasked {
        bail!(why);
    }
    let fault = launch.and_then(Proposed::fault);
    let fault =
        fault.map(|fault| launches.said(&fault, default, &[crate::role::BY_FIRST.to_string()]));
    if let Some(why) = refusal(&looked.found, looked.host.as_deref(), fault.as_deref()) {
        bail!(why);
    }
    if selected.is_none() && !failures.is_empty() {
        bail!("{}", failures.join("\n"));
    }
    read.map(|_| ())
}

/// Whether a directory of this process's `PATH` holds a program by this name.
pub fn on_path(name: &str) -> bool {
    found_on_path(name).is_some()
}

/// The program by this name the first directory of this process's `PATH`
/// that holds one holds.
fn found_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|file| runnable(file))
}

fn runnable(file: &Path) -> bool {
    std::fs::metadata(file)
        .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
}

/// Where Orca's SSH remote runtime puts its CLI's directory on this host:
/// under the home directory. `None`: no home.
fn orca_remote_cli() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(Path::new(&home).join(ORCA_REMOTE_CLI))
}

/// Where this process finds orca, and how: the first `orca` on its `PATH`,
/// which wins, or else the CLI Orca's SSH remote runtime put on this host,
/// which only the terminals Orca opens itself have on theirs. `None`:
/// neither.
pub fn orca_place() -> Option<(PathBuf, OrcaFrom)> {
    let remote = orca_remote_cli().map(|dir| dir.join(Tool::Orca.name()));
    match found_on_path(Tool::Orca.name()) {
        Some(found) if remote.as_ref().is_some_and(|remote| same_file(remote, &found)) => {
            Some((found, OrcaFrom::RemoteCli))
        }
        Some(found) => Some((found, OrcaFrom::Path)),
        None => {
            remote.filter(|remote| runnable(remote)).map(|remote| (remote, OrcaFrom::RemoteCli))
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// How this process found orca (`orca_place`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrcaFrom {
    /// On its `PATH`, put there by whoever started it.
    Path,
    /// Where Orca's SSH remote runtime puts its CLI (`ORCA_REMOTE_CLI`).
    RemoteCli,
}

/// Has every orca call of this process and of what it starts — workers,
/// worktrees, terminals, the watch, the asking session — go to the CLI of
/// Orca's SSH remote runtime, where no `orca` is on its `PATH` and that CLI
/// is on this host: its directory is put at the end of the `PATH`, so an
/// `orca` put on it later still wins. Called once, first thing, before this
/// process starts a thread.
pub fn reach_orca() {
    if on_path(Tool::Orca.name()) {
        return;
    }
    let Some(dir) = orca_remote_cli().filter(|dir| runnable(&dir.join(Tool::Orca.name()))) else {
        return;
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&path).filter(|dir| !dir.as_os_str().is_empty());
    let dirs = dirs.chain([dir]);
    if let Ok(path) = std::env::join_paths(dirs) {
        // SAFETY: called first thing in `main`, before any thread starts.
        unsafe { std::env::set_var("PATH", path) };
    }
}

//! orca, as a subprocess: it owns every worktree, branch and terminal a worker
//! lives in. Nothing here creates or removes one any other way.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use afkconvoy_core::casting::CASTER_WORKTREE;
use afkconvoy_core::cut;
use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::inspection::{self, Finding, Kind};
use afkconvoy_core::launch::{TrustGap, Waited};
use afkconvoy_core::line::niced;
use afkconvoy_core::prompt::brief_pointer;
use afkconvoy_core::question;
use afkconvoy_core::stopped::Nudged;
use afkconvoy_core::tools::{Found, add_checkout};
use afkconvoy_core::train::TRAIN_WORKTREE;
use afkconvoy_core::worker::{
    AgentReport, WorkerState, is_worktree_named, lead, short_branch, worker_state,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::Repo;
use crate::worker_tmp;

/// The worker's instructions, kept in the worktree's own git dir: never staged
/// by the worker's `git add -A`, gone when the worktree is.
const BRIEF_FILE: &str = "convoy-worker-prompt.md";
/// The train's tip its worker was last told about, and when, beside the brief.
const TOLD_FILE: &str = "convoy-train-told";
/// When the caster here was started (epoch seconds), and the issues it was
/// given, beside the brief.
const CASTER_FILE: &str = "convoy-caster";
/// When the worker here was nudged (epoch seconds), beside the brief.
const NUDGE_FILE: &str = "convoy-nudged";
/// When the worker here was first seen waiting on its own background work
/// (epoch seconds), beside the brief: gone once it is seen waiting on none.
const BACKGROUND_FILE: &str = "convoy-background";
/// When the worker here was started (epoch seconds), beside the brief.
const START_FILE: &str = "convoy-started";
/// Where a stage that writes nothing started here (`guard::WritesNothing`).
const WRITES_NOTHING_FILE: &str = "convoy-writes-nothing";
/// The comment of the `blocked` verdict a worker was last started again over.
const ANSWERED_FILE: &str = "convoy-answered";
/// What a bound repo's worker here was started under (`Under`).
const UNDER_FILE: &str = "convoy-started-under";
/// The harness the worker here was last started on, as its key.
const HARNESS_FILE: &str = "convoy-harness";
/// What was taken down as the worker here turned hot (`evidence`).
const EVIDENCE_DIR: &str = "convoy-hot";
/// What the inspection in this checkout found, as it said it.
const INSPECTED_FILE: &str = "convoy-inspected.json";
/// What the inspection in this checkout was started on: a hot worker, or the
/// host.
const INSPECTION_KIND_FILE: &str = "convoy-inspection-kind";
/// The PRs the inspection in this checkout put on the landing train.
const INSPECTION_PRS_FILE: &str = "convoy-inspection-prs";
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

/// One `orca … --json` call → its `result` object. An error when orca cannot
/// be run, exits non-zero, or answers `ok: false` — never an empty answer.
pub fn orca(args: &[&str]) -> Result<Value> {
    let asked = Asked::of(args)?;
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

impl Asked {
    fn of(args: &[&str]) -> Result<Asked> {
        let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let out = Command::new("orca")
            .args(args)
            .arg("--json")
            .current_dir(asked_from())
            .output()
            .with_context(|| format!("orca {what} could not run"))?;
        Ok(Asked {
            what,
            exited_ok: out.status.success(),
            answer: serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
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
}

/// Where orca is run from: the host user's home, or the root on a host with
/// none. What orca takes from where it is run — the worktree a call names
/// none of, which a question's terminal is opened in — is then never the
/// directory afkconvoy happens to run in, which no command is about
/// (docs/adr/0023).
fn asked_from() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.filter(|home| home.is_dir()).unwrap_or_else(|| PathBuf::from("/"))
}

/// What a question's terminal is titled, where orca shows it.
const QUESTION_TITLE: &str = "afkconvoy question";
/// How many lines of a question's terminal are read: more than any question
/// prints.
const QUESTION_LINES: &str = "10000";
/// How long a question is left between two reads of its terminal.
const QUESTION_POLL: Duration = Duration::from_millis(200);

/// Asks `script` of the login shell orca starts in a terminal of its own —
/// the shell a worker gets, with its rc files, PATH and logins — given
/// `timeout` to answer, and closes that terminal whatever came of it →
/// whether it exited zero, and what it printed (`question::answer`); `None`
/// when it did not answer in time. An error: orca made no terminal for it, or
/// could not read it. What is asked is of the host — its login shell, its
/// harnesses and their models, what it keeps for each — and of no one repo,
/// whichever a command is about: the terminal is opened where orca chooses,
/// as it is from the host user's home (`asked_from`), and never where the
/// command happens to run.
pub fn ask(script: &str, timeout: Duration) -> Result<Option<(bool, String)>> {
    ask_through(&orca, script, timeout)
}

/// `ask`, of the orca `orca` calls.
fn ask_through(
    orca: &dyn Fn(&[&str]) -> Result<Value>,
    script: &str,
    timeout: Duration,
) -> Result<Option<(bool, String)>> {
    let typed = question::typed(script);
    let made = orca(&["terminal", "create", "--title", QUESTION_TITLE, "--command", &typed])?;
    let handle = made["terminal"]["handle"].as_str();
    let handle =
        handle.ok_or_else(|| anyhow!("orca terminal create returned no terminal handle"))?;
    let started = Instant::now();
    let answered = loop {
        let read = orca(&["terminal", "read", "--terminal", handle, "--limit", QUESTION_LINES]);
        let lines = match read {
            Ok(read) => read["terminal"]["tail"].as_array().cloned().unwrap_or_default(),
            Err(error) => break Err(error),
        };
        let lines: Vec<String> =
            lines.iter().filter_map(|line| line.as_str().map(str::to_string)).collect();
        if let Some(answer) = question::answer(&lines) {
            break Ok(Some(answer));
        }
        if started.elapsed() >= timeout {
            break Ok(None);
        }
        std::thread::sleep(QUESTION_POLL);
    };
    // Soft: a terminal that is gone already is as wanted.
    let _ = orca(&["terminal", "close", "--terminal", handle]);
    answered
}

/// Whether orca is running. An orca that cannot say is not one a worker can
/// be started through, nor a question asked in.
pub fn running() -> bool {
    orca(&["status"]).is_ok_and(|status| status["runtime"]["state"] == "ready")
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
        let listed = orca(&["repo", "list"])?;
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

/// What a train worker was last told.
pub struct Told {
    pub tip: String,
    /// `None` for a note written before the time was kept.
    pub at: Option<u64>,
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
    /// The landing train's, the caster's, an inspection's or a cut's: the fleet's own.
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
                || is_worktree_named(name, inspection::CHECKOUT)
                || is_worktree_named(name, cut::CHECKOUT);
            Some(OnHost { issue: row["linkedIssue"].as_u64(), path, repo, fleets, held })
        })
        .collect()
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
        let listed = orca(&["worktree", "list"])?;
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
        let listed = orca(&["worktree", "list"])?;
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
    /// inspection: launch evidence applies to those sessions too.
    pub fn of_sessions(source: &Source) -> Result<Vec<(Option<u64>, Worktree)>> {
        let listed = orca(&["worktree", "list"])?;
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
        let repos = orca(&["repo", "list"])?;
        let listed = orca(&["worktree", "list"])?;
        let ps = orca(&["worktree", "ps", "--limit", PS_LIMIT])?;
        Ok(on_host(&repos, &listed, &ps))
    }

    /// Whether this is the landing train's worktree, the caster's, an
    /// inspection's or a cut's: the fleet's own, and no worker's.
    fn is_fleets(&self) -> bool {
        let name = self.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        is_worktree_named(name, TRAIN_WORKTREE)
            || is_worktree_named(name, CASTER_WORKTREE)
            || is_worktree_named(name, inspection::CHECKOUT)
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
        let listed = orca(&["worktree", "list"])?;
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

    /// What the worker here is doing, as its runtime reported it to orca. A
    /// page of `ps` that was cut short is an error: an orca that cannot say is
    /// never "no worker there".
    pub fn state(&self, now: u64) -> Result<WorkerState> {
        let ps = orca(&["worktree", "ps", "--limit", PS_LIMIT])?;
        if ps["truncated"] == true {
            bail!("orca worktree ps was cut short at {PS_LIMIT} rows");
        }
        let rows = ps["worktrees"].as_array().cloned().unwrap_or_default();
        let row = rows.iter().find(|row| {
            row["path"].as_str().is_some_and(|path| same_dir(Path::new(path), &self.path))
        });
        let Some(row) = row else { return Ok(WorkerState::Gone) };
        if row["liveTerminalCount"].as_u64().unwrap_or(0) > 0 && crate::health::has_signal(self)? {
            return Ok(WorkerState::Idle { quiet: None });
        }
        // orca's clocks are epoch milliseconds
        let seconds = |stamp: &Value| stamp.as_u64().map(|ms| ms / 1000);
        let agents: Vec<AgentReport> = row["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|agent| agent["state"].is_string() && agent["parentPaneKey"].is_null())
            .map(|agent| AgentReport {
                working: agent["state"] == "working",
                since: seconds(&agent["stateStartedAt"]),
            })
            .collect();
        let terminals = row["liveTerminalCount"].as_u64().unwrap_or(0);
        Ok(worker_state(terminals, seconds(&row["lastOutputAt"]), lead(&agents), now))
    }

    /// Actual structured runtime observations for diagnosis. A transport
    /// failure is an error, never evidence that an agent has stopped.
    pub fn health_report(&self) -> Result<Value> {
        let ps = orca(&["worktree", "ps", "--limit", PS_LIMIT])?;
        if ps["truncated"] == true {
            bail!("cannot diagnose from truncated worktree ps");
        }
        let rows = ps["worktrees"].as_array().context("orca worktree ps returned no rows")?;
        Ok(rows
            .iter()
            .find(|row| row["path"].as_str().is_some_and(|p| same_dir(Path::new(p), &self.path)))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// The terminal a worker here would be in: the one that wrote last.
    pub fn terminal(&self) -> Result<Option<String>> {
        let listed = orca(&["terminal", "list", "--worktree", &self.selector()])?;
        let rows = listed["terminals"].as_array().cloned().unwrap_or_default();
        let last = rows.iter().max_by_key(|row| row["lastOutputAt"].as_u64().unwrap_or(0));
        Ok(last.and_then(|row| row["handle"].as_str()).map(str::to_string))
    }

    /// A note about the worker here, kept in the worktree's own git dir: this
    /// machine's, like the worker itself, and gone when the worktree is. git
    /// is asked where that is once.
    pub(crate) fn note(&self, name: &str) -> Result<PathBuf> {
        if self.git_dir.get().is_none() {
            let dir = Repo::new(self.path.clone()).git(&["rev-parse", "--absolute-git-dir"])?;
            let _ = self.git_dir.set(PathBuf::from(dir));
        }
        Ok(self.git_dir.get().expect("set above").join(name))
    }

    /// What the train worker here was last told: the train's tip, and when.
    pub fn told(&self) -> Result<Option<Told>> {
        let told = std::fs::read_to_string(self.note(TOLD_FILE)?).unwrap_or_default();
        let mut words = told.split_whitespace();
        Ok(words.next().map(|tip| Told {
            tip: tip.to_string(),
            at: words.next().and_then(|at| at.parse().ok()),
        }))
    }

    /// Records what the train worker here was told at `at`. It is at work on
    /// something new: whatever nudge it had is no longer about this.
    pub fn record_told(&self, tip: &str, at: u64) -> Result<()> {
        self.forget(NUDGE_FILE)?;
        let file = self.note(TOLD_FILE)?;
        std::fs::write(&file, format!("{tip} {at}"))
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// When the caster here was started, and the issues it was given.
    pub fn caster(&self) -> Result<Option<(u64, Vec<u64>, String)>> {
        let kept = std::fs::read_to_string(self.note(CASTER_FILE)?).unwrap_or_default();
        if let Ok((since, given, context)) = serde_json::from_str(&kept) {
            return Ok(Some((since, given, context)));
        }
        let mut words = kept.split_whitespace().map(str::parse::<u64>);
        let Some(Ok(since)) = words.next() else { return Ok(None) };
        Ok(Some((since, words.filter_map(Result::ok).collect(), String::new())))
    }

    /// Records that the caster here was started at `since`, given `given`.
    pub fn record_caster(&self, since: u64, given: &[u64], context: &str) -> Result<()> {
        let file = self.note(CASTER_FILE)?;
        std::fs::write(&file, serde_json::to_vec(&(since, given, context))?)
            .with_context(|| format!("cannot write {}", file.display()))
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
        if self.terminal()?.is_none() {
            return Ok(());
        }
        orca(&["terminal", "close", "--worktree", &self.selector(), "--all"])?;
        Ok(())
    }

    /// End the old agent before a continuation can rewrite its brief or
    /// start another writer. Include descendants that escaped the terminal.
    pub(crate) fn stop_for_continuation(&self) -> Result<()> {
        let before = crate::hot::listed_checked()?;
        let pids: Vec<_> = crate::hot::tree(&before, &self.path).iter().map(|p| p.pid).collect();
        self.close_terminals()?;
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let processes = crate::hot::listed_checked()?;
            let writers = crate::hot::tree(&processes, &self.path);
            let old_alive = processes.iter().any(|p| pids.contains(&p.pid));
            let report = self.health_report()?;
            let agents = report["agents"].as_array();
            let agent_alive = agents.is_some_and(|agents| {
                agents.iter().any(|a| {
                    !matches!(a["state"].as_str(), Some("exited" | "stopped" | "closed" | "gone"))
                })
            });
            if self.terminal()?.is_none() && writers.is_empty() && !old_alive && !agent_alive {
                return Ok(());
            }
            if Instant::now() >= until {
                bail!(
                    "old worker termination is unconfirmed in {}; no replacement was started",
                    self.path.display()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
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
        self.forget(TOLD_FILE)?;
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

    /// When the worker here was first seen waiting on its own background
    /// work — `waiting`, as its screen shows now — recorded `now` when it
    /// was not; `None`, and the record gone, when it waits on none.
    pub fn background_since(&self, waiting: bool, now: u64) -> Result<Option<u64>> {
        if !waiting {
            self.forget(BACKGROUND_FILE)?;
            return Ok(None);
        }
        let file = self.note(BACKGROUND_FILE)?;
        let seen = std::fs::read_to_string(&file).ok();
        if let Some(since) = seen.and_then(|at| at.trim().parse().ok()) {
            return Ok(Some(since));
        }
        std::fs::write(&file, now.to_string())
            .with_context(|| format!("cannot write {}", file.display()))?;
        Ok(Some(now))
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

    /// What the inspection in this checkout found, when it said.
    pub fn inspected(&self) -> Result<Option<Finding>> {
        let said = std::fs::read_to_string(self.note(INSPECTED_FILE)?).unwrap_or_default();
        Ok(serde_json::from_str(&said).ok())
    }

    /// What the inspection in this checkout was started on; a hot worker
    /// where nothing says.
    pub fn inspection_kind(&self) -> Result<Kind> {
        let said = std::fs::read_to_string(self.note(INSPECTION_KIND_FILE)?).unwrap_or_default();
        Ok(if said.trim() == Kind::Host.word() { Kind::Host } else { Kind::Worker })
    }

    /// Records what the inspection in this checkout is started on.
    pub fn record_inspection_kind(&self, kind: Kind) -> Result<()> {
        let file = self.note(INSPECTION_KIND_FILE)?;
        std::fs::write(&file, kind.word())
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// The PRs the inspection in this checkout put on the landing train,
    /// oldest first.
    pub fn inspection_prs(&self) -> Result<Vec<u64>> {
        let kept = std::fs::read_to_string(self.note(INSPECTION_PRS_FILE)?).unwrap_or_default();
        Ok(kept.split_whitespace().filter_map(|pr| pr.parse().ok()).collect())
    }

    /// Records that the inspection in this checkout put `pr` on the landing
    /// train.
    pub fn record_inspection_pr(&self, pr: u64) -> Result<()> {
        let mut prs = self.inspection_prs()?;
        if !prs.contains(&pr) {
            prs.push(pr);
        }
        let file = self.note(INSPECTION_PRS_FILE)?;
        let kept: Vec<String> = prs.iter().map(u64::to_string).collect();
        std::fs::write(&file, kept.join("\n"))
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// Keeps what the inspection in this checkout found: the last it said stands.
    pub fn record_inspected(&self, finding: &Finding) -> Result<()> {
        let file = self.note(INSPECTED_FILE)?;
        std::fs::write(&file, serde_json::to_string(finding)?)
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// What the cut in this checkout is and did so far, where it says; `None`
    /// where nothing does, or what does cannot be read.
    pub fn cut_record(&self) -> Result<Option<cut::Record>> {
        let said = std::fs::read_to_string(self.note(CUT_FILE)?).unwrap_or_default();
        Ok(serde_json::from_str(&said).ok())
    }

    /// Keeps what the cut in this checkout is and did so far: the last it
    /// said stands. It is written whole, then put in place: a tick reading it
    /// meanwhile reads the one before, never half of this one.
    pub fn record_cut(&self, record: &cut::Record) -> Result<()> {
        let file = self.note(CUT_FILE)?;
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

    /// The harness the worker here was last started on, where a start kept it.
    pub fn harness(&self) -> Result<Option<Harness>> {
        let kept = std::fs::read_to_string(self.note(HARNESS_FILE)?).unwrap_or_default();
        Ok(Harness::read(kept.trim()))
    }

    /// Keeps the harness the worker here is started on.
    pub fn keep_harness(&self, harness: Harness) -> Result<()> {
        let file = self.note(HARNESS_FILE)?;
        std::fs::write(&file, harness.key())
            .with_context(|| format!("cannot write {}", file.display()))
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
    pub fn remove(&self) -> Result<()> {
        // One that cannot be turned costs a server the worker started its
        // directory, and no worktree is kept for that.
        let _ = worker_tmp::retire(&self.path);
        orca(&["worktree", "rm", "--worktree", &self.selector(), "--force"])?;
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
    pub fn start(&self, command: &str, now: u64) -> Result<String> {
        // A failed close must not turn a continuation into a second writer.
        // Check again even after an acknowledged close: Orca may still be
        // ending the previous terminal. Leave the worker's progress markers
        // intact until its replacement can actually be started.
        if self.harness()?.is_some() {
            self.stop_for_continuation()?;
        } else {
            self.close_terminals()?;
        }
        if self.terminal()?.is_some() {
            bail!(
                "the previous worker in {} still has a terminal after close; \
                 no replacement was started",
                self.path.display()
            );
        }
        self.forget(NUDGE_FILE)?;
        self.forget(BACKGROUND_FILE)?;
        let command = niced(&worker_tmp::make(&self.path)?.display().to_string(), command);
        let made =
            orca(&["terminal", "create", "--worktree", &self.selector(), "--command", &command])?;
        let handle = made["terminal"]["handle"].as_str();
        let handle =
            handle.ok_or_else(|| anyhow!("orca terminal create returned no terminal handle"))?;
        self.forget(crate::health::SIGNAL_FILE)?;
        let file = self.note(START_FILE)?;
        std::fs::write(&file, now.to_string())
            .with_context(|| format!("cannot write {}", file.display()))?;
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
        Ok(started.and_then(|at| at.trim().parse().ok()).max(since))
    }

    /// Waits until the agent in `terminal` is ready for a prompt. A trust
    /// question names a gap in the launch configuration; it is never answered
    /// at the terminal, where the worker's brief could become the answer.
    pub fn await_ready(
        &self,
        terminal: &str,
        timeout_seconds: u64,
        harness: Harness,
        command: &str,
    ) -> Result<()> {
        let ready = self.wait(terminal, timeout_seconds).with_context(|| {
            self.prompt_context(terminal, "cannot confirm the worker is ready for its brief")
        })?;
        match ready {
            Waited::Ready => Ok(()),
            Waited::AtTrustPrompt => Err(TrustGap {
                harness,
                command: command.to_string(),
                worktree: self.path.display().to_string(),
                terminal: terminal.to_string(),
                seconds: timeout_seconds,
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
    fn wait(&self, terminal: &str, timeout_seconds: u64) -> Result<Waited> {
        let timeout = (timeout_seconds * 1000).to_string();
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
            return Err(asked.failure());
        }
        Ok(if wait["blockedReason"] == TRUST_PROMPT {
            Waited::AtTrustPrompt
        } else if wait["satisfied"] == true && wait["blockedReason"].is_null() {
            Waited::Ready
        } else {
            Waited::NotReady
        })
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

    /// Types one line only at the harness's input, then requires Orca's
    /// receipt to prove that this submission started a turn. Input acceptance
    /// alone is no delivery. A failed line is kept for worker health.
    pub fn tell(&self, terminal: &str, line: &str) -> Result<()> {
        self.tell_at(terminal, line, crate::clock())
    }

    pub fn tell_at(&self, terminal: &str, line: &str, now: u64) -> Result<()> {
        crate::harness_down::tell(self, terminal, now, || self.tell_unchecked(terminal, line))
    }

    fn tell_unchecked(&self, terminal: &str, line: &str) -> Result<()> {
        let ready = self.wait(terminal, 1).with_context(|| {
            self.prompt_context(terminal, "no running worker harness confirmed; no line was sent")
        })?;
        if ready != Waited::Ready {
            return Err(self.prompt_failure(
                terminal,
                "no running worker harness at its input; no line was sent",
            ));
        }
        let timeout = SUBMIT_SECONDS.to_string();
        let sent = orca(&[
            "terminal",
            "send",
            "--terminal",
            terminal,
            "--text",
            line,
            "--enter",
            "--wait-submit",
            &timeout,
        ])?;
        if sent["send"]["accepted"] != true {
            return Err(PromptBlocked(
                sent["send"]["blockedReason"]
                    .as_str()
                    .unwrap_or("terminal did not accept the line")
                    .to_string(),
            )
            .into());
        }
        let prompt = &sent["send"]["prompt"];
        if !prompt["stages"]
            .as_array()
            .is_some_and(|stages| stages.iter().any(|stage| stage == "turn_started"))
        {
            return Err(self.prompt_failure(
                terminal,
                &format!(
                    "the line started no turn within {SUBMIT_SECONDS}s; delivery receipt: {prompt}"
                ),
            ));
        }
        Ok(())
    }

    /// Hands the worker in `terminal` the brief at `brief`.
    pub fn deliver(&self, terminal: &str, brief: &Path) -> Result<()> {
        self.tell(terminal, &brief_pointer(&brief.display().to_string()))?;
        // What a watch starts on first; a record it cannot keep loses no
        // worker its brief.
        let _ = crate::health::delivered(self);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An orca whose question terminal shows `shown` at every read, after the
    /// first: each call is kept in `calls`.
    fn orca_showing<'a>(
        calls: &'a std::cell::RefCell<Vec<String>>,
        shown: &'a [&'a str],
    ) -> impl Fn(&[&str]) -> Result<Value> + 'a {
        move |args: &[&str]| {
            calls.borrow_mut().push(args.join(" "));
            Ok(match args {
                ["terminal", "create", ..] => serde_json::json!({"terminal": {"handle": "t1"}}),
                ["terminal", "read", ..] if calls.borrow().len() > 2 => {
                    serde_json::json!({"terminal": {"tail": shown}})
                }
                _ => serde_json::json!({"terminal": {"tail": []}}),
            })
        }
    }

    #[test]
    fn a_question_is_read_until_it_answers_and_its_terminal_closed() {
        let calls = std::cell::RefCell::new(vec![]);
        let shown = ["rc noise", "AFKCONVOY_ASKED", "OK", "", "AFKCONVOY_ANSWERED 0"];
        let orca = orca_showing(&calls, &shown);
        let answer = ask_through(&orca, "pi --print hi", Duration::from_secs(5));
        assert_eq!(answer.unwrap(), Some((true, "OK".to_string())));
        let calls = calls.borrow();
        assert!(calls[0].starts_with("terminal create --title afkconvoy question --command "));
        assert!(calls[0].contains("( pi --print hi )"), "{}", calls[0]);
        // Of the host, it is asked in no repo's worktree.
        assert!(!calls[0].contains("--worktree"), "{}", calls[0]);
        assert_eq!(calls.iter().filter(|call| call.starts_with("terminal read")).count(), 2);
        assert_eq!(calls.last().unwrap(), "terminal close --terminal t1");
    }

    #[test]
    fn a_question_that_does_not_answer_in_time_is_none_and_its_terminal_closed() {
        let calls = std::cell::RefCell::new(vec![]);
        let orca = orca_showing(&calls, &["AFKCONVOY_ASKED", "thinking"]);
        let answer = ask_through(&orca, "codex exec hi", Duration::from_millis(300));
        assert_eq!(answer.unwrap(), None);
        let calls = calls.borrow();
        assert_eq!(calls.last().unwrap(), "terminal close --terminal t1");
    }

    #[test]
    fn a_question_orca_made_no_terminal_for_is_an_error() {
        let refused = |_: &[&str]| -> Result<Value> { Err(anyhow!("orca terminal create failed")) };
        let error = ask_through(&refused, "pi", Duration::from_secs(1)).unwrap_err();
        assert_eq!(error.to_string(), "orca terminal create failed");
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

//! orca, as a subprocess: it owns every worktree, branch and terminal a worker
//! lives in. Nothing here creates or removes one any other way.

use std::path::{Path, PathBuf};
use std::process::Command;

use afkconvoy_core::guard::WritesNothing;
use afkconvoy_core::launch::{Waited, until_ready};
use afkconvoy_core::prompt::brief_pointer;
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

/// The worker's instructions, kept in the worktree's own git dir: never staged
/// by the worker's `git add -A`, gone when the worktree is.
const BRIEF_FILE: &str = "convoy-worker-prompt.md";
/// The train's tip its worker was last told about, and when, beside the brief.
const TOLD_FILE: &str = "convoy-train-told";
/// When the worker here was nudged (epoch seconds), beside the brief.
const NUDGE_FILE: &str = "convoy-nudged";
/// Where a stage that writes nothing started here (`guard::WritesNothing`).
const WRITES_NOTHING_FILE: &str = "convoy-writes-nothing";
/// The commit a patrol's checkout was at when its worker was started.
const STARTED_FILE: &str = "convoy-patrol-started";
/// The comment of the `blocked` verdict a worker was last started again over.
const ANSWERED_FILE: &str = "convoy-answered";
/// What a bound repo's worker here was started under (`Under`).
const UNDER_FILE: &str = "convoy-started-under";
/// More worktrees than one machine runs workers in.
const PS_LIMIT: &str = "1000";
/// What orca says holds up a worker asking whether its folder is trusted.
const TRUST_PROMPT: &str = "agent-trust-workspace";
/// The Down key: from that question's default, "No, exit", to "Yes, I trust
/// this folder".
const DOWN: &str = "\x1b[B";

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

impl Asked {
    fn of(args: &[&str]) -> Result<Asked> {
        let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let out = Command::new("orca")
            .args(args)
            .arg("--json")
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
        let said = match self.answer["error"]["code"].as_str() {
            Some(code) => code,
            None if self.stderr.is_empty() => &self.stdout,
            None => &self.stderr,
        };
        anyhow!("orca {} failed: {said}", self.what)
    }
}

/// What a start finds of an installed orca: whether it is running, and then
/// whether it knows this checkout. It only asks.
pub fn found() -> Result<Found> {
    // An orca that cannot say is not one a worker can be started through.
    let ready = orca(&["status"]).is_ok_and(|status| status["runtime"]["state"] == "ready");
    if !ready {
        return Ok(Found::NotRunning);
    }
    // No git, no checkout: another fault's to name.
    let Ok(main) = Repo::here().and_then(|repo| repo.main_checkout()) else {
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

/// A worker's worktree on this machine.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    /// The branch orca named for it — never assumed from the worktree's name.
    pub branch: String,
}

impl Worktree {
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
        let checkout = Repo { dir: path.clone() };
        if !checkout.try_git(&["merge-base", "--is-ancestor", sha, "HEAD"])?.ok {
            checkout.git(&["merge", "--quiet", "--ff-only", sha])?;
        }
        Ok(Worktree { path, branch })
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
            Some(Worktree {
                path: PathBuf::from(row["path"].as_str()?),
                branch: short_branch(row["branch"].as_str().unwrap_or("")).to_string(),
            })
        }))
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
            Some(Worktree {
                path: PathBuf::from(row["path"].as_str()?),
                branch: short_branch(row["branch"].as_str().unwrap_or("")).to_string(),
            })
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

    /// The terminal a worker here would be in: the one that wrote last.
    pub fn terminal(&self) -> Result<Option<String>> {
        let listed = orca(&["terminal", "list", "--worktree", &self.selector()])?;
        let rows = listed["terminals"].as_array().cloned().unwrap_or_default();
        let last = rows.iter().max_by_key(|row| row["lastOutputAt"].as_u64().unwrap_or(0));
        Ok(last.and_then(|row| row["handle"].as_str()).map(str::to_string))
    }

    /// A note about the worker here, kept in the worktree's own git dir: this
    /// machine's, like the worker itself, and gone when the worktree is.
    fn note(&self, name: &str) -> Result<PathBuf> {
        let git_dir = Repo { dir: self.path.clone() }.git(&["rev-parse", "--absolute-git-dir"])?;
        Ok(Path::new(&git_dir).join(name))
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

    /// Clears the worktree and keeps it: its terminals closed, what it held
    /// discarded for the commit `sha`, what its worker was told forgotten.
    /// What git ignores — everything a gate run built — stays.
    pub fn clear(&self, sha: &str) -> Result<()> {
        // One with no terminal is as wanted.
        let _ = orca(&["terminal", "close", "--worktree", &self.selector(), "--all"]);
        let repo = Repo { dir: self.path.clone() };
        repo.git(&["reset", "--quiet", "--hard", sha])?;
        repo.git(&["clean", "--quiet", "-fd"])?;
        self.forget(TOLD_FILE)?;
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

    pub fn record_nudge(&self, at: u64) -> Result<()> {
        let file = self.note(NUDGE_FILE)?;
        std::fs::write(&file, at.to_string())
            .with_context(|| format!("cannot write {}", file.display()))
    }

    /// The commit the branch here is at.
    pub fn tip(&self) -> Result<String> {
        Repo { dir: self.path.clone() }.git(&["rev-parse", "HEAD"])
    }

    /// The branch checked out here now; `None` on a detached HEAD.
    pub fn branch_now(&self) -> Result<Option<String>> {
        let named =
            Repo { dir: self.path.clone() }.try_git(&["symbolic-ref", "--short", "HEAD"])?;
        Ok(named.ok.then_some(named.stdout))
    }

    /// The commit a patrol's checkout here was at when its worker was started.
    pub fn started(&self) -> Result<Option<String>> {
        let kept = std::fs::read_to_string(self.note(STARTED_FILE)?).ok();
        Ok(kept.map(|tip| tip.trim().to_string()).filter(|tip| !tip.is_empty()))
    }

    /// Keeps the commit this checkout is at, as a patrol's worker starts in it.
    pub fn keep_started(&self) -> Result<()> {
        let file = self.note(STARTED_FILE)?;
        std::fs::write(&file, self.tip()?)
            .with_context(|| format!("cannot write {}", file.display()))
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
    /// which orca deletes unless it holds changes nothing has merged.
    pub fn remove(&self) -> Result<()> {
        orca(&["worktree", "rm", "--worktree", &self.selector(), "--force"])?;
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
    /// verbatim → its terminal's handle. Whatever terminal was here first is
    /// closed, so the worker's is the only one in the worktree: two agents in
    /// one worktree would fight, and orca opens a bare shell in every worktree
    /// it cuts. A new worker was never nudged, whatever the one before it was.
    pub fn start(&self, command: &str) -> Result<String> {
        self.forget(NUDGE_FILE)?;
        // One with no terminal is as wanted.
        let _ = orca(&["terminal", "close", "--worktree", &self.selector(), "--all"]);
        let made =
            orca(&["terminal", "create", "--worktree", &self.selector(), "--command", command])?;
        let handle = made["terminal"]["handle"].as_str();
        Ok(handle
            .ok_or_else(|| anyhow!("orca terminal create returned no terminal handle"))?
            .to_string())
    }

    /// Waits until the agent in `terminal` is ready for a prompt, trusting
    /// the worktree for it when it asks.
    pub fn await_ready(&self, terminal: &str, timeout_seconds: u64) -> Result<()> {
        let waited = until_ready(|| self.wait(terminal, timeout_seconds), || self.trust(terminal))?;
        let held_up = match waited {
            Waited::Ready => return Ok(()),
            Waited::AtTrustPrompt => "still asks whether to trust the folder, once answered",
            Waited::NotReady => "was not ready for a prompt",
        };
        bail!(
            "the worker in {} {held_up} within {timeout_seconds}s (terminal {terminal})",
            self.path.display()
        )
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
        Ok(if wait["satisfied"] == true {
            Waited::Ready
        } else if wait["blockedReason"] == TRUST_PROMPT {
            Waited::AtTrustPrompt
        } else {
            Waited::NotReady
        })
    }

    /// Answers the agent's trust question in `terminal`: yes. Two sends —
    /// typed in one, the Enter is read before the selection has moved, and
    /// picks "No, exit".
    fn trust(&self, terminal: &str) -> Result<()> {
        for keys in [["--text", DOWN].as_slice(), &["--enter"]] {
            let sent = orca(&[&["terminal", "send", "--terminal", terminal], keys].concat())?;
            if sent["send"]["accepted"] != true {
                bail!("terminal {terminal} did not accept the answer to its trust question");
            }
        }
        Ok(())
    }

    /// Types one line at the worker and submits it. Typed but unsubmitted, a
    /// worker sits idle forever, indistinguishable from one that finished.
    pub fn tell(&self, terminal: &str, line: &str) -> Result<()> {
        let sent = orca(&["terminal", "send", "--terminal", terminal, "--text", line, "--enter"])?;
        if sent["send"]["accepted"] != true {
            bail!("terminal {terminal} did not accept the line");
        }
        Ok(())
    }

    /// Hands the worker in `terminal` the brief at `brief`.
    pub fn deliver(&self, terminal: &str, brief: &Path) -> Result<()> {
        self.tell(terminal, &brief_pointer(&brief.display().to_string()))
    }
}

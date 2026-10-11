//! orca, as a subprocess: it owns every worktree, branch and terminal a worker
//! lives in. Nothing here creates or removes one any other way.

use std::path::{Path, PathBuf};
use std::process::Command;

use afkconvoy_core::prompt::brief_pointer;
use afkconvoy_core::stopped::Nudged;
use afkconvoy_core::train::is_train_worktree;
use afkconvoy_core::worker::{AgentReport, WorkerState, lead, short_branch, worker_state};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::git::Repo;

/// The worker's instructions, kept in the worktree's own git dir: never staged
/// by the worker's `git add -A`, gone when the worktree is.
const BRIEF_FILE: &str = "convoy-worker-prompt.md";
/// The train's tip its worker was last told about, beside the brief.
const TOLD_FILE: &str = "convoy-train-told";
/// When the worker here was nudged (epoch seconds), beside the brief.
const NUDGE_FILE: &str = "convoy-nudged";
/// More worktrees than one machine runs workers in.
const PS_LIMIT: &str = "1000";

/// One `orca … --json` call → its `result` object. An error when orca cannot
/// be run, exits non-zero, or answers `ok: false` — never an empty answer.
pub fn orca(args: &[&str]) -> Result<Value> {
    let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    let out = Command::new("orca")
        .args(args)
        .arg("--json")
        .output()
        .with_context(|| format!("orca {what} could not run"))?;
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    if !out.status.success() || answer["ok"] != true {
        let code = answer["error"]["code"].as_str().map(str::to_string);
        let said = code.unwrap_or_else(|| {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if stderr.is_empty() {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            } else {
                stderr
            }
        });
        bail!("orca {what} failed: {said}");
    }
    Ok(answer["result"].clone())
}

/// The repo orca cuts this checkout's worktrees from.
pub struct Source {
    pub id: String,
}

impl Source {
    /// The orca repo whose path is this checkout's main worktree.
    pub fn of(repo: &Repo) -> Result<Source> {
        let main = repo.main_checkout()?;
        let listed = orca(&["repo", "list"])?;
        let rows = listed["repos"].as_array().cloned().unwrap_or_default();
        let found = rows
            .iter()
            .find(|row| row["path"].as_str().is_some_and(|path| same_dir(Path::new(path), &main)));
        let id = found.and_then(|row| row["id"].as_str()).ok_or_else(|| {
            anyhow!(
                "orca knows no repo at {} — add this checkout once with `orca repo add --path <path>`",
                main.display()
            )
        })?;
        Ok(Source { id: id.to_string() })
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
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

    /// The landing train's worktree on this machine: linked to no issue, it is
    /// known by the name its directory was created under.
    pub fn of_train(source: &Source) -> Result<Option<Worktree>> {
        let listed = orca(&["worktree", "list"])?;
        let rows = listed["worktrees"].as_array().cloned().unwrap_or_default();
        let mut hits: Vec<&Value> = rows
            .iter()
            .filter(|row| {
                let path = Path::new(row["path"].as_str().unwrap_or(""));
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                is_train_worktree(name)
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

    /// The train's tip the worker here was last told about, when one was.
    pub fn told(&self) -> Result<Option<String>> {
        let told = std::fs::read_to_string(self.note(TOLD_FILE)?).ok();
        Ok(told.map(|tip| tip.trim().to_string()).filter(|tip| !tip.is_empty()))
    }

    pub fn record_told(&self, tip: &str) -> Result<()> {
        let file = self.note(TOLD_FILE)?;
        std::fs::write(&file, tip).with_context(|| format!("cannot write {}", file.display()))
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

    /// The last `lines` lines of what `terminal` shows.
    pub fn screen(&self, terminal: &str, lines: usize) -> Result<Vec<String>> {
        let limit = lines.to_string();
        let read =
            orca(&["terminal", "read", "--terminal", terminal, "--screen", "--limit", &limit])?;
        let tail = read["terminal"]["tail"].as_array().cloned().unwrap_or_default();
        Ok(tail.iter().filter_map(|line| line.as_str().map(str::to_string)).collect())
    }

    /// Has orca remove the worktree, its terminals with it.
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
        let nudge = self.note(NUDGE_FILE)?;
        if nudge.exists() {
            std::fs::remove_file(&nudge)
                .with_context(|| format!("cannot remove {}", nudge.display()))?;
        }
        // One with no terminal is as wanted.
        let _ = orca(&["terminal", "close", "--worktree", &self.selector(), "--all"]);
        let made =
            orca(&["terminal", "create", "--worktree", &self.selector(), "--command", command])?;
        let handle = made["terminal"]["handle"].as_str();
        Ok(handle
            .ok_or_else(|| anyhow!("orca terminal create returned no terminal handle"))?
            .to_string())
    }

    /// Waits until the agent in `terminal` is ready for a prompt.
    pub fn await_ready(&self, terminal: &str, timeout_seconds: u64) -> Result<()> {
        let timeout = (timeout_seconds * 1000).to_string();
        let waited = orca(&[
            "terminal",
            "wait",
            "--terminal",
            terminal,
            "--for",
            "tui-idle",
            "--timeout-ms",
            &timeout,
        ]);
        match waited {
            Ok(result) if result["wait"]["satisfied"] == true => Ok(()),
            Ok(_) => bail!(
                "the worker in {} was not ready for a prompt within {timeout_seconds}s (terminal {terminal})",
                self.path.display()
            ),
            Err(e) => Err(e),
        }
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

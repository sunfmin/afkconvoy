//! Running the gate in a worktree, and its records on the remote.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, GreenRun, Ran, Recording, Tree, after_run, record_ref};
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::git::{REMOTE, Repo};

/// What keeps a worktree from being exactly a commit: its uncommitted and
/// untracked paths as `git status --porcelain` lists them, and — given the
/// commit it was at — a HEAD that moved. Empty when it is the commit and
/// nothing else. Ignored files are not listed: a gate's own artifacts belong
/// in `.gitignore`.
pub fn off_commit(repo: &Repo, head: Option<&str>) -> Result<Vec<String>> {
    let mut off = vec![];
    let now = repo.git(&["rev-parse", "HEAD"])?;
    if let Some(head) = head.filter(|head| *head != now) {
        off.push(format!("HEAD moved from {head} to {now}"));
    }
    let status = repo.git_raw(&["status", "--porcelain"])?;
    off.extend(status.lines().filter(|line| !line.is_empty()).map(str::to_string));
    Ok(off)
}

/// Runs the gate command in a worktree, its log going straight to this
/// process's stderr → (exit code, timed out). A run that outlives `timeout`
/// is killed, with everything it started.
fn run_command(repo: &Repo, command: &str, timeout: Duration) -> Result<(i32, bool)> {
    let mut child = Command::new("sh")
        .args(["-c", command])
        .current_dir(&repo.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .context("the gate command could not be started")?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.code().unwrap_or(-1), false));
        }
        if started.elapsed() >= timeout {
            let group = format!("-{}", child.id());
            let _ = Command::new("kill").args(["-KILL", "--", &group]).status();
            let _ = child.wait();
            eprintln!("[afkconvoy] the gate timed out after {}s", timeout.as_secs());
            return Ok((124, true));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One run of the gate in a worktree, and what became of its record.
pub struct GateRun {
    pub ran: Ran,
    /// The commit the worktree was at when the run started.
    pub head: String,
    /// What kept the run from being of the committed tree; empty when it was.
    pub strays: Vec<String>,
    /// The evidence, when the run was green on a committed tree.
    pub green: Option<GreenRun>,
    /// Why a green run is not on record on the remote, when it is not.
    pub not_recorded: Option<String>,
}

/// The tree the worktree's commit holds.
pub fn head_tree(repo: &Repo) -> Result<Tree> {
    Ok(Tree::named(&repo.git(&["rev-parse", "HEAD^{tree}"])?))
}

/// Runs the gate in a worktree and settles its tree's record: a green run on
/// a committed tree is put on record, a red one on a committed tree takes the
/// record away.
pub fn run_and_record(
    repo: &Repo,
    command: &GateCommand,
    now: u64,
    timeout: Duration,
) -> Result<GateRun> {
    let head = repo.git(&["rev-parse", "HEAD"])?;
    let tree = head_tree(repo)?;
    let before = off_commit(repo, None)?;
    let (exit_code, timed_out) = run_command(repo, command.text(), timeout)?;
    let after = if before.is_empty() { off_commit(repo, Some(&head))? } else { vec![] };
    let ran = Ran {
        exit_code,
        timed_out,
        clean_before: before.is_empty(),
        clean_after: after.is_empty(),
    };
    let refname = record_ref(&tree, command);
    let strays = if before.is_empty() { after } else { before };
    let mut run = GateRun { ran, head, strays, green: None, not_recorded: None };
    match after_run(&tree, command, ran, now) {
        Recording::Write(green) => {
            // The record is a parentless commit OF the tested tree.
            let sha = repo.record_commit_of(tree.name(), &green.write(&tree, command))?;
            let pushed =
                repo.try_git(&["push", "--quiet", "--force", REMOTE, &format!("{sha}:{refname}")])?;
            if !pushed.ok {
                // Soft: a record that cannot be written costs the landing a run of the gate.
                run.not_recorded = Some(pushed.stderr.trim().to_string());
            }
            run.green = Some(green);
        }
        Recording::Drop => {
            // Soft: none there is what was wanted.
            let _ = repo.try_git(&["push", "--quiet", REMOTE, "--delete", &refname]);
        }
        Recording::Leave => {}
    }
    Ok(run)
}

/// The green run of `command` on `tree` the remote has on record and still
/// believes at `now`; `None` when it holds none that stands.
pub fn recorded(
    repo: &Repo,
    tree: &Tree,
    command: &GateCommand,
    now: u64,
) -> Result<Option<GreenRun>> {
    let found = repo.remote_records(&record_ref(tree, command))?.pop();
    let run = found.and_then(|record| GreenRun::recorded(&record.subject, tree, command));
    Ok(run.filter(|run| run.stands(now)))
}

/// `afkconvoy gate`: a worker's run of the gate, in the worktree it is called
/// from. The log streams to the worker's terminal; the answer says whether the
/// run is on record as a pass of the commit's tree.
pub fn gate(repo: &Repo, config: &Config, now: u64, timeout: Duration) -> Result<Value> {
    let command = GateCommand::new(&config.gate_command);
    let run = run_and_record(repo, &command, now, timeout)?;
    let status = if run.ran.green() { "green" } else { "red" };
    let mut answer = json!({"status": status, "exit_code": run.ran.exit_code,
        "timed_out": run.ran.timed_out, "command": command.text(), "head": run.head,
        "recorded": false});
    let head = &run.head;
    let detail = if !run.ran.green() {
        "the gate is red — nothing is on record; fix it, commit, and run this again".to_string()
    } else if run.green.is_none() {
        let why = if run.ran.clean_before {
            "the run left the worktree other than the commit it started on"
        } else {
            "the worktree had uncommitted or untracked files when the run started"
        };
        answer["uncommitted"] = json!(run.strays.iter().take(20).collect::<Vec<_>>());
        format!(
            "green, but not on a committed tree — {why}. This run proves nothing about a commit. \
             Commit what belongs to the change (or have the gate's own artifacts ignored) and run \
             this again"
        )
    } else if let Some(why) = &run.not_recorded {
        answer["not_recorded"] = json!(why);
        format!("green on {head}, but the record could not be written — push it and go on")
    } else {
        answer["recorded"] = json!(true);
        format!("green on {head} and on record — push it; any further commit needs another run")
    };
    answer["detail"] = json!(detail);
    Ok(answer)
}

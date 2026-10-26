//! Running the gate in a worktree, and its records on the remote.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{
    GateCommand, GreenRun, Ran, Recording, Tree, after_run, at_limit, record_ref,
};
use afkconvoy_core::lock::named;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};
use crate::lock::standing_locks;
use crate::measure;
use crate::train::{Train, merge_in, unmerged};

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

/// How much of the end of a red run's log is kept, to be shown to whoever
/// was not at the terminal.
const EXCERPT_LINES: usize = 40;

/// The exit code of a run ended at the gate limit: `timeout`'s.
pub(crate) const TIMED_OUT_CODE: i32 = 124;

/// How many of the paths that keep a green run's tree from being the commit
/// its answer lists.
pub(crate) const STRAYS_LISTED: usize = 20;

/// Runs the gate command in a worktree, its log going line by line to this
/// process's stderr → (exit code, timed out, the whole seconds it took, the
/// end of the log). A run that reaches the gate limit, `limit`, is killed, with
/// everything it started.
fn run_command(
    repo: &Repo,
    command: &str,
    limit: Duration,
) -> Result<(i32, bool, u64, Vec<String>)> {
    let mut shell = Command::new("sh");
    crate::exclusive::inherit(&mut shell, repo)?;
    let mut child = crate::process::spawn(
        shell
            // one stream, so the log reads in the order it was written
            .args(["-c", &format!("exec 2>&1\n{command}")])
            .current_dir(&repo.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0),
    )
    .context("the gate command could not be started")?;
    let log = child.stdout.take().context("the gate command has no log")?;
    let (lines, read) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(log).split(b'\n').map_while(|line| line.ok()) {
            let line = String::from_utf8_lossy(&line).into_owned();
            eprintln!("{line}");
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut tail: VecDeque<String> = VecDeque::new();
    let mut keep = |line: String| {
        if tail.len() == EXCERPT_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    };
    let started = Instant::now();
    let (code, timed_out) = loop {
        while let Ok(line) = read.try_recv() {
            keep(line);
        }
        if let Some(status) = child.try_wait()? {
            break (status.code().unwrap_or(-1), false);
        }
        if started.elapsed() >= limit {
            let group = format!("-{}", child.id());
            let _ = crate::process::status(Command::new("kill").args(["-KILL", "--", &group]));
            let _ = child.wait();
            eprintln!("[afkconvoy] the gate reached the gate limit of {}s", limit.as_secs());
            break (TIMED_OUT_CODE, true);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let seconds = started.elapsed().as_secs();
    // What the command wrote before it ended; something it left running may
    // hold the log open, and is not waited for.
    while let Ok(line) = read.recv_timeout(Duration::from_millis(200)) {
        keep(line);
    }
    Ok((code, timed_out, seconds, tail.into()))
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
    /// The end of the run's log.
    pub excerpt: Vec<String>,
}

/// The tree the worktree's commit holds.
pub fn head_tree(repo: &Repo) -> Result<Tree> {
    Ok(Tree::named(&repo.git(&["rev-parse", "HEAD^{tree}"])?))
}

/// Runs the gate in a worktree and settles its tree's record: a green run on
/// a committed tree is put on record, with how long it took on `host`; a red
/// one on a committed tree takes the record away. A run that reaches the gate
/// limit, `limit`, is red.
pub fn run_and_record(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
) -> Result<GateRun> {
    let _permit = if crate::exclusive::held() {
        None
    } else {
        let train = repo
            .dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(afkconvoy_core::train::is_train_worktree);
        Some(crate::exclusive::Permit::take(
            repo,
            command.text(),
            train,
            crate::config::heavy_gate(command.text()),
            false,
        )?)
    };
    let head = repo.git(&["rev-parse", "HEAD"])?;
    let tree = head_tree(repo)?;
    let before = off_commit(repo, None)?;
    let (exit_code, timed_out, seconds, excerpt) = run_command(repo, command.text(), limit)?;
    crate::acting::Acting::new("exclusive")
        .on_own_issue()
        .in_checkout(Some(repo.clone()))
        .say(crate::clock(), format!("gate command ended (exit {exit_code}) after {seconds}s"));
    let after = if before.is_empty() { off_commit(repo, Some(&head))? } else { vec![] };
    let ran = Ran {
        exit_code,
        timed_out,
        clean_before: before.is_empty(),
        clean_after: after.is_empty(),
        seconds,
    };
    let refname = record_ref(repo.layout(), &tree, command);
    let strays = if before.is_empty() { after } else { before };
    let mut run = GateRun { ran, head, strays, green: None, not_recorded: None, excerpt };
    match after_run(&tree, command, ran, host, now) {
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
    let found = repo.remote_records(&record_ref(repo.layout(), tree, command))?.pop();
    let run = found.and_then(|record| GreenRun::recorded(&record.subject, tree, command));
    Ok(run.filter(|run| run.stands(now)))
}

/// Whether the committed tree of a worktree passes the gate — the one question
/// a landing asks of it.
pub enum Gated {
    /// It does: by the run the remote has on record for that tree, or by one
    /// made now (`source`).
    Green {
        run: GreenRun,
        source: &'static str,
    },
    Red {
        ran: Ran,
        excerpt: Vec<String>,
    },
}

/// Answers from the green run on record for the worktree's tree and the
/// command — which is left as it is, no new record written — else by a run
/// made now on `host` under the gate limit, `limit`, which is put on record in
/// its turn. A run made now answers only when it was a run of the committed
/// tree: otherwise this is an error, and nothing lands.
pub fn gated(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
) -> Result<Gated> {
    let tree = head_tree(repo)?;
    if let Some(run) = recorded(repo, &tree, command, now)? {
        return Ok(Gated::Green { run, source: "recorded" });
    }
    let _train_lane = crate::exclusive::Permit::take(repo, command.text(), true, true, false)?;
    let not_the_commit = |why: &str, strays: &[String]| {
        let listed: Vec<&str> = strays.iter().take(20).map(String::as_str).collect();
        anyhow!(
            "the worktree {} is not the commit that would land — {why}. Nothing landed and \
             nothing is on record. Commit what belongs to the change, have the gate's own \
             artifacts ignored (`.gitignore`), discard the rest, and run this again:\n{}",
            repo.dir.display(),
            listed.join("\n")
        )
    };
    let strays = off_commit(repo, None)?;
    if !strays.is_empty() {
        return Err(not_the_commit("it has uncommitted or untracked files", &strays));
    }
    let run = run_and_record(repo, command, host, now, limit)?;
    match run.green {
        Some(green) => Ok(Gated::Green { run: green, source: "run" }),
        None if run.ran.green() => {
            Err(not_the_commit("the gate left it other than the commit it started on", &run.strays))
        }
        None => Ok(Gated::Red { ran: run.ran, excerpt: run.excerpt }),
    }
}

/// `afkconvoy gate`: a worker's run of the gate, in the worktree it is called
/// from, on `host`, under the config's gate limit. The log streams to the
/// worker's terminal; the answer says how long the run took and whether it is
/// on record as a pass of the commit's tree.
///
/// `train`: the branch first takes in what its PR will land behind — the
/// train's tip, or the base branch when there is no train — by a merge, never
/// a rebase. So a conflict with what is on the train is met before the run,
/// and a green run is of the very tree the PR joins the train with. A merge
/// that conflicts is left in progress and no gate runs.
///
/// A red run with a fleet lock standing says which: the base branch may be
/// what is red, and the worker is to declare nothing of it.
pub fn gate(repo: &Repo, config: &Config, train: bool, host: &str, now: u64) -> Result<Value> {
    let train_worktree = repo
        .dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(afkconvoy_core::train::is_train_worktree);
    if train && !train_worktree && !crate::exclusive::held() {
        let gh = GitHub::of(repo)?;
        let branch = repo.git(&["symbolic-ref", "--short", "HEAD"])?;
        let prs = gh.open_prs()?;
        let pr = prs.iter().find(|pr| pr.head_branch == branch)
            .ok_or_else(|| anyhow!("no open PR has branch {branch:?} for its head — push it and open its PR before asking for a turn (`gh pr create --base {} --head {branch}`)", config.base_branch))?;
        let issue = pr.closes.first().ok_or_else(|| {
            anyhow!("this PR closes no issue — use `land --inspection` for an inspection's PR")
        })?;
        return crate::train::land(repo, config, &gh, *issue, host, now);
    }
    let command = GateCommand::new(&config.gate_command);
    let mut merged = None;
    if train {
        let mut files = unmerged(repo)?;
        if files.is_empty() {
            let train = Train::read(repo, config)?;
            files = merge_in(repo, &train.tip, "the landing train")?;
            merged = Some(train.tip);
        }
        if !files.is_empty() {
            return Ok(json!({"status": "conflict", "files": files, "recorded": false,
                "detail": "merging what this PR lands behind into this branch conflicted; the \
                    merge is in progress here — resolve every file, `git add` it, COMMIT the \
                    merge, and run this again. No gate ran"}));
        }
    }
    let limit = config.gate_seconds();
    let run = run_and_record(repo, &command, host, now, Duration::from_secs(limit))?;
    // A worker's fresh build is what its worktree holds after its first green
    // run — one that built everything — by the train worktree's measure: what
    // its held disk is judged against (`hot`). Best effort: none on record
    // only leaves it unjudged.
    if run.ran.green() && measure::fresh(repo).is_ok_and(|fresh| fresh.is_none()) {
        let _ = measure::built(repo).and_then(|built| measure::record_fresh(repo, built));
    }
    let red = format!("the gate is red{}", at_limit(run.ran, limit));
    let status = if run.ran.green() { "green" } else { "red" };
    let mut answer = json!({"status": status, "exit_code": run.ran.exit_code,
        "timed_out": run.ran.timed_out, "seconds": run.ran.seconds, "command": command.text(),
        "head": run.head, "recorded": false});
    if let Some(merged) = merged {
        answer["merged"] = json!(merged);
    }
    let head = &run.head;
    // Soft: locks that cannot be read cost the worker only this warning.
    let locks = match run.ran.green() {
        true => vec![],
        false => GitHub::of(repo).and_then(|gh| standing_locks(&gh, config)).unwrap_or_default(),
    };
    let detail = if !locks.is_empty() {
        answer["locks"] = json!(locks);
        format!(
            "{red} — nothing is on record. A fleet lock stands ({}): the base branch \
             may be what is red, and not your change — declare nothing of it, stop, and run \
             this again when you are told to carry on",
            named(&locks)
        )
    } else if !run.ran.green() {
        format!("{red} — nothing is on record; fix it, commit, and run this again")
    } else if run.green.is_none() {
        let why = if run.ran.clean_before {
            "the run left the worktree other than the commit it started on"
        } else {
            "the worktree had uncommitted or untracked files when the run started"
        };
        answer["uncommitted"] = json!(run.strays.iter().take(STRAYS_LISTED).collect::<Vec<_>>());
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

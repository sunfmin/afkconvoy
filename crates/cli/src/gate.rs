//! Running the gate in a worktree, and its records on the remote.

use std::collections::VecDeque;
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{
    GateCommand, GreenRun, Ran, Recording, Tree, after_run, at_limit, record_ref,
};
use afkconvoy_core::lock::named;
use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};
use crate::lock::standing_locks;
use crate::measure;

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

/// What wait status proves about a gate command ended by a signal. The pid
/// also names its process group, created by the gate. Wait status identifies
/// no sender and proves nothing about which repository checks started.
#[derive(Clone, Copy, Serialize)]
pub struct Termination {
    pid: u32,
    signal: i32,
    core_dumped: bool,
}

impl Termination {
    pub(crate) fn description(&self) -> String {
        format!(
            "gate command pid {} (process group {}) ended by signal {} (core dumped: {}); \
             signal sender unknown; repository checks may not have started",
            self.pid, self.pid, self.signal, self.core_dumped
        )
    }
}

struct CommandRun {
    exit_code: i32,
    timed_out: bool,
    seconds: u64,
    excerpt: Vec<String>,
    termination: Option<Termination>,
    cancelled: bool,
}

/// Runs the gate command in a worktree, its log going line by line to this
/// process's stderr, keeping its status, signal evidence and the end of its
/// log. A run that reaches the gate limit, `limit`, is killed, with everything
/// it started, and is reported as a timeout rather than an outside signal.
fn run_command(
    repo: &Repo,
    command: &str,
    limit: Duration,
    candidate: Option<&crate::candidates::Gate<'_>>,
) -> Result<CommandRun> {
    let mut shell = Command::new("sh");
    crate::exclusive::inherit(&mut shell, repo)?;
    let prefix = if candidate.is_some() { "read -r convoy_ready || exit 125\n" } else { "" };
    let mut child = crate::process::spawn(
        shell
            // one stream, so the log reads in the order it was written
            .args(["-c", &format!("{prefix}exec 2>&1\n{command}")])
            .current_dir(&repo.dir)
            .stdin(if candidate.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0),
    )
    .context("the gate command could not be started")?;
    let pid = child.id();
    if let Some(candidate) = candidate {
        if let Err(error) = candidate.started(pid) {
            drop(child.stdin.take());
            let _ = child.wait();
            return Err(error);
        }
        child.stdin.take().context("candidate gate has no start barrier")?.write_all(b"go\n")?;
    }
    eprintln!(
        "[afkconvoy] gate: command pid {pid} (process group {pid}) started; gate limit {}s",
        limit.as_secs()
    );
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
    let mut cancelled = false;
    let (exit_code, timed_out, termination) = loop {
        while let Ok(line) = read.try_recv() {
            keep(line);
        }
        if let Some(candidate) = candidate
            && candidate.cancelled()?
        {
            cancelled = true;
            let _ = child.wait();
            break (-1, false, None);
        }
        if let Some(status) = child.try_wait()? {
            let termination = status.signal().map(|signal| Termination {
                pid,
                signal,
                core_dumped: status.core_dumped(),
            });
            break (status.code().unwrap_or(-1), false, termination);
        }
        if started.elapsed() >= limit {
            let group = format!("-{}", child.id());
            let _ = crate::process::status(Command::new("kill").args(["-KILL", "--", &group]));
            let _ = child.wait();
            eprintln!("[afkconvoy] the gate reached the gate limit of {}s", limit.as_secs());
            break (TIMED_OUT_CODE, true, None);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let seconds = started.elapsed().as_secs();
    // What the command wrote before it ended; something it left running may
    // hold the log open, and is not waited for.
    while let Ok(line) = read.recv_timeout(Duration::from_millis(200)) {
        keep(line);
    }
    if let Some(termination) = termination {
        let line = format!("[afkconvoy] gate: {}", termination.description());
        eprintln!("{line}");
        keep(line);
    }
    Ok(CommandRun { exit_code, timed_out, seconds, excerpt: tail.into(), termination, cancelled })
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
    /// Present only when wait status reported a signal, outside our timeout.
    pub termination: Option<Termination>,
    pub cancelled: bool,
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
    controlled_run(repo, command, host, now, limit, None)
}

fn controlled_run(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    candidate: Option<&crate::candidates::Gate<'_>>,
) -> Result<GateRun> {
    let _permit = if crate::exclusive::held() {
        None
    } else {
        Some(crate::exclusive::Permit::take(
            repo,
            command.text(),
            crate::config::heavy_gate(command.text()),
            false,
        )?)
    };
    let head = repo.git(&["rev-parse", "HEAD"])?;
    let tree = head_tree(repo)?;
    let before = off_commit(repo, None)?;
    let command_run = run_command(repo, command.text(), limit, candidate)?;
    let ended = match command_run.termination {
        Some(termination) => termination.description(),
        None => format!("gate command ended (exit {})", command_run.exit_code),
    };
    crate::acting::Acting::new("exclusive")
        .on_own_issue()
        .in_checkout(Some(repo.clone()))
        .say(crate::clock(), format!("{ended} after {}s", command_run.seconds));
    let after = if before.is_empty() { off_commit(repo, Some(&head))? } else { vec![] };
    let ran = Ran {
        exit_code: command_run.exit_code,
        timed_out: command_run.timed_out,
        clean_before: before.is_empty(),
        clean_after: after.is_empty(),
        seconds: command_run.seconds,
    };
    let refname = record_ref(repo.layout(), &tree, command);
    let strays = if before.is_empty() { after } else { before };
    let mut run = GateRun {
        ran,
        head,
        strays,
        green: None,
        not_recorded: None,
        excerpt: command_run.excerpt,
        termination: command_run.termination,
        cancelled: command_run.cancelled,
    };
    if run.cancelled {
        return Ok(run);
    }
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

pub(crate) fn candidate_gate(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    candidate: &crate::candidates::Gate<'_>,
) -> Result<Value> {
    let tree = head_tree(repo)?;
    if let Some(run) = recorded(repo, &tree, command, now)? {
        return Ok(
            json!({"status": "green", "recorded": true, "source": "recorded", "command": command.text(), "seconds": run.took().map(|took| took.seconds()), "head": repo.git(&["rev-parse", "HEAD"])?}),
        );
    }
    let run = controlled_run(repo, command, host, now, limit, Some(candidate))?;
    Ok(
        json!({"status": if run.cancelled { "cancelled" } else if run.ran.green() { "green" } else { "red" },
        "recorded": run.green.is_some() && run.not_recorded.is_none(), "head": run.head,
        "command": command.text(), "seconds": run.ran.seconds, "exit_code": run.ran.exit_code,
        "timed_out": run.ran.timed_out, "termination": run.termination, "excerpt": run.excerpt,
        "not_recorded": run.not_recorded, "uncommitted": run.strays}),
    )
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
        termination: Option<Termination>,
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
        None => Ok(Gated::Red { ran: run.ran, excerpt: run.excerpt, termination: run.termination }),
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
    if train && !crate::exclusive::held() {
        let gh = GitHub::of(repo)?;
        let branch = repo.git(&["symbolic-ref", "--short", "HEAD"])?;
        let prs = gh.open_prs()?;
        let pr = prs.iter().find(|pr| pr.head_branch == branch)
            .ok_or_else(|| anyhow!("no open PR has branch {branch:?} for its head — push it and open its PR before asking for a turn (`gh pr create --base {} --head {branch}`)", config.base_branch))?;
        let issue = pr.closes.first().ok_or_else(|| {
            anyhow!("this PR closes no issue — use `land --watch` for a watch's PR")
        })?;
        return crate::turn::land(repo, config, &gh, *issue, host, now);
    }
    let command = GateCommand::new(&config.gate_command);
    let limit = config.gate_seconds();
    let run = run_and_record(repo, &command, host, now, Duration::from_secs(limit))?;
    // A worker's fresh build is what its worktree holds after its first green
    // run — one that built everything — by the train worktree's measure: what
    // its held disk is judged against (`hot`). Best effort: none on record
    // only leaves it unjudged.
    if run.ran.green() && measure::fresh(repo).is_ok_and(|fresh| fresh.is_none()) {
        let _ = measure::built(repo).and_then(|built| measure::record_fresh(repo, built));
    }
    let red = match run.termination {
        Some(termination) => format!("the gate is red — {}", termination.description()),
        None => format!("the gate is red{}", at_limit(run.ran, limit)),
    };
    let status = if run.ran.green() { "green" } else { "red" };
    let mut answer = json!({"status": status, "exit_code": run.ran.exit_code,
        "timed_out": run.ran.timed_out, "seconds": run.ran.seconds, "command": command.text(),
        "head": run.head, "recorded": false, "termination": run.termination});
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
    } else if run.termination.is_some() {
        format!(
            "{red} — nothing is on record; inspect harness/tool cancellation and host process \
            logs using this pid and signal, then run `afkconvoy gate` again"
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
        format!(
            "green on {head}, but the record could not be written — run this again before joining"
        )
    } else {
        answer["recorded"] = json!(true);
        format!("green on {head} and on record — push it; any further commit needs another run")
    };
    answer["detail"] = json!(detail);
    Ok(answer)
}

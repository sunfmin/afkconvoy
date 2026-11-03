//! Running the gate in a worktree, and its records on the remote.

use std::collections::VecDeque;
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use afkconvoy_core::config::{Config, Which};
use afkconvoy_core::gate::{
    GateCommand, GreenRun, Ran, Recording, Tree, after_run, at_limit, believed_among, record_ref,
    red_ref, red_subject,
};
use afkconvoy_core::layout::Layout;
use afkconvoy_core::lock::named;
use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

use crate::gh::GitHub;
use crate::git::Repo;
use crate::lock::standing_locks;
use crate::measure;
use crate::remote::{Fresh, Push};

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

/// Gives a gate's tests their share of the host's cores where it runs beside
/// others (`exclusive::share`). That is told once its lane is held: now, in
/// a turn that holds it, or else by the `exclusive` its command waits in.
fn share(command: &mut Command) {
    if crate::exclusive::held() {
        crate::exclusive::share(command, std::process::id());
    } else {
        command.env(crate::exclusive::SHARE_ENV, "1");
    }
}

/// How often a candidate's gate asks whether the candidate was cancelled.
/// Each ask reads the chain live from the remote — over the network, on a
/// real one — and starts git, all beside the gate it is asked of; asked at
/// every look at the gate (50 ms), they took the host's cores from it
/// (sunfmin/control#309). The turn that cancels a candidate on this host
/// signals its gate itself (`candidates::cleanup`).
const CANCELLED_EVERY: Duration = Duration::from_secs(1);

/// Runs the gate command in a worktree, its log going line by line to this
/// process's stderr, keeping its status, signal evidence and the end of its
/// log. A run that reaches the gate limit, `limit`, is killed, with everything
/// it started, and is reported as a timeout rather than an outside signal.
/// `alone` is the one of the two gates run alone, which the command's
/// `exclusive` is told, so that a fast gate takes the fast lane.
fn run_command(
    repo: &Repo,
    command: &str,
    limit: Duration,
    candidate: Option<&crate::candidates::Gate<'_>>,
    alone: Option<Which>,
) -> Result<CommandRun> {
    let mut shell = Command::new("sh");
    crate::exclusive::inherit(&mut shell, repo)?;
    match alone {
        Some(which) => shell.env(crate::exclusive::GATE_ENV, which.word()),
        None => shell.env_remove(crate::exclusive::GATE_ENV),
    };
    // A slow gate runs alone in its lane, its tests mostly waiting
    // on the processes they start: twelve at a time, as its profile says
    // (sunfmin/control#234), not the share a fast gate's tests are held to.
    if !(candidate.is_some_and(|c| c.slow) || alone == Some(Which::Slow)) {
        share(&mut shell);
    }
    if let Some(candidate) = candidate {
        shell.env(crate::candidates::PROCESS_ENV, candidate.process_marker());
        // Five cold builds at once on eight cores passed the fast gate's
        // limit before their tests began (2026-10-29).
        if std::env::var_os("CARGO_TARGET_DIR").is_none() {
            shell.env("CARGO_TARGET_DIR", candidate.target());
        }
    }
    let scope = candidate
        .map(|c| c.scope().and_then(|path| crate::process_scope::create(&path, &mut shell)))
        .transpose()?;
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
    drop(scope); // only descendants retain the shared process-ownership lease.
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
    let mut asked: Option<Instant> = None;
    let (exit_code, timed_out, termination) = loop {
        while let Ok(line) = read.try_recv() {
            keep(line);
        }
        let ask = candidate.filter(|_| asked.is_none_or(|at| at.elapsed() >= CANCELLED_EVERY));
        if ask.is_some() {
            asked = Some(Instant::now());
        }
        if let Some(candidate) = ask
            && candidate.cancelled()?
        {
            cancelled = true;
            candidate.stop()?;
            let _ = child.wait();
            break (-1, false, None);
        }
        if let Some(status) = child.try_wait()? {
            let termination = status.signal().map(|signal| Termination {
                pid,
                signal,
                core_dumped: status.core_dumped(),
            });
            if !status.success()
                && let Some(candidate) = candidate
            {
                candidate.failed(
                    status.code().unwrap_or(-1),
                    false,
                    started.elapsed().as_secs(),
                    termination,
                )?;
            }
            break (status.code().unwrap_or(-1), false, termination);
        }
        if started.elapsed() >= limit {
            if let Some(candidate) = candidate {
                candidate.failed(TIMED_OUT_CODE, true, started.elapsed().as_secs(), None)?;
            } else {
                let group = format!("-{}", child.id());
                let _ = crate::process::status(Command::new("kill").args(["-KILL", "--", &group]));
            }
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
    issue: Option<u64>,
) -> Result<GateRun> {
    controlled_run(repo, command, host, now, limit, issue, None)
}

fn controlled_run(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    issue: Option<u64>,
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
    let command_run = run_command(repo, command.text(), limit, candidate, None)?;
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
    // A new record is written on the noted layout and not also on the other.
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
    // The host may have slept while the gate ran. Its initial check does
    // not authorize a record after another host has taken the issue.
    if let Some(issue) = issue {
        crate::claims::on_this_host(repo, issue, &crate::host_settings::host_name(), Fresh::Live)?;
    }
    match after_run(&tree, command, ran, host, now) {
        Recording::Write(green) => {
            let sha = repo.record_commit_on(
                tree.name(),
                &run.head,
                &green.write_issued(&tree, command, issue),
            )?;
            // Soft: a record that cannot be written costs the landing a run of the gate.
            run.not_recorded = push_record(repo, &sha, &refname)?;
            run.green = Some(green);
        }
        Recording::Drop => {
            // Soft: none there is what was wanted. Both layouts, so a red run
            // is not believed from the one the record was left on.
            for layout in Layout::ALL {
                let name = record_ref(layout, &tree, command);
                let limit = Some(record_push_limit());
                let _ = repo.remote().try_push_retried(&Push::delete(&name), limit, || Ok(false));
            }
            // A red run of this issue's tree is kept on its own ref until the
            // work account cites it. A base-branch certification names no issue.
            if let Some(issue) = issue {
                let subject = red_subject(&tree, command, ran.seconds, host, now, issue);
                let red = red_ref(repo.layout(), issue, now, tree.key(), command.key());
                if let Ok(sha) = repo.record_commit_on(tree.name(), &run.head, &subject) {
                    let _ = repo.remote().try_push_retried(
                        &Push::set(&sha, &red).force().progress(),
                        Some(record_push_limit()),
                        || Ok(false),
                    );
                }
            }
        }
        Recording::Leave => {}
    }
    Ok(run)
}

/// Every newly assigned candidate runs its gate, including a cut whose tree
/// matches the certified base. Its own green result survives predecessor
/// landing; another candidate's record does not replace this run.
pub(crate) fn candidate_gate(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    issue: Option<u64>,
    candidate: &crate::candidates::Gate<'_>,
) -> Result<Value> {
    let run = controlled_run(repo, command, host, now, limit, issue, Some(candidate))?;
    Ok(
        json!({"status": if run.cancelled { "cancelled" } else if run.ran.green() { "green" } else { "red" },
        "recorded": run.green.is_some() && run.not_recorded.is_none(), "head": run.head,
        "command": command.text(), "seconds": run.ran.seconds, "exit_code": run.ran.exit_code,
        "timed_out": run.ran.timed_out, "termination": run.termination, "excerpt": run.excerpt,
        "not_recorded": run.not_recorded, "uncommitted": run.strays}),
    )
}

/// Puts the whole gate, `whole`, on record for `tree` from the records of its
/// two parts there, `fast` and `slow` → why it is not on record, `None` when
/// it is. `head` is the tested commit and the new record's parent. A round's
/// tip whose slow gate passed is a tree the whole gate passed (docs/adr/0048):
/// the target tip it becomes needs no other run.
pub(crate) fn record_whole(
    repo: &Repo,
    tree: &Tree,
    head: &str,
    (whole, fast, slow): (&GateCommand, &GateCommand, &GateCommand),
    issue: Option<u64>,
    now: u64,
) -> Result<Option<String>> {
    let (Some(f), Some(s)) = (recorded(repo, tree, fast, now)?, recorded(repo, tree, slow, now)?)
    else {
        return Ok(Some(format!(
            "the fast or the slow gate's run of {} is not on record",
            tree.name()
        )));
    };
    let Some(run) = GreenRun::joined(tree, whole, fast, &f, slow, &s) else {
        return Ok(Some("the fast and the slow gate are one command".into()));
    };
    let sha = repo.record_commit_on(tree.name(), head, &run.write_issued(tree, whole, issue))?;
    let refname = record_ref(repo.layout(), tree, whole);
    push_record(repo, &sha, &refname)
}

/// How long one gate-record push may take before its process group is ended.
/// The tests set this short; the production bound keeps a stalled transport
/// from holding a candidate's worker lane.
const RECORD_PUSH_SECONDS: u64 = 30;

fn record_push_limit() -> Duration {
    let seconds = std::env::var("AFKCONVOY_GATE_RECORD_PUSH_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(RECORD_PUSH_SECONDS);
    Duration::from_secs(seconds)
}

/// Pushes the record `sha` to `refname` → why it could not, `None` once it
/// is there. A record's push is a force push of one commit, so trying it
/// again is safe. A rejection that is GitHub's own fault is tried again
/// (`Remote::try_push_retried`); any other is not. On 2026-10-29 two
/// candidates whose fast gate was green went red for the first, cancelling
/// every candidate behind them. A push that reaches `record_push_limit` is
/// also not retried: its remote outcome is unknown, so this run stays
/// unrecorded.
fn push_record(repo: &Repo, sha: &str, refname: &str) -> Result<Option<String>> {
    let push = Push::set(sha, refname).force().progress();
    let pushed = repo.remote().try_push_retried(&push, Some(record_push_limit()), || Ok(false))?;
    Ok((!pushed.ok).then(|| push_error(&pushed.stderr)))
}

/// The last pack size Git's writing progress reported, with its unit.
fn sent_bytes(stderr: &str) -> Option<&str> {
    stderr
        .split(['\r', '\n'])
        .filter_map(|line| {
            let writing = line.rsplit_once("Writing objects:")?.1;
            let (_, bytes) = writing.rsplit_once(',')?;
            let bytes = bytes.split('|').next()?.trim();
            (!bytes.is_empty()).then_some(bytes)
        })
        .next_back()
}

fn push_error(stderr: &str) -> String {
    let stderr = stderr.trim();
    if !stderr.contains("did not end within") {
        return stderr.to_string();
    }
    let timeout = stderr
        .rsplit(['\r', '\n'])
        .find(|line| line.contains("did not end within"))
        .unwrap_or(stderr);
    let sent = sent_bytes(stderr).unwrap_or("no pack bytes");
    format!("{timeout}; Git reported {sent} sent before the push was ended")
}

/// The green run of `command` on `tree` the remote has on record and still
/// believes at `now`, from either ref layout; `None` when it holds none that
/// stands. One listing reads both records.
pub fn recorded(
    repo: &Repo,
    tree: &Tree,
    command: &GateCommand,
    now: u64,
) -> Result<Option<GreenRun>> {
    let noted = repo.layout();
    let names: Vec<String> =
        Layout::ALL.into_iter().map(|layout| record_ref(layout, tree, command)).collect();
    let patterns: Vec<&str> = names.iter().map(String::as_str).collect();
    let found = repo.remote().records(&patterns, Fresh::Process)?;
    Ok(recorded_among(&found, noted, tree, command, now))
}

/// The still-valid green run among an existing remote snapshot's records,
/// from either ref layout. The noted layout's record wins when both stand.
pub fn recorded_among(
    records: &[crate::remote::RemoteRecord],
    layout: afkconvoy_core::layout::Layout,
    tree: &Tree,
    command: &GateCommand,
    now: u64,
) -> Option<GreenRun> {
    let pairs: Vec<(&str, &str)> =
        records.iter().map(|record| (record.name.as_str(), record.subject.as_str())).collect();
    believed_among(&pairs, layout, tree, command, now)
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
/// command, from either ref layout — which is left as it is, no new record
/// written — else by a run made now on `host` under the gate limit, `limit`,
/// which is put on record on the noted layout. A run made now answers only
/// when it was a run of the committed tree: otherwise this is an error, and
/// nothing lands. A record of another tree, including the commit a cut lands,
/// is not a run of this one.
pub fn gated(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    issue: Option<u64>,
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
    let run = run_and_record(repo, command, host, now, limit, issue)?;
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
    let issue = std::env::var(afkconvoy_core::role::ISSUE_ENV)
        .ok()
        .and_then(|raw| afkconvoy_core::record::fleet_number(&raw));
    let run = run_and_record(repo, &command, host, now, Duration::from_secs(limit), issue)?;
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

/// `afkconvoy gate --fast` or `--slow`: one of the two gates the repo's gate
/// is split into (docs/adr/0047), run alone in the worktree it is called
/// from, under its own limit, and held to its own budget. Neither is put on
/// record: what a landing turn proves is the whole gate, `gate_command`.
pub fn one(repo: &Repo, config: &Config, which: Which) -> Result<Value> {
    let name = which.word();
    let gate = config.gate(which).ok_or_else(|| {
        anyhow!(
            "this repo declares no {name} gate: its config has no ```yaml gates block \
             (`afkconvoy config --help`); run `afkconvoy gate`, the whole gate"
        )
    })?;
    let run =
        run_command(repo, &gate.command, Duration::from_secs(gate.seconds), None, Some(which))?;
    let green = run.exit_code == 0 && !run.timed_out && run.termination.is_none();
    let over_budget = run.seconds > gate.budget_seconds;
    let detail = match (green, &run.termination) {
        (true, _) if over_budget => format!(
            "green, but slow: the {name} gate took {}s, past its budget of {}s — it is not red; \
             say so on the issue if your change made it slower",
            run.seconds, gate.budget_seconds
        ),
        (true, _) => format!("green in {}s; nothing is on record", run.seconds),
        (false, Some(termination)) => {
            format!("the {name} gate is red — {}", termination.description())
        }
        (false, None) if run.timed_out => format!(
            "the {name} gate is red: its run reached its limit of {}s and was ended",
            gate.seconds
        ),
        (false, None) => format!("the {name} gate is red — fix it, commit, and run this again"),
    };
    Ok(json!({"gate": name, "status": if green { "green" } else { "red" },
        "exit_code": run.exit_code, "timed_out": run.timed_out, "seconds": run.seconds,
        "command": gate.command, "limit_seconds": gate.seconds,
        "budget_seconds": gate.budget_seconds, "over_budget": over_budget, "recorded": false,
        "termination": run.termination, "excerpt": if green { vec![] } else { run.excerpt },
        "detail": detail}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timed_out_record_push_reports_gits_last_pack_progress() {
        let stderr = "Writing objects:  50% (1/2)\r\
                      Writing objects: 100% (2/2), 12.50 MiB | 1.25 MiB/s\r\
                      git push --quiet --progress did not end within 1s; its process group was ended";
        let said = push_error(stderr);
        assert!(said.contains("did not end within 1s"), "{said}");
        assert!(said.contains("Git reported 12.50 MiB sent before the push was ended"), "{said}");
        assert!(!said.contains("Writing objects"), "progress is evidence, not the answer: {said}");
        assert_eq!(sent_bytes("Writing objects: 50% (1/2)"), None);
        assert!(push_error("git push did not end within 1s").contains("no pack bytes sent"));
    }
}

//! Running the gate in a worktree, and its records on the remote.

use std::collections::VecDeque;
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use afkconvoy_core::candidate::{Evidence, Overloaded, Status};
use afkconvoy_core::config::{Config, Which};
pub use afkconvoy_core::gate::Termination;
use afkconvoy_core::gate::{
    Ended, GateCommand, GreenRun, Judged, Ran, Recording, Red, Tree, Unsettled, after_run,
    believed_among, judge, record_ref, red_ref, red_subject,
};
use afkconvoy_core::layout::Layout;
use afkconvoy_core::lock::named;
use anyhow::{Context, Result, anyhow};
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

struct CommandRun {
    exit_code: i32,
    timed_out: bool,
    seconds: u64,
    excerpt: Vec<String>,
    termination: Option<Termination>,
    cancelled: bool,
    judged: Judged,
}

/// Keeps `line` at the end of the excerpt `tail`, and in the whole run's
/// lines `all`.
fn keep(tail: &mut VecDeque<String>, all: &mut Vec<String>, line: String) {
    if tail.len() == EXCERPT_LINES {
        tail.pop_front();
    }
    tail.push_back(line.clone());
    all.push(line);
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
/// `exclusive` is told, so that a fast gate takes the fast lane. Its whole
/// log is also written to `output`, where one is given: a file of this run's
/// own, which it creates.
fn run_command(
    repo: &Repo,
    command: &str,
    limit: Duration,
    candidate: Option<&crate::candidates::Gate<'_>>,
    alone: Option<Which>,
    output: Option<&Path>,
) -> Result<CommandRun> {
    let mut kept =
        output
            .map(|file| {
                std::fs::OpenOptions::new().write(true).create_new(true).open(file).with_context(
                    || format!("cannot write the gate's output to {}", file.display()),
                )
            })
            .transpose()?;
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
            // Soft: a log that cannot be written leaves the run as it is.
            if let Some(file) = kept.as_mut()
                && writeln!(file, "{line}").is_err()
            {
                kept = None;
            }
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut tail: VecDeque<String> = VecDeque::new();
    let mut all: Vec<String> = vec![];
    // What the command wrote before it ended; something it left running may
    // hold the log open, and is not waited for.
    let drain = |tail: &mut VecDeque<String>, all: &mut Vec<String>| {
        while let Ok(line) = read.recv_timeout(Duration::from_millis(200)) {
            keep(tail, all, line);
        }
    };
    let mut judged = None;
    let started = Instant::now();
    let mut cancelled = false;
    let mut asked: Option<Instant> = None;
    let (exit_code, timed_out, termination) = loop {
        while let Ok(line) = read.try_recv() {
            keep(&mut tail, &mut all, line);
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
            let code = status.code().unwrap_or(-1);
            if !status.success() {
                // Its whole output first: the gate's last line says whether
                // the run says anything of the tree.
                drain(&mut tail, &mut all);
            }
            let said = judge(code, false, limit.as_secs(), termination, &all);
            // An unsettled run is no red: it cancels no candidate behind
            // this one, nor its round (sunfmin/control#617).
            if matches!(said, Judged::Red(_))
                && let Some(candidate) = candidate
            {
                candidate.failed(
                    status.code().unwrap_or(-1),
                    false,
                    started.elapsed().as_secs(),
                    termination,
                )?;
            }
            judged = Some(said);
            break (code, false, termination);
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
    drain(&mut tail, &mut all);
    // A run ended at the limit, or cancelled, is judged on what it wrote by
    // then; a cancelled one's is read by nobody.
    let judged =
        judged.unwrap_or_else(|| judge(exit_code, timed_out, limit.as_secs(), termination, &all));
    if let Some(termination) = termination {
        let line = format!("[afkconvoy] gate: {}", termination.description());
        eprintln!("{line}");
        keep(&mut tail, &mut all, line);
    }
    Ok(CommandRun {
        exit_code,
        timed_out,
        seconds,
        excerpt: tail.into(),
        termination,
        cancelled,
        judged,
    })
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
    /// What the run says of its tree, judged from its whole output
    /// (`gate::judge`); an unsettled one is recorded neither green nor red.
    pub judged: Judged,
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
    output: Option<&Path>,
) -> Result<GateRun> {
    controlled_run(repo, command, host, now, limit, issue, None, output)
}

#[allow(clippy::too_many_arguments)]
fn controlled_run(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    issue: Option<u64>,
    candidate: Option<&crate::candidates::Gate<'_>>,
    output: Option<&Path>,
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
    let command_run = run_command(repo, command.text(), limit, candidate, None, output)?;
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
        judged: command_run.judged,
    };
    if run.cancelled {
        return Ok(run);
    }
    // The host may have slept while the gate ran. Its initial check does
    // not authorize a record after another host has taken the issue.
    if let Some(issue) = issue {
        crate::claims::on_this_host(repo, issue, &crate::host_settings::host_name(), Fresh::Live)?;
    }
    let recording = match run.judged {
        Judged::Unsettled(_) => Recording::Leave,
        _ => after_run(&tree, command, ran, host, now),
    };
    match recording {
        Recording::Write(green) => {
            let sha = repo.record_commit_on(
                tree.name(),
                &run.head,
                &afkconvoy_core::gate::write_issued(&green, &tree, command, issue),
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
/// landing; another candidate's record does not replace this run. → the
/// run's evidence, as the chain keeps it, and what the run says of its tree.
pub(crate) fn candidate_gate(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    issue: Option<u64>,
    candidate: &crate::candidates::Gate<'_>,
) -> Result<(Evidence, Judged)> {
    let run = controlled_run(repo, command, host, now, limit, issue, Some(candidate), None)?;
    let unsettled = match &run.judged {
        Judged::Unsettled(unsettled) => {
            Some(Overloaded { said: unsettled.said.clone(), tests: unsettled.tests().to_vec() })
        }
        Judged::Green | Judged::Red(_) => None,
    };
    let status = match () {
        _ if run.cancelled => Status::Cancelled,
        _ if run.ran.green() => Status::Green,
        _ => Status::Red,
    };
    let evidence = Evidence {
        command: Some(command.text().to_string()),
        exit_code: run.ran.exit_code,
        excerpt: run.excerpt,
        head: Some(run.head),
        recorded: run.green.is_some() && run.not_recorded.is_none(),
        not_recorded: run.not_recorded,
        seconds: run.ran.seconds,
        status,
        termination: run.termination,
        timed_out: run.ran.timed_out,
        uncommitted: run.strays,
        unsettled,
    };
    Ok((evidence, run.judged))
}

/// Runs `gate` in a worktree of its own, detached at `commit` beside
/// `repo`'s and named for `name`, for a gate that no worker's changes may
/// reach: a base tip's, a delivery's. The worktree is removed after
/// (`Repo::remove_worktree`), whatever the run → what `gate` answered, and
/// why the worktree is still there, where it is: the caller says whether
/// that hides its answer.
pub fn detached<T>(
    repo: &Repo,
    commit: &str,
    name: &str,
    gate: impl FnOnce(&Repo) -> Result<T>,
) -> Result<(Result<T>, Option<anyhow::Error>)> {
    let path = std::env::temp_dir().join(format!("afkconvoy-{name}-{}", crate::unique()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    repo.git(&["worktree", "add", "--quiet", "--detach", &path.to_string_lossy(), commit])?;
    let answered = gate(&Repo::new(path.clone()));
    Ok((answered, repo.remove_worktree(&path).err()))
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
    let sha = repo.record_commit_on(
        tree.name(),
        head,
        &afkconvoy_core::gate::write_issued(&run, tree, whole, issue),
    )?;
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
    if !stderr.contains(crate::git::PAST_LIMIT) {
        return stderr.to_string();
    }
    let timeout = stderr
        .rsplit(['\r', '\n'])
        .find(|line| line.contains(crate::git::PAST_LIMIT))
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
    Green { run: GreenRun, source: &'static str },
    /// It does not, by a run made now.
    Red { red: Red, excerpt: Vec<String> },
    /// A run made now says nothing of it (`Unsettled`), and nothing is on
    /// record.
    Unsettled { unsettled: Unsettled, excerpt: Vec<String> },
}

/// Answers from the green run on record for the worktree's tree and the
/// command, from either ref layout — which is left as it is, no new record
/// written — else by a run made now on `host` under the gate limit, `limit`,
/// which is put on record on the noted layout. A run made now answers only
/// when it was a run of the committed tree: otherwise this is an error, and
/// nothing lands. A record of another tree, including the commit a cut lands,
/// is not a run of this one. A run made now writes its whole log to
/// `output`, where one is given (`run_command`).
pub fn gated(
    repo: &Repo,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    issue: Option<u64>,
    output: Option<&Path>,
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
    let run = run_and_record(repo, command, host, now, limit, issue, output)?;
    match run.green {
        Some(green) => Ok(Gated::Green { run: green, source: "run" }),
        None if run.ran.green() => {
            Err(not_the_commit("the gate left it other than the commit it started on", &run.strays))
        }
        None => match run.judged {
            Judged::Unsettled(unsettled) => {
                Ok(Gated::Unsettled { unsettled, excerpt: run.excerpt })
            }
            Judged::Red(red) => Ok(Gated::Red { red, excerpt: run.excerpt }),
            Judged::Green => unreachable!("a green run is answered above"),
        },
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
        .and_then(|raw| afkconvoy_core::record::canonical_number(&raw));
    let run = run_and_record(repo, &command, host, now, Duration::from_secs(limit), issue, None)?;
    // A worker's fresh build is what its worktree holds after its first green
    // run — one that built everything — by the train worktree's measure: what
    // its held disk is judged against (`hot`). Best effort: none on record
    // only leaves it unjudged.
    if run.ran.green() && measure::fresh(repo).is_ok_and(|fresh| fresh.is_none()) {
        let _ = measure::built(repo).and_then(|built| measure::record_fresh(repo, built));
    }
    let red = match &run.judged {
        Judged::Red(red) | Judged::Unsettled(Unsettled { red, .. }) => red.said("the gate"),
        Judged::Green => String::new(),
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
    let run = run_command(
        repo,
        &gate.command,
        Duration::from_secs(gate.seconds),
        None,
        Some(which),
        None,
    )?;
    let green = run.judged == Judged::Green;
    let over_budget = run.seconds > gate.budget_seconds;
    let detail = match &run.judged {
        Judged::Green if over_budget => format!(
            "green, but slow: the {name} gate took {}s, past its budget of {}s — it is not red; \
             say so on the issue if your change made it slower",
            run.seconds, gate.budget_seconds
        ),
        Judged::Green => format!("green in {}s; nothing is on record", run.seconds),
        Judged::Red(red) | Judged::Unsettled(Unsettled { red, .. }) => {
            let said = red.said(&format!("the {name} gate"));
            match red.ended {
                Ended::Exited(_) => format!("{said} — fix it, commit, and run this again"),
                Ended::AtLimit(_) | Ended::Signalled(_) => said,
            }
        }
    };
    Ok(json!({"gate": name, "status": if green { "green" } else { "red" },
        "exit_code": run.exit_code, "timed_out": run.timed_out, "seconds": run.seconds,
        "command": gate.command, "limit_seconds": gate.seconds,
        "budget_seconds": gate.budget_seconds, "over_budget": over_budget, "recorded": false,
        "termination": run.termination, "excerpt": if green { vec![] } else { run.excerpt },
        "detail": detail}))
}

/// What `afkconvoy gate --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Run `gate` on a committed change before workflow review or for diagnosis. Run `gate --land` once an issue PR is open to take the same landing turn as `land --issue <n>`. Where the repo splits its gate in two, run `gate --fast` before opening or updating a PR, and `gate --slow` to run the rest alone. A fast gate waits on no slow gate or landing turn: it takes the host's fast lane (`afkconvoy exclusive --help`).

## Who runs it

A worker in its own worktree, or the operator diagnosing a red gate.

Before running any gate, a worker's AFKCONVOY_ISSUE claim is read live. If another host holds it, the command refuses: `the claim on #N is held by instance X on host H since <ts>: this worker's work was taken; stop`. Nothing runs or is recorded. The host is the name its host settings give it, regardless of --host; a restart on the same host keeps its workers.

## What it reads and writes

The config's gate_command runs on this worktree, with its log streaming to stderr and its gate limit, gate_seconds (default {const:GATE_SECONDS}). A green run on the committed tree is recorded with the tree, command, host and elapsed seconds. The record has the tested commit as its parent, so its push sends only objects the remote lacks; parentless records already on the remote are still found by tree and command. A dirty worktree or artifacts the gate fails to ignore cannot produce a record. Its first green run records the fresh build for worker disk observations.

With --fast or --slow, the fast gate's or the slow gate's command runs alone, as the ```yaml gates block of the config declares it (`afkconvoy config --help`), under that gate's own limit, `seconds`. A run here is put on record as neither, and neither stands in for the whole gate: a landing turn runs the fast gate per candidate and the slow gate per round, and records the whole gate only from both on one tree (`afkconvoy land --help`). A run past that gate's `budget_seconds` is green and says it is slow. A repo that declares no gates block answers an error.

With --land, open the PR first. One turn makes the PR's merge commit on the current base tip, gates that commit, records green and lands it as a fast-forward. An unrecorded base tip is gated alone first; worker turns wait while it is red and the fleet dispatches its repair. Only the fleet-lock issue's repair may take a turn on a red base. `land --help` describes every outcome.

## What it answers

{"status", "recorded", "detail", "exit_code", "timed_out", "seconds", "command", "head"}. status is green or red; recorded is true only for a green run of the committed tree written to the remote. detail says what to do next. seconds times the command. A run that reaches gate_seconds is red with exit_code {const:TIMED_OUT_CODE} and timed_out true. uncommitted lists at most {const:STRAYS_LISTED} paths when green could not certify a committed tree; not_recorded says why the record could not be written, including a push ended after stalling for 30 seconds. locks names standing fleet locks on a red run. termination is null unless a signal ended the command outside its limit; then {pid, signal, core_dumped}, with exit_code -1 and timed_out false. Its pid also names its process group. The sender is unknown and repository checks may not have started; inspect harness/tool cancellation and host process logs using that evidence. A shell exit such as 137 alone does not prove a signal. With --land the answer also carries the landing outcome, PR and base tip merged. With --fast or --slow it is {"gate", "status", "recorded", "detail", "exit_code", "timed_out", "seconds", "command", "limit_seconds", "budget_seconds", "over_budget", "termination", "excerpt"}: gate is fast or slow, recorded is always false, over_budget says whether seconds passed budget_seconds, and excerpt is the end of a red run's log.

Errors are {"error": …} with non-zero exit: an unreadable config, an uncommitted landing change, a missing base branch or no open issue PR for this branch. Red is an answer with exit 0; read status and recorded.

## What trips you up

- Commit every change before gating. A further commit needs another run.
- A standalone gate lands nothing; a workflow's landing worker lands after its final stage.
- Resolve every conflict and commit the merge before asking for another turn.
- The gate limit is gate_seconds in the config, not a flag.
- A green fast gate is not a green gate: the slow gate's tests have not run. Only the whole gate, at landing, is on record, from a round's fast and slow runs on one tree or a run of gate_command."#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `script` as a gate in a directory of its own, under `limit`,
    /// keeping its whole output in `output` where one is named there.
    fn ran(script: &str, limit: u64, output: Option<&str>) -> (CommandRun, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::new(dir.path().to_path_buf());
        let output = output.map(|name| dir.path().join(name));
        let limit = Duration::from_secs(limit);
        let run = run_command(&repo, script, limit, None, None, output.as_deref()).unwrap();
        (run, dir)
    }

    const TIMEOUT: &str = "echo '     TIMEOUT [   6.004s] (40/669) afkconvoy::cli dsh::the_line'";
    const OVERLOADED: &str = "echo 'gate: host overloaded or load unavailable (load 12.71, CPUs \
                              12); cannot classify TIMEOUT tests.'";

    /// Every gate run is judged by the one module, from its whole output
    /// (sunfmin/control#619): green, red on a FAIL, red on a TIMEOUT the
    /// gate classified, unsettled on TIMEOUTs it cannot classify under
    /// overload, red at the gate limit.
    #[test]
    fn a_gate_run_is_judged_green_red_or_unsettled_from_its_whole_output() {
        assert_eq!(ran("echo fine", 60, None).0.judged, Judged::Green);

        let fail = "echo '        FAIL [   0.208s] (2/3) afkconvoy cutting::a_named_test'; \
                    exit 100";
        let Judged::Red(red) = ran(fail, 60, None).0.judged else { panic!("a FAIL is red") };
        assert_eq!(
            red.said("the gate"),
            "the gate is red: it exited 100; failing: afkconvoy cutting::a_named_test"
        );

        let quiet = format!(
            "{TIMEOUT}; echo 'gate: afkconvoy::cli dsh::the_line was still TIMEOUT at 2s on a \
             quiet host.'; exit 100"
        );
        let Judged::Red(red) = ran(&quiet, 60, None).0.judged else {
            panic!("a TIMEOUT the gate classified is red")
        };
        assert_eq!(red.tests, ["afkconvoy::cli dsh::the_line"]);

        let overloaded = format!("{TIMEOUT}; {OVERLOADED} >&2; exit 100");
        let Judged::Unsettled(unsettled) = ran(&overloaded, 60, None).0.judged else {
            panic!("only TIMEOUTs it cannot classify under overload say nothing")
        };
        assert_eq!(unsettled.tests(), ["afkconvoy::cli dsh::the_line"]);
        assert!(unsettled.said.contains("load 12.71, CPUs 12"), "{}", unsettled.said);

        let hung = format!("{TIMEOUT}; {OVERLOADED}; sleep 30");
        let (run, _dir) = ran(&hung, 1, None);
        assert!(run.timed_out);
        let Judged::Red(red) = run.judged else { panic!("a run at the gate limit is red") };
        assert!(red.how().starts_with("its run reached the gate limit of 1s"), "{}", red.how());
    }

    /// The run is judged from all it wrote, not from the excerpt kept of
    /// its end, nor from the file its whole output is kept in: a failure
    /// named before the excerpt is named, and a run that keeps no file is
    /// judged the same as one that does.
    #[test]
    fn a_gate_run_is_judged_from_all_it_wrote_with_or_without_a_kept_file() {
        let script = "echo '        FAIL [   0.208s] (2/3) afkconvoy cutting::a_named_test'; \
                      i=0; while [ $i -lt 100 ]; do echo line $i; i=$((i+1)); done; exit 1";
        let (kept, dir) = ran(script, 60, Some("gate.log"));
        let (unkept, _) = ran(script, 60, None);
        assert_eq!(kept.excerpt.len(), EXCERPT_LINES);
        assert!(!kept.excerpt.iter().any(|line| line.contains("FAIL")), "{:?}", kept.excerpt);
        let Judged::Red(red) = &kept.judged else { panic!("red") };
        assert_eq!(red.tests, ["afkconvoy cutting::a_named_test"]);
        assert!(red.quote[0].contains("FAIL"), "quoted from its first failure: {:?}", red.quote);
        assert_eq!(kept.judged, unkept.judged);
        let file = std::fs::read_to_string(dir.path().join("gate.log")).unwrap();
        assert_eq!(file.lines().count(), 101, "the file keeps the whole output");
    }

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

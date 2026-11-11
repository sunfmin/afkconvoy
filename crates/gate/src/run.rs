//! A gate run's evidence on the remote: what a run of a gate command on a
//! committed tree writes there, takes away, and reads back (docs/adr/0024).
//! Whether a run is put on record is `gate::after_run`'s to say; what is
//! written where, and read from which ref layout, is said here once, for
//! every program that records gate runs. How a record reaches the remote is
//! the program's own (`Records`); `Git` reaches it with plain git, and
//! `gate` runs a gate command with it, for a program that has no other way.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::gate::{
    GateCommand, GreenRun, Judged, Ran, Recording, Termination, Tree, after_run, believed_among,
    judge, record_ref,
};
use crate::layout::Layout;

/// Where a program's gate records are written and read: the remote, as it
/// reaches it.
pub trait Records {
    /// What reaching the remote fails with.
    type Error;

    /// The layout a new record is written on: the one the checkout noted.
    fn layout(&self) -> Layout;

    /// Each of the refs `names` the remote has, with the subject of the
    /// commit it points at. A name it has not is left out.
    fn read(&self, names: &[&str]) -> Result<Vec<(String, String)>, Self::Error>;

    /// A commit of `tree` on `parent` whose subject is `subject` → its sha.
    fn commit(&self, tree: &str, parent: &str, subject: &str) -> Result<String, Self::Error>;

    /// Puts the commit `sha` at `refname`, over whatever is there → why it is
    /// not there, `None` once it is.
    fn put(&self, sha: &str, refname: &str) -> Result<Option<String>, Self::Error>;

    /// Takes `refname` away, where it is: none there is what is wanted.
    fn delete(&self, refname: &str);
}

/// What became of a run's record.
#[derive(Debug, PartialEq, Eq)]
pub enum Settled {
    /// Green on a committed tree: the run, and why it is not on record on the
    /// remote, when it is not.
    Written { green: GreenRun, not_recorded: Option<String> },
    /// Red on a committed tree: the tree's record of the command is gone,
    /// from either layout.
    Dropped,
    /// The run says nothing of a committed tree: nothing changed.
    Left,
}

/// Settles the record of `command` on `tree`, at the commit `head`, as
/// `recording` says: a run put on record is written on the noted layout,
/// with `more` fields its program notes of it; a red run takes the record
/// away from both, so it is not believed from the one it was left on.
pub fn settle<R: Records>(
    records: &R,
    tree: &Tree,
    head: &str,
    command: &GateCommand,
    recording: Recording,
    more: &[(&str, &str)],
) -> Result<Settled, R::Error> {
    match recording {
        Recording::Write(green) => {
            let sha = records.commit(tree.name(), head, &green.write_with(tree, command, more))?;
            let not_recorded = records.put(&sha, &record_ref(records.layout(), tree, command))?;
            Ok(Settled::Written { green, not_recorded })
        }
        Recording::Drop => {
            for layout in Layout::ALL {
                records.delete(&record_ref(layout, tree, command));
            }
            Ok(Settled::Dropped)
        }
        Recording::Leave => Ok(Settled::Left),
    }
}

/// The green run of `command` on `tree` the remote has on record and still
/// believes at `now`, from either ref layout; `None` when it holds none that
/// stands. One read asks for both.
pub fn recorded<R: Records>(
    records: &R,
    tree: &Tree,
    command: &GateCommand,
    now: u64,
) -> Result<Option<GreenRun>, R::Error> {
    let names: Vec<String> =
        Layout::ALL.into_iter().map(|layout| record_ref(layout, tree, command)).collect();
    let asked: Vec<&str> = names.iter().map(String::as_str).collect();
    let found = records.read(&asked)?;
    let pairs: Vec<(&str, &str)> =
        found.iter().map(|(name, subject)| (name.as_str(), subject.as_str())).collect();
    Ok(believed_among(&pairs, records.layout(), tree, command, now))
}

/// Puts the whole gate, `whole`, on record for `tree` at the commit `head`
/// from the records of its two parts there, `fast` and `slow` (docs/adr/0047)
/// → why it is not on record, `None` once it is.
pub fn record_whole<R: Records>(
    records: &R,
    tree: &Tree,
    head: &str,
    (whole, fast, slow): (&GateCommand, &GateCommand, &GateCommand),
    more: &[(&str, &str)],
    now: u64,
) -> Result<Option<String>, R::Error> {
    let (Some(f), Some(s)) =
        (recorded(records, tree, fast, now)?, recorded(records, tree, slow, now)?)
    else {
        return Ok(Some(format!(
            "the fast or the slow gate's run of {} is not on record",
            tree.name()
        )));
    };
    let Some(run) = GreenRun::joined(tree, whole, fast, &f, slow, &s) else {
        return Ok(Some("the fast and the slow gate are one command".into()));
    };
    let sha = records.commit(tree.name(), head, &run.write_with(tree, whole, more))?;
    records.put(&sha, &record_ref(records.layout(), tree, whole))
}

/// A worktree whose records are kept on one remote, reached with plain git.
#[derive(Debug, Clone)]
pub struct Git {
    /// The worktree: its commit is the one a run tests.
    pub worktree: PathBuf,
    /// The remote its records are kept on: `origin`, a path, a URL.
    pub remote: String,
    /// The layout new records are written on.
    pub layout: Layout,
}

/// The identity a record's commit is written under, whoever's machine runs
/// it.
const RECORD_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "gate"),
    ("GIT_AUTHOR_EMAIL", "gate@localhost"),
    ("GIT_COMMITTER_NAME", "gate"),
    ("GIT_COMMITTER_EMAIL", "gate@localhost"),
];

/// One git call in `dir` with `env` that must succeed → its stdout, trimmed.
fn git(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("LC_ALL", "C")
        .envs(env.iter().copied())
        .output()
        .map_err(|error| {
            format!("git {} could not run in {}: {error}", args.join(" "), dir.display())
        })?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The commit the worktree `dir` is at, and the tree it holds.
pub fn head(dir: &Path) -> Result<(String, Tree), String> {
    let head = git(dir, &["rev-parse", "HEAD"], &[])?;
    let tree = git(dir, &["rev-parse", "HEAD^{tree}"], &[])?;
    Ok((head, Tree::named(&tree)))
}

/// What keeps the worktree `dir` from being exactly its commit — and the
/// commit `head`, where one is given: a HEAD that moved, and each path
/// uncommitted or untracked. Empty when it is that commit.
pub fn off_commit(dir: &Path, head: Option<&str>) -> Result<Vec<String>, String> {
    let mut off = vec![];
    let now = git(dir, &["rev-parse", "HEAD"], &[])?;
    if let Some(head) = head.filter(|head| *head != now) {
        off.push(format!("HEAD moved from {head} to {now}"));
    }
    let status = git(dir, &["status", "--porcelain"], &[])?;
    for line in status.lines() {
        if !line.is_empty() {
            off.push(line.to_string());
        }
    }
    Ok(off)
}

impl Records for Git {
    type Error = String;

    fn layout(&self) -> Layout {
        self.layout
    }

    fn read(&self, names: &[&str]) -> Result<Vec<(String, String)>, String> {
        let mut found = vec![];
        for name in names {
            let listed = git(&self.worktree, &["ls-remote", &self.remote, name], &[])?;
            let Some(sha) = listed
                .lines()
                .find_map(|line| line.split_once('\t').filter(|(_, n)| n == name))
                .map(|(sha, _)| sha.to_string())
            else {
                continue;
            };
            let have = git(&self.worktree, &["cat-file", "-e", &format!("{sha}^{{commit}}")], &[]);
            if have.is_err() {
                git(&self.worktree, &["fetch", "--quiet", "--no-tags", &self.remote, name], &[])?;
            }
            let subject = git(&self.worktree, &["show", "-s", "--format=%s", &sha], &[])?;
            found.push((name.to_string(), subject));
        }
        Ok(found)
    }

    fn commit(&self, tree: &str, parent: &str, subject: &str) -> Result<String, String> {
        git(&self.worktree, &["commit-tree", tree, "-p", parent, "-m", subject], &RECORD_IDENTITY)
    }

    fn put(&self, sha: &str, refname: &str) -> Result<Option<String>, String> {
        let pushed = git(
            &self.worktree,
            &["push", "--quiet", "--force", &self.remote, &format!("{sha}:{refname}")],
            &[],
        );
        Ok(pushed.err())
    }

    fn delete(&self, refname: &str) {
        let gone = format!(":{refname}");
        let _ = git(&self.worktree, &["push", "--quiet", &self.remote, &gone], &[]);
    }
}

/// One run of a gate command in a worktree, and what became of its record.
#[derive(Debug)]
pub struct Run {
    pub ran: Ran,
    /// The commit the worktree was at when the run started.
    pub head: String,
    /// The tree it held.
    pub tree: Tree,
    /// What kept the run from being of the committed tree; empty when it was.
    pub strays: Vec<String>,
    /// What the run says of its tree.
    pub judged: Judged,
    /// What became of its record.
    pub settled: Settled,
    /// Its output, stdout and stderr, line by line as each came.
    pub output: Vec<String>,
}

/// Sends each line `stream` says to `send`, as it comes, until it ends.
fn read_lines(
    stream: impl Read + Send + 'static,
    send: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let _ = send.send(line);
        }
    })
}

/// The exit code of a run ended at the gate limit: `timeout`'s.
pub const TIMED_OUT_CODE: i32 = 124;

/// How often a running command is looked at, for its end or its limit.
const LOOKS_EVERY: Duration = Duration::from_millis(20);

/// Runs `command` with `sh -c` in `git`'s worktree under the gate limit of
/// `limit`, in a process group of its own, which is ended at the limit, and
/// settles its tree's record: a run judged green on a committed tree is put
/// on record as having taken its seconds on `host`, finished at `now`, with
/// `more` fields; one judged red takes the record away; one that says nothing
/// of its tree, or ran on no committed tree, changes nothing. Errors: git
/// failed, or the command could not be started.
pub fn gate(
    git: &Git,
    command: &GateCommand,
    host: &str,
    now: u64,
    limit: Duration,
    more: &[(&str, &str)],
) -> Result<Run, String> {
    let (head, tree) = head(&git.worktree)?;
    let before = off_commit(&git.worktree, None)?;
    let began = Instant::now();
    let mut child = Command::new("sh")
        .args(["-c", command.text()])
        .current_dir(&git.worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("`{}` could not be started: {error}", command.text()))?;
    let (send, lines) = mpsc::channel();
    let mut readers = vec![];
    if let Some(out) = child.stdout.take() {
        readers.push(read_lines(out, send.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(read_lines(err, send.clone()));
    }
    drop(send);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if !timed_out && began.elapsed() >= limit {
            timed_out = true;
            let group = format!("-{}", child.id());
            let _ = Command::new("kill").args(["-KILL", "--", &group]).status();
        }
        std::thread::sleep(LOOKS_EVERY);
    };
    for reader in readers {
        let _ = reader.join();
    }
    let output: Vec<String> = lines.into_iter().collect();
    let seconds = began.elapsed().as_secs();
    let termination = match (status.code(), status.signal()) {
        (None, Some(signal)) if !timed_out => {
            Some(Termination { core_dumped: status.core_dumped(), pid: child.id(), signal })
        }
        _ => None,
    };
    let exit_code = match (timed_out, status.code(), status.signal()) {
        (true, _, _) => TIMED_OUT_CODE,
        (false, Some(code), _) => code,
        (false, None, Some(signal)) => signal.saturating_add(128),
        (false, None, None) => 1,
    };
    let after = if before.is_empty() { off_commit(&git.worktree, Some(&head))? } else { vec![] };
    let ran = Ran {
        exit_code,
        timed_out,
        clean_before: before.is_empty(),
        clean_after: after.is_empty(),
        seconds,
    };
    let judged = judge(exit_code, timed_out, limit.as_secs(), termination, &output);
    let recording = match judged {
        Judged::Unsettled(_) => Recording::Leave,
        _ => after_run(&tree, command, ran, host, now),
    };
    let settled = settle(git, &tree, &head, command, recording, more)?;
    let strays = if before.is_empty() { after } else { before };
    Ok(Run { ran, head, tree, strays, judged, settled, output })
}

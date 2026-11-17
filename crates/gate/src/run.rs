//! A gate run's evidence on record: what a run of a gate command on a
//! committed tree writes there, takes away, and reads back (docs/adr/0047).
//! Whether a run is put on record is `gate::after_run`'s to say; what is
//! written, and which record is believed, is said here once, for every
//! program that records gate runs. Where a record is kept is the program's
//! own (`Records`): afkconvoy keeps its host's in the host's database
//! (docs/adr/0037); `Git` keeps them on a remote, reached with plain git, and
//! `gate` runs a gate command with it, for a program that has no other way.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::gate::{
    GateCommand, GreenRun, Judged, Ran, Recording, Termination, Tree, after_run, judge, record_ref,
    test_times,
};
use crate::layout::Layout;

/// Where a program's gate records are written and read: one record of a
/// command on a tree, its whole message as `GreenRun::write_with` makes it.
pub trait Records {
    /// What reaching the records fails with.
    type Error;

    /// The whole message of each record of `command` on `tree` held, the one
    /// to believe first first; empty where none is.
    fn read(&self, tree: &Tree, command: &GateCommand) -> Result<Vec<String>, Self::Error>;

    /// Puts `message` on record as the run of `command` on `tree`, tested at
    /// the commit `head`, over whatever is there → why it is not on record,
    /// `None` once it is.
    fn write(
        &self,
        tree: &Tree,
        head: &str,
        command: &GateCommand,
        message: &str,
    ) -> Result<Option<String>, Self::Error>;

    /// Takes every record of `command` on `tree` away, where one is: none
    /// there is what is wanted.
    fn remove(&self, tree: &Tree, command: &GateCommand);
}

/// What became of a run's record.
#[derive(Debug, PartialEq, Eq)]
pub enum Settled {
    /// Green on a committed tree: the run, and why it is not on record, when
    /// it is not.
    Written { green: GreenRun, not_recorded: Option<String> },
    /// Red on a committed tree: the tree's record of the command is gone.
    Dropped,
    /// The run says nothing of a committed tree: nothing changed.
    Left,
    /// A reused run: the green run on record of the same command on the
    /// same committed tree, on the same kind of machine, answered for the
    /// run asked, and nothing ran (`reusable`). Its record is left as it is.
    Reused(GreenRun),
}

/// Settles the record of `command` on `tree`, at the commit `head`, as
/// `recording` says: a run put on record is written with `more` fields its
/// program notes of it; a red run takes the record away, so it is not
/// believed.
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
            let message = green.write_with(tree, command, more);
            let not_recorded = records.write(tree, head, command, &message)?;
            Ok(Settled::Written { green, not_recorded })
        }
        Recording::Drop => {
            records.remove(tree, command);
            Ok(Settled::Dropped)
        }
        Recording::Leave => Ok(Settled::Left),
    }
}

/// The green run of `command` on `tree` on record and still believed at
/// `now`, timing each test its run's output timed (`GreenRun::tests`); `None`
/// when none held stands. The first record `Records::read` gives that stands
/// is the one believed.
pub fn recorded<R: Records>(
    records: &R,
    tree: &Tree,
    command: &GateCommand,
    now: u64,
) -> Result<Option<GreenRun>, R::Error> {
    Ok(records.read(tree, command)?.iter().find_map(|message| {
        GreenRun::recorded(message, tree, command).filter(|run| run.stands(now))
    }))
}

/// The green run on record that answers for a run of `command` on `tree` on
/// a machine of kind `machine` (`gate::machine`) at `now`, instead of a run
/// made again: one `recorded` believes, which ran on that kind of machine.
/// A red run is never on record; a run on another kind of machine, or whose
/// record names none, answers for none — some tests depend on the machine.
/// The command is the gate the tree's own commit declares, so a different
/// declaration is another record.
pub fn reusable<R: Records>(
    records: &R,
    tree: &Tree,
    command: &GateCommand,
    machine: &str,
    now: u64,
) -> Result<Option<GreenRun>, R::Error> {
    let run = recorded(records, tree, command, now)?;
    Ok(run.filter(|run| !machine.is_empty() && run.machine() == Some(machine)))
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
    records.write(tree, head, whole, &run.write_with(tree, whole, more))
}

/// A worktree whose records are kept on one remote, reached with plain git:
/// each a commit of the tested tree at `gate::record_ref`, on either ref
/// layout.
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
    git_raw(dir, args, env).map(|out| out.trim().to_string())
}

/// One git call in `dir` with `env` that must succeed → its stdout, whole.
fn git_raw(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
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
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One git call in `dir` with `env`, given `input` on stdin, that must
/// succeed → its stdout, trimmed: for text too long for an argument.
fn git_given(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: &str,
) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("LC_ALL", "C")
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("git {} could not run in {}: {error}", args.join(" "), dir.display())
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(|error| error.to_string())?;
    }
    let out = child.wait_with_output().map_err(|error| error.to_string())?;
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

/// What keeps the worktree `dir` from being exactly a commit: its uncommitted
/// and untracked paths as `git status --porcelain` lists them, and — given
/// the commit `head` it was at — a HEAD that moved. Empty when it is the
/// commit and nothing else. Ignored files are not listed: a gate's own
/// artifacts belong in `.gitignore`.
pub fn off_commit(dir: &Path, head: Option<&str>) -> Result<Vec<String>, String> {
    let mut off = vec![];
    let now = git(dir, &["rev-parse", "HEAD"], &[])?;
    if let Some(head) = head.filter(|head| *head != now) {
        off.push(format!("HEAD moved from {head} to {now}"));
    }
    let status = git_raw(dir, &["status", "--porcelain"], &[])?;
    for line in status.lines() {
        if !line.is_empty() {
            off.push(line.to_string());
        }
    }
    Ok(off)
}

impl Git {
    /// The commit the remote's ref `name` points at, fetched here when it is
    /// not yet; `None` when the remote has no such ref.
    fn fetched(&self, name: &str) -> Result<Option<String>, String> {
        let listed = git(&self.worktree, &["ls-remote", &self.remote, name], &[])?;
        let Some(sha) = listed
            .lines()
            .find_map(|line| line.split_once('\t').filter(|(_, n)| *n == name))
            .map(|(sha, _)| sha.to_string())
        else {
            return Ok(None);
        };
        let have = git(&self.worktree, &["cat-file", "-e", &format!("{sha}^{{commit}}")], &[]);
        if have.is_err() {
            git(&self.worktree, &["fetch", "--quiet", "--no-tags", &self.remote, name], &[])?;
        }
        Ok(Some(sha))
    }
}

impl Records for Git {
    type Error = String;

    /// The noted layout's record first, then the other's.
    fn read(&self, tree: &Tree, command: &GateCommand) -> Result<Vec<String>, String> {
        let mut found = vec![];
        for layout in [self.layout, self.layout.other()] {
            if let Some(sha) = self.fetched(&record_ref(layout, tree, command))? {
                found.push(git(&self.worktree, &["show", "-s", "--format=%B", &sha], &[])?);
            }
        }
        Ok(found)
    }

    /// On the noted layout alone.
    fn write(
        &self,
        tree: &Tree,
        head: &str,
        command: &GateCommand,
        message: &str,
    ) -> Result<Option<String>, String> {
        let args = ["commit-tree", tree.name(), "-p", head, "-F", "-"];
        let sha = git_given(&self.worktree, &args, &RECORD_IDENTITY, &format!("{message}\n"))?;
        let refname = record_ref(self.layout, tree, command);
        let pushed = git(
            &self.worktree,
            &["push", "--quiet", "--force", &self.remote, &format!("{sha}:{refname}")],
            &[],
        );
        Ok(pushed.err())
    }

    /// From both layouts, so it is not believed from the one it was left on.
    fn remove(&self, tree: &Tree, command: &GateCommand) {
        for layout in Layout::ALL {
            let gone = format!(":{}", record_ref(layout, tree, command));
            let _ = git(&self.worktree, &["push", "--quiet", &self.remote, &gone], &[]);
        }
    }
}

/// What a reused run's output says, its one line: the run on record that
/// answered, and that nothing ran.
pub fn reused_line(green: &GreenRun, tree: &Tree, command: &GateCommand) -> String {
    let on = match green.took() {
        Some(took) => format!(" on {} in {}s", took.host(), took.seconds()),
        None => String::new(),
    };
    format!(
        "gate: reused the green run of `{}` on tree {} at {}{on}, on {}: nothing ran",
        command.text(),
        tree.name(),
        green.at(),
        green.machine().unwrap_or_default()
    )
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
/// on record as having taken its seconds on `host`, a machine of kind
/// `machine`, finished at `now`, with `more` fields and each test its output
/// timed (`test_times`); one judged red takes the record away; one that says
/// nothing of its tree, or ran on no committed tree, changes nothing.
///
/// A green run of a tree is not run again: where the worktree is exactly
/// its commit and `git` holds a green run of `command` on its tree,
/// on the same kind of machine, still believed at `now` (`reusable`), that
/// run answers, `Settled::Reused`, and nothing runs. Errors: git failed, or
/// the command could not be started.
pub fn gate(
    git: &Git,
    command: &GateCommand,
    (host, machine): (&str, &str),
    now: u64,
    limit: Duration,
    more: &[(&str, &str)],
) -> Result<Run, String> {
    let (head, tree) = head(&git.worktree)?;
    let before = off_commit(&git.worktree, None)?;
    if before.is_empty()
        && let Some(green) = reusable(git, &tree, command, machine, now)?
    {
        let ran = Ran {
            exit_code: 0,
            timed_out: false,
            clean_before: true,
            clean_after: true,
            seconds: 0,
        };
        let output = vec![reused_line(&green, &tree, command)];
        let settled = Settled::Reused(green);
        return Ok(Run { ran, head, tree, strays: vec![], judged: Judged::Green, settled, output });
    }
    let began = Instant::now();
    let mut child = Command::new("sh")
        .args(["-c", command.text()])
        .current_dir(&git.worktree)
        .env(crate::builds::SERIAL_BUILDS.0, crate::builds::SERIAL_BUILDS.1)
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
        _ => after_run(&tree, command, ran, host, now)
            .with_tests(test_times(&output))
            .on_machine(machine),
    };
    let settled = settle(git, &tree, &head, command, recording, more)?;
    let strays = if before.is_empty() { after } else { before };
    Ok(Run { ran, head, tree, strays, judged, settled, output })
}

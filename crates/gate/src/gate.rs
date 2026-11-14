//! The gate, green runs of it on record, and what a run's output says of
//! its tree.
//!
//! A record says: this command passed on this tree, and how long the run took
//! on which host, and each test's wall time and CPU time where the run's
//! output said them. It is one ref on the remote, named for the tree and the
//! command, so it stands wherever the same content is about to land — in
//! another worktree, on another machine.

use flux_rs::attrs::*;

use crate::layout::{GATE_KIND, Layout};
use crate::record;

/// How far ahead of the reader a stamp may be and still count as "just now":
/// ordinary skew between two hosts' clocks, in seconds.
pub const SKEW: u64 = 300;

/// How long a recorded run is believed: a green that depends on something
/// outside the tree (a toolchain, a service, the date) is trusted for a day.
pub const RECORD_TTL: u64 = 86_400;

/// The word a gate record opens with, green or red.
pub const GATE: &str = "convoy:gate";

/// A string as a number Flux can compare: equal strings give equal keys.
/// (FNV-1a; a collision would need two trees or two commands hashing alike.)
pub fn key(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A git tree: the content a gate run tests and a landing lands, whatever
/// commit holds it.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(key: int)]
pub struct Tree {
    name: String,
    #[field(u64[key])]
    key: u64,
}

impl Tree {
    /// The tree with the object name `name` (a sha).
    pub fn named(name: &str) -> Tree {
        Tree { name: name.to_string(), key: key(name) }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[spec(fn(&Tree[@t]) -> u64[t])]
    pub fn key(&self) -> u64 {
        self.key
    }
}

/// The gate command a repo configured, with the key records are filed under.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(key: int)]
pub struct GateCommand {
    text: String,
    #[field(u64[key])]
    key: u64,
}

impl GateCommand {
    pub fn new(text: &str) -> GateCommand {
        GateCommand { text: text.to_string(), key: key(text) }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    #[spec(fn(&GateCommand[@c]) -> u64[c])]
    pub fn key(&self) -> u64 {
        self.key
    }
}

/// The remote ref a recorded run of `command` on `tree` lives at. The name is
/// the key, so asking "was this tested green?" is asking for one ref, and two
/// runs never contend.
pub fn record_ref(layout: Layout, tree: &Tree, command: &GateCommand) -> String {
    layout.under(&format!("{GATE_KIND}{}-{:016x}", tree.name, command.key))
}

/// The green run of `command` on `tree` still believed at `now` among
/// `records` — each a ref name and the subject of the commit it points at —
/// from either ref layout. The noted layout's record is the one believed when
/// both stand. A day-old record, a red run, a run that wrote no record, and a
/// record of another tree or another command is not believed from either.
pub fn believed_among(
    records: &[(&str, &str)],
    noted: Layout,
    tree: &Tree,
    command: &GateCommand,
    now: u64,
) -> Option<GreenRun> {
    [noted, noted.other()].into_iter().find_map(|layout| {
        let name = record_ref(layout, tree, command);
        records.iter().find(|(record_name, _)| *record_name == name).and_then(|(_, subject)| {
            GreenRun::recorded(subject, tree, command).filter(|run| run.stands(now))
        })
    })
}

/// One run of the gate command, as the code that ran it saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ran {
    pub exit_code: i32,
    pub timed_out: bool,
    /// The worktree was exactly its commit when the run started: nothing
    /// uncommitted, nothing untracked.
    pub clean_before: bool,
    /// …and was still exactly that commit when it ended.
    pub clean_after: bool,
    /// The whole command's wall time, in whole seconds.
    pub seconds: u64,
}

impl Ran {
    /// Only exit 0 is green, and a run that timed out is red.
    pub fn green(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }
}

/// How long a green run of the gate command took, and the host it ran on. The
/// fleet times the whole command: it does not know the gate's steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Took {
    seconds: u64,
    host: String,
}

impl Took {
    /// `None` for a run on a host with no name: its record would state no
    /// host, and read back as a run with no time.
    fn on(host: &str, seconds: u64) -> Option<Took> {
        if host.is_empty() { None } else { Some(Took { seconds, host: host.to_string() }) }
    }

    pub fn seconds(&self) -> u64 {
        self.seconds
    }

    pub fn host(&self) -> &str {
        &self.host
    }
}

/// Evidence that `command` passed on `tree`, finished at `at`. There are three
/// ways to come by one, and no other: having seen the run's exit code
/// (`after_run`), having read the record off the remote (`GreenRun::recorded`),
/// or, for the whole gate, its fast and slow parts' runs on that tree
/// (`GreenRun::joined`).
#[derive(Debug, PartialEq, Eq)]
#[refined_by(tree: int, command: int, at: int)]
pub struct GreenRun {
    #[field(u64[tree])]
    tree: u64,
    #[field(u64[command])]
    command: u64,
    #[field(u64[at])]
    at: u64,
    took: Option<Took>,
    machine: Option<String>,
    tests: Vec<TestTime>,
}

/// The kind of machine this program runs on: its operating system and its
/// processor's architecture, `macos-aarch64`. A green run on record answers
/// for another run on the same kind of machine alone (`run::reusable`):
/// some tests depend on the machine.
pub fn machine() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

impl GreenRun {
    #[spec(fn(&GreenRun[@g]) -> u64[g.at])]
    pub fn at(&self) -> u64 {
        self.at
    }

    /// How long the run took, and on which host; `None` for a run whose record
    /// says neither, as every record did once. It is the green run it was.
    pub fn took(&self) -> Option<&Took> {
        self.took.as_ref()
    }

    /// Each test the run's output timed, in the order it first named them;
    /// empty for a run whose record says none, as every record did once. It
    /// is the green run it was.
    pub fn tests(&self) -> &[TestTime] {
        &self.tests
    }

    /// This run, timing `tests` (`test_times` of its output).
    #[spec(fn(GreenRun[@g], Vec<TestTime>) -> GreenRun[g])]
    pub fn with_tests(self, tests: Vec<TestTime>) -> GreenRun {
        GreenRun { tests, ..self }
    }

    /// The kind of machine the run ran on (`machine`); `None` for a run
    /// whose record says none, as every record did once. It is the green
    /// run it was, and answers for no run asked of a kind of machine.
    pub fn machine(&self) -> Option<&str> {
        self.machine.as_deref()
    }

    /// This run, as having run on a machine of kind `machine`; an empty
    /// kind says none.
    #[spec(fn(GreenRun[@g], &str) -> GreenRun[g])]
    pub fn on_machine(self, machine: &str) -> GreenRun {
        let machine = if machine.is_empty() { None } else { Some(machine.to_string()) };
        GreenRun { machine, ..self }
    }

    /// Whether the run is still believed at `now`: no older than a day, and
    /// not stamped from the future — whose day would never run out.
    #[spec(fn(&GreenRun[@g], now: u64) -> bool[fresh(g.at, now)])]
    pub fn stands(&self, now: u64) -> bool {
        believed(self.at, now)
    }

    /// The record that carries this run, for the commit pushed to `record_ref`.
    pub fn write(&self, tree: &Tree, command: &GateCommand) -> String {
        let at = self.at.to_string();
        let seconds = self.took.as_ref().map(|took| took.seconds.to_string()).unwrap_or_default();
        let host = self.took.as_ref().map(Took::host).unwrap_or_default();
        record::write(
            GATE,
            &[
                ("tree", &tree.name),
                ("command", &command.text),
                ("at", &at),
                ("seconds", &seconds),
                ("host", host),
                ("machine", self.machine.as_deref().unwrap_or_default()),
            ],
        )
    }

    /// The record `write` makes, followed by `more` fields the program that
    /// ran it notes of the run: who it was run for, say. They read back as
    /// the same run. Where the run timed its tests, the record is the
    /// message of its commit: that line, its subject, then a line for each
    /// test after a blank one, which a reader of subjects alone never sees.
    pub fn write_with(&self, tree: &Tree, command: &GateCommand, more: &[(&str, &str)]) -> String {
        let line = self.write(tree, command);
        let rest = record::write("", more);
        let mut message = format!("{line}{rest}");
        if !self.tests.is_empty() {
            message.push('\n');
        }
        for test in &self.tests {
            message.push('\n');
            message.push_str(&test.write());
        }
        message
    }

    /// A commit subject, or its whole message, read at
    /// `record_ref(tree, command)` → the run it records, timing the tests its
    /// message's later lines time; `None` when it is no record of a run of
    /// this command on this tree.
    #[spec(fn(subject: &str, tree: &Tree[@t], command: &GateCommand[@c])
        -> Option<GreenRun{g: g.tree == t && g.command == c}>)]
    pub fn recorded(subject: &str, tree: &Tree, command: &GateCommand) -> Option<GreenRun> {
        let fields = record::read(GATE, subject)?;
        if fields.get("tree") != Some(tree.name()) || fields.get("command") != Some(command.text())
        {
            return None;
        }
        if fields.get("red").is_some() {
            return None;
        }
        let at = fields.number("at")?;
        let took = match (fields.number("seconds"), fields.get("host")) {
            (Some(seconds), Some(host)) => Took::on(host, seconds),
            _ => None,
        };
        let machine = fields.get("machine").filter(|kind| !kind.is_empty()).map(str::to_string);
        let tests = subject.lines().skip(1).filter_map(TestTime::read).collect();
        Some(GreenRun { tree: tree.key, command: command.key, at, took, machine, tests })
    }

    /// The whole gate's run on `tree`, from the runs of its two parts there:
    /// `fast` of `fast_command` and `slow` of `slow_command`, two commands
    /// that together check what `whole` does (docs/adr/0047). It finished
    /// when the older part did, and took both parts' seconds on the slow
    /// part's host, timing the fast part's tests, then the slow part's; on
    /// the kind of machine both ran on, where they ran on one.
    /// `None` when either run is of another tree or command.
    #[spec(fn(tree: &Tree[@t], whole: &GateCommand[@w], fast_command: &GateCommand,
        fast: &GreenRun, slow_command: &GateCommand, slow: &GreenRun)
        -> Option<GreenRun{g: g.tree == t && g.command == w}>)]
    pub fn joined(
        tree: &Tree,
        whole: &GateCommand,
        fast_command: &GateCommand,
        fast: &GreenRun,
        slow_command: &GateCommand,
        slow: &GreenRun,
    ) -> Option<GreenRun> {
        if fast.tree != tree.key
            || slow.tree != tree.key
            || fast.command != fast_command.key
            || slow.command != slow_command.key
            || fast_command.key == slow_command.key
        {
            return None;
        }
        let at = if fast.at < slow.at { fast.at } else { slow.at };
        let took = match (&fast.took, &slow.took) {
            (Some(f), Some(s)) => Took::on(&s.host, f.seconds.saturating_add(s.seconds)),
            _ => None,
        };
        let machine = if fast.machine == slow.machine { slow.machine.clone() } else { None };
        let tests = fast.tests.iter().chain(&slow.tests).cloned().collect();
        Some(GreenRun { tree: tree.key, command: whole.key, at, took, machine, tests })
    }
}

/// Whether a run finished at `at` is believed at `now`.
#[spec(fn(at: u64, now: u64) -> bool[fresh(at, now)])]
pub fn believed(at: u64, now: u64) -> bool {
    if at <= now { now - at <= RECORD_TTL } else { at - now <= SKEW }
}

/// Whether the record a gate ref's commit carries is swept at `now`: one no
/// landing would believe any more, or one nobody can read. Whatever tree and
/// command it is of.
pub fn outlived(subject: &str, now: u64) -> bool {
    match record::read(GATE, subject).and_then(|fields| fields.number("at")) {
        Some(at) => !believed(at, now),
        None => true,
    }
}

// A run finished at `at` is believed at `now`.
defs! {
    fn fresh(at: int, now: int) -> bool {
        (at <= now && now - at <= RECORD_TTL) || (at > now && at - now <= SKEW)
    }
}

/// Leave to land a tree on the base branch: a green run of the configured
/// command on that very tree, believed at `now`. `Proven::by` is the only way
/// to one, so a push to the base branch that asks for one cannot be made on a
/// run of another tree, of another command, or on one gone stale.
#[derive(Debug)]
#[refined_by(tree: int, command: int, at: int, now: int)]
#[invariant(fresh(at, now))]
// The keys and the time are what the invariant is stated on: nothing reads them back.
#[allow(dead_code)]
pub struct Proven {
    name: String,
    #[field(u64[tree])]
    tree: u64,
    #[field(u64[command])]
    command: u64,
    #[field(u64[at])]
    at: u64,
    #[field(u64[now])]
    now: u64,
}

impl Proven {
    /// `run` as the proof that `tree` passes `command` at `now`; `None` when it
    /// is not that.
    #[spec(fn(tree: &Tree[@t], command: &GateCommand[@c], run: &GreenRun, now: u64)
        -> Option<Proven{p: p.tree == t && p.command == c && p.now == now}>)]
    pub fn by(tree: &Tree, command: &GateCommand, run: &GreenRun, now: u64) -> Option<Proven> {
        if run.tree == tree.key && run.command == command.key && run.stands(now) {
            Some(Proven {
                name: tree.name.clone(),
                tree: run.tree,
                command: run.command,
                at: run.at,
                now,
            })
        } else {
            None
        }
    }

    /// The tree it is the proof of, by name: what a commit must hold to be
    /// pushed on it.
    pub fn tree(&self) -> &str {
        &self.name
    }

    /// When the run that proves it finished.
    #[spec(fn(&Proven[@p]) -> u64[p.at])]
    pub fn at(&self) -> u64 {
        self.at
    }
}

/// How a run that reached the gate limit of `gate_seconds` ended.
pub fn reached(gate_seconds: u64) -> String {
    format!("reached the gate limit of {gate_seconds}s and was ended")
}

/// What a run of the gate does to its tree's record on the remote: `written`
/// when it puts a run on record, the one of `command` on `tree` finished at `at`.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(written: bool, tree: int, command: int, at: int)]
pub enum Recording {
    /// Green, on a committed tree: put the run on record.
    #[variant((GreenRun[@g]) -> Recording[true, g.tree, g.command, g.at])]
    Write(GreenRun),
    /// Red, on a committed tree: the tree's record goes — the latest run of a
    /// tree is the one believed.
    #[variant(Recording[false, 0, 0, 0])]
    Drop,
    /// The run was not of a committed tree: it proves nothing about one.
    #[variant(Recording[false, 0, 0, 0])]
    Leave,
}

/// What a run of `command` in a worktree whose commit holds `tree` does to
/// the record, given what the code that ran it saw.
///
/// A run is of the committed tree only when the worktree was exactly its
/// commit before and after: a command that rewrites a tracked file passes on
/// what it wrote, not on what is committed. The run it puts on record is of
/// this command on this tree, finished at `now`, and says how long it took on
/// `host`.
#[spec(fn(tree: &Tree[@t], command: &GateCommand[@c], ran: Ran, host: &str, now: u64)
    -> Recording{r: r.written => r.tree == t && r.command == c && r.at == now})]
pub fn after_run(tree: &Tree, command: &GateCommand, ran: Ran, host: &str, now: u64) -> Recording {
    if !ran.clean_before {
        Recording::Leave
    } else if !ran.green() {
        Recording::Drop
    } else if !ran.clean_after {
        Recording::Leave
    } else {
        let took = Took::on(host, ran.seconds);
        Recording::Write(GreenRun {
            tree: tree.key,
            command: command.key,
            at: now,
            took,
            machine: None,
            tests: vec![],
        })
    }
}

impl Recording {
    /// The same recording, its run (where it writes one) timing `tests`
    /// (`test_times` of the run's output).
    #[spec(fn(Recording[@r], Vec<TestTime>) -> Recording[r])]
    pub fn with_tests(self, tests: Vec<TestTime>) -> Recording {
        match self {
            Recording::Write(green) => Recording::Write(green.with_tests(tests)),
            other => other,
        }
    }

    /// The same recording, its run (where it writes one) as having run on a
    /// machine of kind `machine` (`machine`).
    #[spec(fn(Recording[@r], &str) -> Recording[r])]
    pub fn on_machine(self, machine: &str) -> Recording {
        match self {
            Recording::Write(green) => Recording::Write(green.on_machine(machine)),
            other => other,
        }
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A tree landed on a run nobody asked about: it may be a week old.
    #[should_fail]
    #[spec(fn(run: &GreenRun, now: u64) -> Proven)]
    fn a_run_landed_on_without_asking_whether_it_stands(run: &GreenRun, now: u64) -> Proven {
        Proven { name: String::new(), tree: run.tree, command: run.command, at: run.at, now }
    }
}

/// What callers may rely on: each is checked by Flux, and stops passing when
/// the spec it reads is weakened.
#[cfg(flux)]
#[allow(dead_code)]
mod relied_on {
    use super::*;

    /// A proof is of the tree, the command and the time it was asked about.
    #[spec(fn(tree: &Tree[@t], command: &GateCommand[@c], run: &GreenRun, now: u64)
        -> Option<Proven{p: p.tree == t && p.command == c && p.now == now}>)]
    fn a_proof_is_of_what_was_asked(
        tree: &Tree,
        command: &GateCommand,
        run: &GreenRun,
        now: u64,
    ) -> Option<Proven> {
        Proven::by(tree, command, run, now)
    }

    /// A record read is a run of the tree and the command it was read for.
    #[spec(fn(subject: &str, tree: &Tree[@t], command: &GateCommand[@c])
        -> Option<GreenRun{g: g.tree == t && g.command == c}>)]
    fn a_record_is_of_what_it_was_read_for(
        subject: &str,
        tree: &Tree,
        command: &GateCommand,
    ) -> Option<GreenRun> {
        GreenRun::recorded(subject, tree, command)
    }

    /// The run a gate run puts on record is of its tree and its command,
    /// finished when the run did.
    #[spec(fn(tree: &Tree[@t], command: &GateCommand[@c], ran: Ran, now: u64)
        -> Option<GreenRun[t, c, now]>)]
    fn a_run_on_record_is_of_its_tree(
        tree: &Tree,
        command: &GateCommand,
        ran: Ran,
        now: u64,
    ) -> Option<GreenRun> {
        match after_run(tree, command, ran, "host", now) {
            Recording::Write(run) => Some(run),
            Recording::Drop | Recording::Leave => None,
        }
    }

    /// A record an hour old is believed, not swept.
    #[spec(fn(at: u64, now: u64) requires at <= now && now - at <= 3600)]
    fn a_record_an_hour_old_is_believed(at: u64, now: u64) {
        flux_rs::assert(believed(at, now));
    }

    /// A run stands exactly while it is fresh.
    #[spec(fn(run: &GreenRun[@g], now: u64) -> bool[super::fresh(g.at, now)])]
    fn a_run_stands_while_fresh(run: &GreenRun, now: u64) -> bool {
        run.stands(now)
    }
}

/// How many lines of a red gate's output are quoted from its first failure
/// on, or from its end where it names no failing test.
pub const QUOTE_LINES: usize = 60;

/// The statuses nextest names a test that failed by, at the start of its
/// line: `FAIL`, `TIMEOUT`, `LEAK-FAIL`, `ABORT`, and a signal (`SIGSEGV`).
fn failed_status(word: &str) -> bool {
    matches!(word, "FAIL" | "TIMEOUT" | "LEAK-FAIL" | "ABORT") || word.starts_with("SIG")
}

/// The test a line of nextest's output names as failing, as nextest names
/// it — its binary, then the test: `afkconvoy cutting::a_test`. A line of a
/// retry (`TRY 2 FAIL`) names one too; `None` of every other line.
pub fn failed_test(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    if let Some(retry) = rest.strip_prefix("TRY ") {
        rest = retry.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
    }
    let (status, rest) = rest.split_once(' ')?;
    if !failed_status(status) {
        return None;
    }
    let rest = rest.trim_start().strip_prefix('[')?;
    let (_, rest) = rest.split_once(']')?;
    let mut rest = rest.trim_start();
    if let Some(counted) = rest.strip_prefix('(') {
        rest = counted.split_once(')')?.1.trim_start();
    }
    let name = rest.trim_end();
    (!name.is_empty()).then_some(name)
}

/// The test a line of libtest's output — a plain `cargo test` run's — names
/// as failing, as libtest names it, without its binary: `test a_test ...
/// FAILED` or, opening what it printed, `---- a_test stdout ----`. `None` of
/// every other line.
pub fn libtest_failed(line: &str) -> Option<&str> {
    let line = line.trim();
    let name = match line.strip_prefix("test ") {
        Some(rest) => rest.strip_suffix(" ... FAILED")?,
        None => line.strip_prefix("---- ")?.strip_suffix(" stdout ----")?,
    };
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

/// The test binary cargo's line after a failed one names, as nextest names
/// a binary — the crate, then its target: `guildd::interface` of `error:
/// test failed, to rerun pass `-p guildd --test interface``, `guildd` of
/// `-p guildd --lib`. In a one-crate repo, which cargo names no `-p` of, it
/// is the target alone. `None` of every other line.
fn rerun_binary(line: &str) -> Option<String> {
    let (_, args) = line.split_once("to rerun pass `")?;
    let args = args.split('`').next()?;
    binary_of(args)
}

/// The test binary `args`, cargo's way to run it again — `-p guildd --test
/// interface` — names, as `rerun_binary` names it.
fn binary_of(args: &str) -> Option<String> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let after = |flag: &str| {
        words.iter().position(|word| *word == flag).and_then(|at| words.get(at + 1)).copied()
    };
    let krate = after("-p").or_else(|| after("--package"));
    let target =
        [("--test", ""), ("--bin", "bin/"), ("--example", "example/"), ("--bench", "bench/")]
            .iter()
            .find_map(|(flag, kind)| after(flag).map(|name| format!("{kind}{name}")));
    match (krate, target) {
        (Some(krate), Some(target)) => Some(format!("{krate}::{target}")),
        (Some(krate), None) => Some(krate.to_string()),
        (None, Some(target)) => Some(target),
        (None, None) => None,
    }
}

/// The tests a plain `cargo test` run's output `lines` names as failing
/// (`libtest_failed`), each once, in the order it first names them, and
/// each named as its binary, then the test, where cargo named the binary:
/// after it, by the way to run it again (`rerun_binary`), or, with
/// `--no-fail-fast`, in its list of the targets that failed at the end.
fn libtest_failing(lines: &[String]) -> Vec<String> {
    // The tests of each binary that failed, in the order it ran, and
    // whether cargo named that binary yet.
    let mut binaries: Vec<(Vec<String>, bool)> = vec![(vec![], false)];
    let mut listed: Vec<String> = vec![];
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("Running ") {
            if !binaries.last().is_some_and(|(tests, _)| tests.is_empty()) {
                binaries.push((vec![], false));
            }
        } else if let Some(name) = libtest_failed(line) {
            let (tests, _) = binaries.last_mut().expect("a binary");
            if !tests.iter().any(|named| named == name) {
                tests.push(name.to_string());
            }
        } else if let Some(binary) = rerun_binary(line) {
            if let Some((tests, named)) =
                binaries.iter_mut().rev().find(|(tests, named)| !named && !tests.is_empty())
            {
                for test in tests.iter_mut() {
                    *test = format!("{binary} {test}");
                }
                *named = true;
            }
        } else if let Some(args) = trimmed.strip_prefix('`').and_then(|rest| rest.strip_suffix('`'))
            && let Some(binary) = binary_of(args)
        {
            listed.push(binary);
        }
    }
    let mut listed = listed.into_iter();
    let mut failing: Vec<String> = vec![];
    for (tests, named) in binaries.into_iter().filter(|(tests, _)| !tests.is_empty()) {
        let binary = if named { None } else { listed.next() };
        for test in tests {
            let test = match &binary {
                Some(binary) => format!("{binary} {test}"),
                None => test,
            };
            if !failing.contains(&test) {
                failing.push(test);
            }
        }
    }
    failing
}

/// The tests a red gate's output `lines` names as failing, each once, in the
/// order it first names them: nextest's (`failed_test`) — a test a later
/// retry passed is not among them — or, where nextest names none, those of
/// a plain `cargo test` run (`libtest_failing`). nextest prints what a
/// failing test printed, libtest's lines among it, so these are read only
/// where it names none.
pub fn failing_tests(lines: &[String]) -> Vec<String> {
    let mut failing: Vec<String> = vec![];
    for line in lines {
        if let Some(name) = failed_test(line)
            && !failing.iter().any(|named| named == name)
        {
            failing.push(name.to_string());
        }
        let passed = line.trim_start();
        if (passed.starts_with("TRY ") && passed.contains(" PASS ")) || passed.starts_with("FLAKY ")
        {
            failing.retain(|named| !passed.ends_with(named.as_str()));
        }
    }
    if failing.is_empty() && !lines.iter().any(|line| failed_test(line).is_some()) {
        return libtest_failing(lines);
    }
    failing
}

/// The status a line of nextest's output names its failing test by — `FAIL`,
/// `TIMEOUT`, a retry's (`TRY 2 FAIL`) — where it names one.
fn failed_status_of(line: &str) -> Option<&str> {
    failed_test(line)?;
    let mut rest = line.trim_start();
    if let Some(retry) = rest.strip_prefix("TRY ") {
        rest = retry.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
    }
    rest.split_once(' ').map(|(status, _)| status)
}

/// What a gate says of a TIMEOUT it cannot tell from load: a line of its
/// own that says the host is overloaded and that it cannot classify the
/// tests — `gate: host overloaded or load unavailable (load 12.71, CPUs 12);
/// cannot classify TIMEOUT tests.` — or one of them, naming it — `gate: host
/// overloaded (load 13, CPUs 12); cannot classify afkconvoy cutting::a_test.`
fn cannot_classify(line: &str, test: &str) -> bool {
    let Some(after) = classifies_not(line) else {
        return false;
    };
    after.starts_with("TIMEOUT tests")
        || after.strip_prefix(test).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// Whether a red gate's output `lines` is red only on TIMEOUTs it said it
/// cannot classify because the host is overloaded: it names a failing test
/// (`failing_tests`), every one of them failed by `TIMEOUT` alone, and for
/// each the gate said so (`cannot_classify`). Such a run says nothing of the
/// tree: a quieter host may pass it. A FAIL beside them, a TIMEOUT the gate
/// did not say it cannot classify — it ran on a host not overloaded — or a
/// red that names no test is a red gate.
pub fn overloaded_timeouts(lines: &[String]) -> bool {
    let failing = failing_tests(lines);
    if failing.is_empty() {
        return false;
    }
    failing.iter().all(|test| {
        let timed_out = lines.iter().all(|line| match failed_test(line) {
            Some(name) if name == test => failed_status_of(line) == Some("TIMEOUT"),
            _ => true,
        });
        let named = lines.iter().any(|line| failed_test(line) == Some(test.as_str()));
        timed_out && named && lines.iter().any(|line| cannot_classify(line, test))
    })
}

/// The last line of a gate's output `lines` that says the host is overloaded
/// and it cannot classify a TIMEOUT, with the load it read: what an
/// unsettled run is told by.
pub fn overload_said(lines: &[String]) -> Option<&str> {
    lines.iter().map(|line| line.trim()).rfind(|line| classifies_not(line).is_some())
}

/// What a gate's line says it cannot classify on an overloaded host, where
/// it says so: the rest of the line after `cannot classify `.
fn classifies_not(line: &str) -> Option<&str> {
    let (before, after) = line.trim().split_once("cannot classify ")?;
    before.contains("host overloaded").then_some(after)
}

/// What a red gate's output `lines` is quoted by: `QUOTE_LINES` from the
/// line that names its first failing test — nextest prints what the test
/// printed after it — or, where nextest names none, from the first line of
/// a plain `cargo test` run's that does (`libtest_failed`), or, where no
/// line names one, its last `QUOTE_LINES`.
pub fn quoted(lines: &[String]) -> Vec<String> {
    let first = lines
        .iter()
        .position(|line| failed_test(line).is_some())
        .or_else(|| lines.iter().position(|line| libtest_failed(line).is_some()));
    let from = first.unwrap_or(lines.len().saturating_sub(QUOTE_LINES));
    lines.iter().skip(from).take(QUOTE_LINES).cloned().collect()
}

/// What wait status proves about a gate command ended by a signal. The pid
/// also names its process group, created by the gate. Wait status identifies
/// no sender and proves nothing about which repository checks started.
/// Its fields are in the order a JSON object's keys are written, so a
/// candidate's evidence kept with it reads as it did untyped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Termination {
    pub core_dumped: bool,
    pub pid: u32,
    pub signal: i32,
}

impl Termination {
    pub fn description(&self) -> String {
        format!(
            "gate command pid {} (process group {}) ended by signal {} (core dumped: {}); \
             signal sender unknown; repository checks may not have started",
            self.pid, self.pid, self.signal, self.core_dumped
        )
    }
}

/// How a red run of the gate ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// By itself, with this exit code.
    Exited(i32),
    /// At the gate limit of this many seconds, by the code that ran it.
    AtLimit(u64),
    /// By a signal, outside the gate limit.
    Signalled(Termination),
}

/// A red run of the gate: how it ended, the tests its whole output names as
/// failing (`failing_tests`), and what that output is quoted by (`quoted`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Red {
    pub ended: Ended,
    pub tests: Vec<String>,
    pub quote: Vec<String>,
}

impl Red {
    /// How the run went red and the tests it named failing, the one wording
    /// of it every gate's answer uses: `it exited 1; failing: nx a_test`.
    pub fn how(&self) -> String {
        let ended = match self.ended {
            Ended::Exited(code) => format!("it exited {code}"),
            Ended::AtLimit(seconds) => format!("its run {}", reached(seconds)),
            Ended::Signalled(termination) => termination.description(),
        };
        match self.tests.as_slice() {
            [] => format!("{ended}; its output names no failing test"),
            tests => format!("{ended}; failing: {}", tests.join(", ")),
        }
    }

    /// That `what` is red, and how: `the gate is red: it exited 1; failing:
    /// nx a_test`.
    pub fn said(&self, what: &str) -> String {
        format!("{what} is red: {}", self.how())
    }
}

/// A red run that says nothing of its tree: it ended red before the gate
/// limit only on TIMEOUTs the gate said it cannot classify because the host
/// is overloaded (`overloaded_timeouts`). A quieter host may pass it. It is
/// recorded neither green nor red.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsettled {
    /// The run, as red: its failing tests are the TIMEOUTs.
    pub red: Red,
    /// The gate's line that says the host is overloaded, with its load
    /// (`overload_said`).
    pub said: String,
}

impl Unsettled {
    /// The TIMEOUT tests, as the run named them.
    pub fn tests(&self) -> &[String] {
        &self.red.tests
    }

    /// Why the run says nothing of its tree, the one wording of it:
    /// `red only on TIMEOUTs it cannot classify on an overloaded host (nx
    /// a_test) — `gate: host overloaded (load 13, CPUs 12); …``.
    pub fn how(&self) -> String {
        format!(
            "red only on TIMEOUTs it cannot classify on an overloaded host ({}) — `{}`",
            self.tests().join(", "),
            self.said
        )
    }
}

/// What one run of the gate says of its tree, judged by `judge`: green, red,
/// or nothing (`Unsettled`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(green: bool, unsettled: bool)]
pub enum Judged {
    #[variant(Judged[true, false])]
    Green,
    #[variant((Red) -> Judged[false, false])]
    Red(Red),
    #[variant((Unsettled) -> Judged[false, true])]
    Unsettled(Unsettled),
}

/// Judges a run of the gate from how it ended — `exit_code`, `timed_out` at
/// the gate limit of `limit` seconds, the signal that ended it — and its
/// whole output, `lines`. Only exit 0 before the limit is green. A run that
/// reached the limit is red whatever it printed; one that ended before it
/// red only on TIMEOUTs the gate said it cannot classify on an overloaded
/// host is unsettled (`overloaded_timeouts`); every other is red.
#[spec(fn(exit_code: i32, timed_out: bool, limit: u64, termination: Option<Termination>,
    lines: &[String]) -> Judged{j: (j.green <=> (exit_code == 0 && !timed_out))
        && (j.unsettled => !timed_out)})]
pub fn judge(
    exit_code: i32,
    timed_out: bool,
    limit: u64,
    termination: Option<Termination>,
    lines: &[String],
) -> Judged {
    if exit_code == 0 && !timed_out {
        return Judged::Green;
    }
    let ended = match (timed_out, termination) {
        (true, _) => Ended::AtLimit(limit),
        (false, Some(termination)) => Ended::Signalled(termination),
        (false, None) => Ended::Exited(exit_code),
    };
    let red = Red { ended, tests: failing_tests(lines), quote: quoted(lines) };
    if timed_out || !overloaded_timeouts(lines) {
        return Judged::Red(red);
    }
    let said = overload_said(lines).unwrap_or_default().to_string();
    Judged::Unsettled(Unsettled { red, said })
}

/// The word a recorded run's line for one of its tests opens with.
pub const TEST: &str = "convoy:test";

/// What a gate's output starts a line with to say a test's CPU time, as
/// its runner measured it (the test process's `getrusage`, say), in
/// nextest's own spelling: `gate: cpu [   0.010s] nx tests::passes`. nextest
/// says a test's wall time, not its CPU time.
pub const CPU_SAID: &str = "gate: cpu ";

/// One test a gate run's output timed, by its name as nextest names it —
/// its binary, then the test: `afkconvoy cutting::a_test`. Each time is in
/// milliseconds, as nextest says them, and `None` where the output said
/// none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestTime {
    pub name: String,
    /// Its wall time, from the line nextest passed it on.
    pub wall_ms: Option<u64>,
    /// Its CPU time, from a line `CPU_SAID` opens.
    pub cpu_ms: Option<u64>,
}

impl TestTime {
    /// The line of a recorded run's message that carries this test.
    fn write(&self) -> String {
        let wall = self.wall_ms.map(|ms| ms.to_string()).unwrap_or_default();
        let cpu = self.cpu_ms.map(|ms| ms.to_string()).unwrap_or_default();
        record::write(TEST, &[("name", &self.name), ("wall_ms", &wall), ("cpu_ms", &cpu)])
    }

    /// A line of a recorded run's message → the test it carries; `None` when
    /// it carries none.
    fn read(line: &str) -> Option<TestTime> {
        let fields = record::read(TEST, line)?;
        let name = fields.get("name").filter(|name| !name.is_empty())?.to_string();
        Some(TestTime { name, wall_ms: fields.number("wall_ms"), cpu_ms: fields.number("cpu_ms") })
    }
}

/// A time as nextest writes one between brackets, `   0.012s` → its
/// milliseconds; at most three decimals, as it writes them.
fn said_ms(raw: &str) -> Option<u64> {
    let raw = raw.trim().strip_suffix('s')?;
    let (whole, part) = raw.split_once('.').unwrap_or((raw, ""));
    let digits = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || whole.len() > 12 || part.len() > 3 || !digits(whole) || !digits(part) {
        return None;
    }
    let whole: u64 = whole.parse().ok()?;
    let part: u64 = format!("{part:0<3}").parse().ok()?;
    let ms = whole.saturating_mul(1000).saturating_add(part);
    Some(ms)
}

/// `[ time] (count) name`, the rest of a nextest status line → the time
/// and the name. The count is nextest's progress, where it writes one.
fn timed(rest: &str) -> Option<(u64, &str)> {
    let rest = rest.trim_start().strip_prefix('[')?;
    let (time, rest) = rest.split_once(']')?;
    let ms = said_ms(time)?;
    let mut rest = rest.trim_start();
    if let Some(counted) = rest.strip_prefix('(') {
        rest = counted.split_once(')')?.1.trim_start();
    }
    let name = rest.trim_end();
    (!name.is_empty()).then_some((ms, name))
}

/// A line of nextest's output that passes a test, its retry's (`TRY 2
/// PASS`) or one that leaked (`LEAK`) → the test's wall time and name.
fn passed_test(line: &str) -> Option<(u64, &str)> {
    let mut rest = line.trim_start();
    if let Some(retry) = rest.strip_prefix("TRY ") {
        rest = retry.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
    }
    let (status, rest) = rest.split_once(' ')?;
    if !matches!(status, "PASS" | "LEAK") {
        return None;
    }
    timed(rest)
}

/// Each test a gate run's output `lines` times, once, in the order it first
/// names them: its wall time from the line nextest passed it on — the last
/// such, of a test retried — and its CPU time from a line `CPU_SAID` opens.
/// Empty for output that times none.
pub fn test_times(lines: &[String]) -> Vec<TestTime> {
    let mut tests: Vec<TestTime> = vec![];
    for line in lines {
        let (ms, name, cpu) = match line.trim_start().strip_prefix(CPU_SAID) {
            Some(rest) => match timed(rest) {
                Some((ms, name)) => (ms, name, true),
                None => continue,
            },
            None => match passed_test(line) {
                Some((ms, name)) => (ms, name, false),
                None => continue,
            },
        };
        let (wall_ms, cpu_ms) = if cpu { (None, Some(ms)) } else { (Some(ms), None) };
        match tests.iter_mut().find(|test| test.name == name) {
            Some(test) if cpu => test.cpu_ms = cpu_ms,
            Some(test) => test.wall_ms = wall_ms,
            None => tests.push(TestTime { name: name.to_string(), wall_ms, cpu_ms }),
        }
    }
    tests
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_red_gate_s_output_names_its_failing_tests_and_is_quoted_from_the_first() {
        let lines: Vec<String> = [
            "    Starting 3 tests across 1 binary",
            "        PASS [   0.012s] (1/3) nx tests::passes",
            "        FAIL [   0.012s] (2/3) nx tests::fails_here",
            "  stderr ───",
            "    thread 'tests::fails_here' panicked at src/lib.rs:4:56:",
            "    TRY 1 FAIL [   1.000s] (3/3) nx tests::flaky",
            "    TRY 2 PASS [   1.000s] (3/3) nx tests::flaky",
            "   TIMEOUT [ 240.004s] nx tests::slow",
            "     Summary [   0.017s] 3 tests run: 1 passed, 2 failed, 0 skipped",
            "        FAIL [   0.012s] (2/3) nx tests::fails_here",
            "gate: the seconds each step took: fmt 1s",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(failing_tests(&lines), ["nx tests::fails_here", "nx tests::slow"]);
        let quote = quoted(&lines);
        assert_eq!(quote[0], lines[2]);
        assert!(quote.iter().any(|line| line.contains("panicked at")), "{quote:?}");
        assert_eq!(failed_test("        PASS [   0.012s] (1/3) nx tests::passes"), None);
        assert_eq!(failed_test("FAIL to start"), None);
        // No test named: its end is quoted, not only its last line.
        let built: Vec<String> = (0..100).map(|n| format!("line {n}")).collect();
        assert!(failing_tests(&built).is_empty());
        let quote = quoted(&built);
        assert_eq!((quote.len(), quote[0].as_str()), (QUOTE_LINES, "line 40"));
    }

    /// A gate that runs plain `cargo test` names its failing tests as
    /// nextest would — the crate's test binary, then the test — and is
    /// quoted from the first (sunfmin/control#1028).
    #[test]
    fn a_red_plain_cargo_test_run_names_its_failing_tests_and_is_quoted_from_the_first() {
        let test =
            "an_instance_started_from_the_hub_is_kept_within_its_machines_limits_by_its_own_guildd";
        let lines = said(&[
            "     Running tests/interface.rs (target/debug/deps/interface-0a1b2c3d)",
            "running 14 tests",
            &format!("test {test} has been running for over 60 seconds"),
            &format!("test {test} ... FAILED"),
            "failures:",
            &format!("---- {test} stdout ----"),
            "thread '…' panicked at crates/guildd/tests/interface.rs:515:5:",
            "failures:",
            &format!("    {test}"),
            "test result: FAILED. 13 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
            "error: test failed, to rerun pass `-p guildd --test interface`",
        ]);
        let named = format!("guildd::interface {test}");
        assert_eq!(failing_tests(&lines), [named.as_str()]);
        let quote = quoted(&lines);
        assert_eq!(quote[0], lines[3]);
        assert!(quote.iter().any(|line| line.contains("panicked at")), "{quote:?}");
        let Judged::Red(red) = judge(101, false, 600, None, &lines) else {
            panic!("a FAILED test is red");
        };
        assert_eq!(red.how(), format!("it exited 101; failing: {named}"));
        // An overloaded host's line does not make a plain FAILED unsettled.
        let overloaded = "gate: host overloaded or load unavailable (load 603, CPUs 16); cannot \
                          classify TIMEOUT tests.";
        let mut also = lines.clone();
        also.push(overloaded.to_string());
        assert!(!overloaded_timeouts(&also));

        // With --no-fail-fast cargo lists the failed targets at the end, in
        // the order they ran; a one-crate repo's binaries have no crate.
        let lines = said(&[
            "     Running unittests src/lib.rs (target/debug/deps/nx-1)",
            "test tests::fails_here ... FAILED",
            "     Running tests/cli.rs (target/debug/deps/cli-2)",
            "test ok_test ... ok",
            "     Running tests/more.rs (target/debug/deps/more-3)",
            "test a ... FAILED",
            "test b ... FAILED",
            "error: 2 targets failed:",
            "    `-p nx --lib`",
            "    `-p nx --test more`",
        ]);
        assert_eq!(failing_tests(&lines), ["nx tests::fails_here", "nx::more a", "nx::more b"]);
        let one = said(&["test it ... FAILED", "error: test failed, to rerun pass `--bin tool`"]);
        assert_eq!(failing_tests(&one), ["bin/tool it"]);
        // Where nextest names a failing test, libtest's lines it printed are not read again.
        let nextest = said(&[
            "        FAIL [   0.012s] (2/3) nx tests::fails_here",
            "test tests::fails_here ... FAILED",
        ]);
        assert_eq!(failing_tests(&nextest), ["nx tests::fails_here"]);
        assert_eq!(libtest_failed("test a ... ok"), None);
    }

    fn said(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn a_red_gate_only_of_timeouts_it_cannot_classify_under_overload_is_told_apart() {
        let timeout = "     TIMEOUT [   6.004s] (40/669) afkconvoy::cli dsh::the_line";
        let all = "gate: host overloaded or load unavailable (load 12.71, CPUs 12); cannot \
                   classify TIMEOUT tests. Retry on a quiet host.";
        let one = "gate: host overloaded (load 13, CPUs 12); cannot classify afkconvoy::cli \
                   dsh::the_line. Retry on a quiet host.";
        assert!(overloaded_timeouts(&said(&[timeout, timeout, all])));
        assert!(overloaded_timeouts(&said(&[timeout, one])));
        assert_eq!(overload_said(&said(&[timeout, all, timeout])), Some(all));
        // A TIMEOUT on a host the gate did not say is overloaded is red.
        let quiet = "gate: afkconvoy::cli dsh::the_line was still TIMEOUT at 2s on a quiet host.";
        assert!(!overloaded_timeouts(&said(&[timeout, quiet])));
        assert!(!overloaded_timeouts(&said(&[timeout])));
        assert_eq!(overload_said(&said(&[timeout, quiet])), None);
        // A FAIL beside it is red, and so is a red that names no test.
        let fail = "        FAIL [   0.208s] (2/3) afkconvoy cutting::a_named_test";
        assert!(!overloaded_timeouts(&said(&[timeout, fail, all])));
        assert!(!overloaded_timeouts(&said(&["error: could not compile", all])));
        // The one test named is not another's line, nor a TIMEOUT retried as FAIL.
        let other = "gate: host overloaded (load 13, CPUs 12); cannot classify afkconvoy::cli \
                     dsh::the_line_too.";
        assert!(!overloaded_timeouts(&said(&[timeout, other])));
        let retried = "    TRY 2 FAIL [   1.000s] (40/669) afkconvoy::cli dsh::the_line";
        assert!(!overloaded_timeouts(&said(&[timeout, retried, all])));
    }

    /// Every gate run is judged one way, and its red worded one way
    /// (sunfmin/control#619): green; red on a FAIL; red on a TIMEOUT the
    /// gate classified; unsettled on TIMEOUTs it cannot classify under
    /// overload; red at the gate limit, whatever its output says.
    #[test]
    fn a_gate_run_is_judged_green_red_or_unsettled_and_its_red_worded_once() {
        let timeout = "     TIMEOUT [   6.004s] (40/669) afkconvoy::cli dsh::the_line";
        let overloaded = "gate: host overloaded or load unavailable (load 12.71, CPUs 12); \
                          cannot classify TIMEOUT tests. Retry on a quiet host.";
        let quiet = "gate: afkconvoy::cli dsh::the_line was still TIMEOUT at 2s on a quiet host.";
        let fail = "        FAIL [   0.208s] (2/3) afkconvoy cutting::a_named_test";
        assert_eq!(judge(0, false, 600, None, &said(&[fail])), Judged::Green);

        let Judged::Red(red) = judge(100, false, 600, None, &said(&["built", fail, "end"])) else {
            panic!("a FAIL is red");
        };
        assert_eq!(red.ended, Ended::Exited(100));
        assert_eq!(red.tests, ["afkconvoy cutting::a_named_test"]);
        assert_eq!(red.quote, said(&[fail, "end"]), "quoted from the first failure");
        assert_eq!(
            red.said("the gate"),
            "the gate is red: it exited 100; failing: afkconvoy cutting::a_named_test"
        );

        let Judged::Red(red) = judge(100, false, 600, None, &said(&[timeout, quiet])) else {
            panic!("a TIMEOUT the gate classified is red");
        };
        assert_eq!(red.how(), "it exited 100; failing: afkconvoy::cli dsh::the_line");

        let Judged::Unsettled(unsettled) =
            judge(100, false, 600, None, &said(&[timeout, overloaded]))
        else {
            panic!("only TIMEOUTs the gate cannot classify under overload say nothing");
        };
        assert_eq!(unsettled.tests(), ["afkconvoy::cli dsh::the_line"]);
        assert_eq!(unsettled.said, overloaded);
        assert_eq!(
            unsettled.how(),
            format!(
                "red only on TIMEOUTs it cannot classify on an overloaded host \
                 (afkconvoy::cli dsh::the_line) — `{overloaded}`"
            )
        );
        assert_eq!(unsettled.red.how(), "it exited 100; failing: afkconvoy::cli dsh::the_line");

        let Judged::Red(red) = judge(124, true, 600, None, &said(&[timeout, overloaded])) else {
            panic!("a run that reached the gate limit is red, whatever it printed");
        };
        assert_eq!(
            red.how(),
            "its run reached the gate limit of 600s and was ended; failing: afkconvoy::cli \
             dsh::the_line"
        );
        assert_eq!(judge(0, true, 1, None, &[]), judge(124, true, 1, None, &[]), "not green");

        let ended = Termination { pid: 7, signal: 15, core_dumped: false };
        let Judged::Red(red) = judge(-1, false, 600, Some(ended), &[]) else {
            panic!("a signal is red");
        };
        assert_eq!(red.ended, Ended::Signalled(ended));
        assert_eq!(red.how(), format!("{}; its output names no failing test", ended.description()));
    }

    fn tree() -> Tree {
        Tree::named("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
    }

    fn command() -> GateCommand {
        GateCommand::new("make test")
    }

    fn ran(exit_code: i32) -> Ran {
        Ran { exit_code, timed_out: false, clean_before: true, clean_after: true, seconds: 42 }
    }

    fn written(now: u64) -> GreenRun {
        match after_run(&tree(), &command(), ran(0), "mini", now) {
            Recording::Write(run) => run,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_tree_is_proven_only_by_a_standing_run_of_it() {
        let (tree, command, run) = (tree(), command(), written(1000));
        let proven = Proven::by(&tree, &command, &run, 1000 + RECORD_TTL).expect("a day old");
        assert_eq!((proven.tree(), proven.at()), (tree.name(), 1000));
        assert!(Proven::by(&tree, &command, &run, 1001 + RECORD_TTL).is_none(), "gone stale");
        assert!(Proven::by(&Tree::named("other"), &command, &run, 1000).is_none());
        assert!(Proven::by(&tree, &GateCommand::new("true"), &run, 1000).is_none());
    }

    #[test]
    fn the_whole_gate_passes_on_a_tree_whose_fast_and_slow_parts_passed_there() {
        let (tree, whole) = (tree(), command());
        let (fast, slow) = (GateCommand::new("./gate.sh fast"), GateCommand::new("./gate.sh slow"));
        let run = |command: &GateCommand, at| match after_run(&tree, command, ran(0), "mini", at) {
            Recording::Write(run) => run,
            other => panic!("{other:?}"),
        };
        let (f, s) = (run(&fast, 1000), run(&slow, 2000));
        let joined = GreenRun::joined(&tree, &whole, &fast, &f, &slow, &s).expect("both passed");
        assert_eq!(joined.at(), 1000, "as old as the older part");
        assert_eq!(joined.took().map(|t| (t.seconds(), t.host())), Some((84, "mini")));
        assert!(Proven::by(&tree, &whole, &joined, 1500).is_some());
        assert!(GreenRun::joined(&tree, &whole, &fast, &s, &slow, &f).is_none(), "parts swapped");
        assert!(GreenRun::joined(&tree, &whole, &fast, &f, &fast, &f).is_none(), "one part twice");
        let other = Tree::named("other");
        assert!(GreenRun::joined(&other, &whole, &fast, &f, &slow, &s).is_none());
    }

    #[test]
    fn only_exit_zero_is_green_and_a_timeout_is_red() {
        assert!(ran(0).green());
        assert!(!ran(1).green());
        assert!(!ran(-1).green());
        assert!(!Ran { timed_out: true, ..ran(0) }.green());
    }

    #[test]
    fn a_run_is_recorded_only_when_green_on_a_committed_tree() {
        let (tree, command) = (tree(), command());
        assert!(matches!(after_run(&tree, &command, ran(0), "mini", 50), Recording::Write(_)));
        assert_eq!(after_run(&tree, &command, ran(2), "mini", 50), Recording::Drop);
        assert_eq!(
            after_run(&tree, &command, Ran { timed_out: true, ..ran(0) }, "mini", 50),
            Recording::Drop
        );
        let dirty_before = Ran { clean_before: false, ..ran(0) };
        assert_eq!(after_run(&tree, &command, dirty_before, "mini", 50), Recording::Leave);
        let red_and_dirty = Ran { clean_before: false, ..ran(1) };
        assert_eq!(
            after_run(&tree, &command, red_and_dirty, "mini", 50),
            Recording::Leave,
            "it tested no commit's tree"
        );
        let rewrote = Ran { clean_after: false, ..ran(0) };
        assert_eq!(after_run(&tree, &command, rewrote, "mini", 50), Recording::Leave);
    }

    #[test]
    fn a_record_reads_back_for_its_own_tree_and_command_only() {
        let (tree, command) = (tree(), command());
        let subject = written(1000).write(&tree, &command);
        assert_eq!(GreenRun::recorded(&subject, &tree, &command).map(|run| run.at()), Some(1000));
        assert_eq!(GreenRun::recorded(&subject, &Tree::named("other"), &command), None);
        assert_eq!(GreenRun::recorded(&subject, &tree, &GateCommand::new("make  test")), None);
        assert_eq!(GreenRun::recorded("initial commit", &tree, &command), None);
        assert_eq!(GreenRun::recorded("convoy:gate tree=x", &tree, &command), None);
    }

    #[test]
    fn a_record_says_how_long_the_run_took_and_on_which_host() {
        let (tree, command) = (tree(), command());
        let subject = written(1000).write(&tree, &command);
        assert!(subject.ends_with(" at=1000 seconds=42 host=mini"), "{subject}");
        let more = written(1000).write_with(&tree, &command, &[("by", "7")]);
        assert!(more.ends_with(" host=mini by=7"), "{more}");
        assert_eq!(GreenRun::recorded(&more, &tree, &command).map(|run| run.at()), Some(1000));
        let red = format!("{subject} red=1");
        assert_eq!(GreenRun::recorded(&red, &tree, &command), None);
        let run = GreenRun::recorded(&subject, &tree, &command).unwrap();
        assert_eq!(run, written(1000), "what was written reads back");
        assert_eq!(run.took().map(|took| (took.seconds(), took.host())), Some((42, "mini")));
    }

    #[test]
    fn a_record_with_no_seconds_or_no_host_is_a_green_run_with_no_time() {
        let (tree, command) = (tree(), command());
        let whole = written(1000).write(&tree, &command);
        let before = whole.strip_suffix(" seconds=42 host=mini").unwrap();
        for subject in [
            before.to_string(),
            format!("{before} seconds=42"),
            format!("{before} host=mini"),
            format!("{before} seconds=soon host=mini"),
        ] {
            let run = GreenRun::recorded(&subject, &tree, &command).expect(&subject);
            assert_eq!((run.at(), run.took()), (1000, None), "{subject}");
            assert!(run.stands(1000 + RECORD_TTL), "it stands as long as it would have");
            assert!(Proven::by(&tree, &command, &run, 1000).is_some(), "{subject}");
            assert!(!outlived(&subject, 1000 + RECORD_TTL), "{subject}");
            assert_eq!(run.write(&tree, &command), before, "no time is written for it");
        }
        // a run on a host with no name is one with no time, as its record reads
        let nameless = match after_run(&tree, &command, ran(0), "", 1000) {
            Recording::Write(run) => run,
            other => panic!("{other:?}"),
        };
        assert_eq!(nameless.took(), None);
    }

    #[test]
    fn a_record_stands_for_one_day_and_never_from_the_future() {
        let run = written(100_000);
        assert!(run.stands(100_000));
        assert!(run.stands(100_000 + RECORD_TTL));
        assert!(!run.stands(100_001 + RECORD_TTL));
        assert!(run.stands(100_000 - SKEW), "ordinary skew between two hosts");
        assert!(!run.stands(99_999 - SKEW));
    }

    #[test]
    fn a_record_is_swept_once_no_landing_would_believe_it() {
        let subject = written(100_000).write(&tree(), &command());
        assert!(!outlived(&subject, 100_000));
        assert!(!outlived(&subject, 100_000 + RECORD_TTL), "a day old is still believed");
        assert!(outlived(&subject, 100_001 + RECORD_TTL));
        assert!(outlived(&subject, 99_999 - SKEW), "stamped from the future");
        assert!(outlived("initial commit", 100_000), "nobody can read it");
        assert!(outlived("convoy:gate tree=x command=y", 100_000));
    }

    #[test]
    fn the_ref_is_named_for_the_tree_and_the_command() {
        let name = record_ref(Layout::Hidden, &tree(), &command());
        assert!(name.starts_with("refs/convoy/gate/4b825dc642cb6eb9a060e54bf8d69288fbee4904-"));
        assert_eq!(name.len(), "refs/convoy/gate/".len() + 40 + 1 + 16);
        assert_ne!(name, record_ref(Layout::Hidden, &tree(), &GateCommand::new("make check")));
        assert_eq!(key("make test"), key("make test"));
    }

    /// A record of `run` as the remote would keep it under `layout`.
    fn kept(layout: Layout, run: &GreenRun) -> (String, String) {
        let (tree, command) = (tree(), command());
        (record_ref(layout, &tree, &command), run.write(&tree, &command))
    }

    fn believes(records: &[(&str, &str)], noted: Layout, now: u64) -> Option<u64> {
        believed_among(records, noted, &tree(), &command(), now).map(|run| run.at())
    }

    #[test]
    fn a_standing_green_run_is_believed_from_either_layout() {
        let run = written(1000);
        let hidden = kept(Layout::Hidden, &run);
        let branches = kept(Layout::Branches, &run);
        let hidden_only = [(hidden.0.as_str(), hidden.1.as_str())];
        let branch_only = [(branches.0.as_str(), branches.1.as_str())];
        assert_eq!(believes(&hidden_only, Layout::Branches, 1000), Some(1000));
        assert_eq!(believes(&branch_only, Layout::Hidden, 1000), Some(1000));
        assert_eq!(believes(&hidden_only, Layout::Hidden, 1000 + RECORD_TTL), Some(1000));

        let both = [hidden_only[0], branch_only[0]];
        let later = written(2000);
        let later_hidden = kept(Layout::Hidden, &later);
        let noted_wins = [(later_hidden.0.as_str(), later_hidden.1.as_str()), branch_only[0]];
        assert_eq!(believes(&both, Layout::Hidden, 1000), Some(1000));
        assert_eq!(
            believes(&noted_wins, Layout::Hidden, 2000),
            Some(2000),
            "the noted layout wins"
        );
    }

    #[test]
    fn a_day_old_record_a_red_run_and_no_record_are_not_believed_from_the_other_layout() {
        let run = written(1000);
        let branches = kept(Layout::Branches, &run);
        let branch_only = [(branches.0.as_str(), branches.1.as_str())];
        assert_eq!(believes(&branch_only, Layout::Hidden, 1001 + RECORD_TTL), None, "a day old");
        assert_eq!(believes(&[], Layout::Hidden, 1000), None, "a run that wrote no record");
        let red = (branches.0.as_str(), "initial commit");
        assert_eq!(believes(&[red], Layout::Hidden, 1000), None, "a red run wrote no green record");

        let stale = kept(Layout::Hidden, &run);
        let fresh = written(1000 + RECORD_TTL);
        let fresh_branches = kept(Layout::Branches, &fresh);
        let mixed = [
            (stale.0.as_str(), stale.1.as_str()),
            (fresh_branches.0.as_str(), fresh_branches.1.as_str()),
        ];
        let now = 1000 + RECORD_TTL + 1;
        assert_eq!(believes(&mixed, Layout::Hidden, now), Some(fresh.at()));
    }

    #[test]
    fn a_record_of_an_earlier_tree_is_not_a_run_of_the_commit_a_cut_lands() {
        let earlier = tree();
        let landed = Tree::named("later-tree-a-cut-commits");
        let command = command();
        let run = written(1000);
        let name = record_ref(Layout::Branches, &earlier, &command);
        let subject = run.write(&earlier, &command);
        let records = [(name.as_str(), subject.as_str())];
        assert!(believed_among(&records, Layout::Hidden, &landed, &command, 1000).is_none());
        assert_eq!(
            believed_among(&records, Layout::Hidden, &earlier, &command, 1000).map(|run| run.at()),
            Some(1000)
        );
    }

    fn lines(said: &[&str]) -> Vec<String> {
        said.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn a_runs_output_times_each_test_it_passed() {
        let output = lines(&[
            "    Starting 4 tests across 1 binary",
            "        PASS [   0.012s] (  1/669) nx tests::passes",
            "gate: cpu [   0.010s] nx tests::passes",
            "        SLOW [> 60.000s] nx tests::slow",
            "    TRY 1 FAIL [   1.000s] (2/4) nx tests::flaky",
            "    TRY 2 PASS [   1.5s] (2/4) nx tests::flaky",
            "        LEAK [1234.567s] (3/4) nx tests::leaky",
            "gate: cpu [   2.000s] nx tests::only_cpu",
            "        PASS [   oops] (4/4) nx tests::unreadable",
            "     Summary [   0.017s] 4 tests run: 4 passed, 0 skipped",
        ]);
        let timed = |name: &str, wall_ms, cpu_ms| TestTime { name: name.into(), wall_ms, cpu_ms };
        assert_eq!(
            test_times(&output),
            [
                timed("nx tests::passes", Some(12), Some(10)),
                timed("nx tests::flaky", Some(1500), None),
                timed("nx tests::leaky", Some(1_234_567), None),
                timed("nx tests::only_cpu", None, Some(2000)),
            ]
        );
        assert!(test_times(&lines(&["checking fast", "gate: green"])).is_empty());
    }

    fn green(tests: Vec<TestTime>) -> GreenRun {
        let ran = Ran {
            exit_code: 0,
            timed_out: false,
            clean_before: true,
            clean_after: true,
            seconds: 9,
        };
        match after_run(&Tree::named("abc"), &GateCommand::new("./gate.sh"), ran, "mini", 1000)
            .with_tests(tests)
        {
            Recording::Write(run) => run,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_recorded_run_reads_its_tests_back_exactly() {
        let (tree, command) = (Tree::named("abc"), GateCommand::new("./gate.sh"));
        let tests = vec![
            TestTime { name: "nx tests::a test=odd%".into(), wall_ms: Some(12), cpu_ms: Some(10) },
            TestTime { name: "nx 日本語".into(), wall_ms: None, cpu_ms: Some(0) },
            TestTime { name: "nx tests::wall".into(), wall_ms: Some(7), cpu_ms: None },
        ];
        let run = green(tests.clone());
        let message = run.write_with(&tree, &command, &[("issue", "7")]);
        let subject = message.lines().next().unwrap();
        assert_eq!(subject, green(vec![]).write_with(&tree, &command, &[("issue", "7")]));
        assert_eq!(message.lines().nth(1), Some(""));
        let back = GreenRun::recorded(&message, &tree, &command).unwrap();
        assert_eq!(back, run);
        assert_eq!(back.tests(), tests);
        // A subject alone, as every record so far, is the same run, with no tests.
        let alone = GreenRun::recorded(subject, &tree, &command).unwrap();
        assert_eq!((alone.at(), alone.took(), alone.tests()), (run.at(), run.took(), &[][..]));
        assert_eq!(
            green(vec![]).write_with(&tree, &command, &[]),
            green(vec![]).write(&tree, &command)
        );
    }

    #[test]
    fn the_whole_gate_times_its_fast_tests_then_its_slow_ones() {
        let (tree, whole) = (Tree::named("abc"), GateCommand::new("./gate.sh"));
        let (fast, slow) = (GateCommand::new("./gate.sh fast"), GateCommand::new("./gate.sh slow"));
        let timed = |name: &str| TestTime { name: name.into(), wall_ms: Some(1), cpu_ms: None };
        let ran = Ran {
            exit_code: 0,
            timed_out: false,
            clean_before: true,
            clean_after: true,
            seconds: 1,
        };
        let part =
            |command: &GateCommand, test: &str| match after_run(&tree, command, ran, "mini", 5)
                .with_tests(vec![timed(test)])
            {
                Recording::Write(run) => run,
                other => panic!("{other:?}"),
            };
        let (f, s) = (part(&fast, "nx quick"), part(&slow, "nx long"));
        let joined = GreenRun::joined(&tree, &whole, &fast, &f, &slow, &s).unwrap();
        assert_eq!(joined.tests(), [timed("nx quick"), timed("nx long")]);
    }
}

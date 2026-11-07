//! The gate, and green runs of it on record.
//!
//! A record says: this command passed on this tree, and how long the run took
//! on which host. It is one ref on the remote, named for the tree and the
//! command, so it stands wherever the same content is about to land — in
//! another worktree, on another machine.

use flux_rs::attrs::*;

use crate::layout::{GATE_KIND, Layout};
use crate::lease::SKEW;
use crate::record;

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
            ],
        )
    }

    /// A commit subject read at `record_ref(tree, command)` → the run it
    /// records; `None` when it is no record of a run of this command on this
    /// tree.
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
        Some(GreenRun { tree: tree.key, command: command.key, at, took })
    }

    /// The whole gate's run on `tree`, from the runs of its two parts there:
    /// `fast` of `fast_command` and `slow` of `slow_command`, two commands
    /// that together check what `whole` does (docs/adr/0047). It finished
    /// when the older part did, and took both parts' seconds on the slow
    /// part's host. `None` when either run is of another tree or command.
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
        Some(GreenRun { tree: tree.key, command: whole.key, at, took })
    }

    /// The record `write` makes, naming `issue` when the run was of that
    /// issue's own tree. No issue is the record `write` makes: a base-branch
    /// certification names none.
    pub fn write_issued(&self, tree: &Tree, command: &GateCommand, issue: Option<u64>) -> String {
        let line = self.write(tree, command);
        match issue {
            Some(issue) => format!("{line} issue={issue}"),
            None => line,
        }
    }
}

/// The record of a red run of `command` on `tree` for `issue`, finished at
/// `at`. It is not a green run: `GreenRun::recorded` reads one back as none.
/// The ref is not the green ref, which a red run deletes.
pub fn red_subject(
    tree: &Tree,
    command: &GateCommand,
    seconds: u64,
    host: &str,
    at: u64,
    issue: u64,
) -> String {
    let seconds = seconds.to_string();
    let at = at.to_string();
    let issue = issue.to_string();
    record::write(
        GATE,
        &[
            ("tree", tree.name()),
            ("command", command.text()),
            ("at", &at),
            ("seconds", &seconds),
            ("host", host),
            ("issue", &issue),
            ("red", "1"),
        ],
    )
}

/// Where a red run for `issue` is kept until a work account cites it. The
/// green ref stays one per tree and command; a red one is its own, so an
/// earlier red survives a later green.
pub fn red_ref(layout: Layout, issue: u64, at: u64, tree: u64, command: u64) -> String {
    layout.under(&format!("{GATE_KIND}red-{issue}-{at:016x}-{tree:016x}-{command:016x}"))
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

/// What a red run adds to the words that report it when it reached the gate
/// limit of `gate_seconds`: that it did, and what the limit is. Nothing for a
/// run that ended by itself.
pub fn at_limit(ran: Ran, gate_seconds: u64) -> String {
    if ran.timed_out { format!(": its run {}", reached(gate_seconds)) } else { String::new() }
}

/// How a run that reached the gate limit of `gate_seconds` ended.
fn reached(gate_seconds: u64) -> String {
    format!("reached the gate limit of {gate_seconds}s and was ended")
}

/// The comment a red gate run leaves on each PR it was a run of: the command,
/// how it ended under the gate limit of `gate_seconds`, and the end of its log.
pub fn red_comment(
    command: &GateCommand,
    ran: Ran,
    gate_seconds: u64,
    excerpt: &[String],
) -> String {
    let ended =
        if ran.timed_out { reached(gate_seconds) } else { format!("exited {}", ran.exit_code) };
    let log = if excerpt.is_empty() { "(no output)".to_string() } else { excerpt.join("\n") };
    format!(
        "The gate is red on the base branch with this PR on it: `{}` {ended}. Nothing landed; \
         the train's worker is fixing the train.\n\n```\n{log}\n```\n",
        command.text
    )
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
        Recording::Write(GreenRun { tree: tree.key, command: command.key, at: now, took })
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

/// The tests a red gate's output `lines` names as failing, each once, in the
/// order it first names them. A test a later retry passed is not among them.
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
        timed_out && lines.iter().any(|line| cannot_classify(line, test))
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
/// printed after it — or, where no line names one, its last `QUOTE_LINES`.
pub fn quoted(lines: &[String]) -> Vec<String> {
    let first = lines.iter().position(|line| failed_test(line).is_some());
    let from = first.unwrap_or(lines.len().saturating_sub(QUOTE_LINES));
    lines.iter().skip(from).take(QUOTE_LINES).cloned().collect()
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
        let comment = red_comment(&command, ran(2), 600, &["a".into(), "b".into()]);
        assert!(comment.contains("`make test` exited 2") && comment.contains("```\na\nb\n```"));
        let at_the_limit = Ran { timed_out: true, ..ran(124) };
        let comment = red_comment(&command, at_the_limit, 600, &[]);
        assert!(comment.contains("`make test` reached the gate limit of 600s and was ended"));
        assert_eq!(
            at_limit(at_the_limit, 600),
            ": its run reached the gate limit of 600s and was ended"
        );
        assert_eq!(at_limit(ran(1), 600), "", "a run that ended by itself reached none");
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
        assert_eq!(written(1000).write_issued(&tree, &command, None), subject);
        let issued = written(1000).write_issued(&tree, &command, Some(7));
        assert!(issued.ends_with(" host=mini issue=7"), "{issued}");
        assert_eq!(GreenRun::recorded(&issued, &tree, &command).map(|run| run.at()), Some(1000));
        let red = red_subject(&tree, &command, 17, "other", 1900, 7);
        assert!(red.contains(" red=1"), "{red}");
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
}

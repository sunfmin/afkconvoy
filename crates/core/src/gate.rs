//! The gate, and green runs of it on record.
//!
//! A record says: this command passed on this tree. It is one ref on the
//! remote, named for the tree and the command, so it stands wherever the same
//! content is about to land — in another worktree, on another machine.

use flux_rs::attrs::*;

use crate::layout::{GATE_KIND, Layout};
use crate::lease::SKEW;
use crate::record;

/// How long a recorded run is believed: a green that depends on something
/// outside the tree (a toolchain, a service, the date) is trusted for a day.
pub const RECORD_TTL: u64 = 86_400;

const GATE: &str = "convoy:gate";

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
}

impl Ran {
    /// Only exit 0 is green, and a run that timed out is red.
    pub fn green(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }
}

/// Evidence that `command` passed on `tree`, finished at `at`. There are two
/// ways to come by one, and no other: having seen the run's exit code
/// (`after_run`), or having read the record off the remote (`GreenRun::recorded`).
#[derive(Debug, PartialEq, Eq)]
#[refined_by(tree: int, command: int, at: int)]
pub struct GreenRun {
    #[field(u64[tree])]
    tree: u64,
    #[field(u64[command])]
    command: u64,
    #[field(u64[at])]
    at: u64,
}

impl GreenRun {
    #[spec(fn(&GreenRun[@g]) -> u64[g.at])]
    pub fn at(&self) -> u64 {
        self.at
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
        record::write(GATE, &[("tree", &tree.name), ("command", &command.text), ("at", &at)])
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
        let at = fields.number("at")?;
        Some(GreenRun { tree: tree.key, command: command.key, at })
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
        "The gate is red on the landing train with this PR on it: `{}` {ended}. Nothing landed; \
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
/// this command on this tree, finished at `now`.
#[spec(fn(tree: &Tree[@t], command: &GateCommand[@c], ran: Ran, now: u64)
    -> Recording{r: r.written => r.tree == t && r.command == c && r.at == now})]
pub fn after_run(tree: &Tree, command: &GateCommand, ran: Ran, now: u64) -> Recording {
    if !ran.clean_before {
        Recording::Leave
    } else if !ran.green() {
        Recording::Drop
    } else if !ran.clean_after {
        Recording::Leave
    } else {
        Recording::Write(GreenRun { tree: tree.key, command: command.key, at: now })
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
        match after_run(tree, command, ran, now) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        Tree::named("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
    }

    fn command() -> GateCommand {
        GateCommand::new("make test")
    }

    fn ran(exit_code: i32) -> Ran {
        Ran { exit_code, timed_out: false, clean_before: true, clean_after: true }
    }

    fn written(now: u64) -> GreenRun {
        match after_run(&tree(), &command(), ran(0), now) {
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
    fn only_exit_zero_is_green_and_a_timeout_is_red() {
        assert!(ran(0).green());
        assert!(!ran(1).green());
        assert!(!ran(-1).green());
        assert!(!Ran { timed_out: true, ..ran(0) }.green());
    }

    #[test]
    fn a_run_is_recorded_only_when_green_on_a_committed_tree() {
        let (tree, command) = (tree(), command());
        assert!(matches!(after_run(&tree, &command, ran(0), 50), Recording::Write(_)));
        assert_eq!(after_run(&tree, &command, ran(2), 50), Recording::Drop);
        assert_eq!(
            after_run(&tree, &command, Ran { timed_out: true, ..ran(0) }, 50),
            Recording::Drop
        );
        let dirty_before = Ran { clean_before: false, ..ran(0) };
        assert_eq!(after_run(&tree, &command, dirty_before, 50), Recording::Leave);
        let red_and_dirty = Ran { clean_before: false, ..ran(1) };
        assert_eq!(
            after_run(&tree, &command, red_and_dirty, 50),
            Recording::Leave,
            "it tested no commit's tree"
        );
        let rewrote = Ran { clean_after: false, ..ran(0) };
        assert_eq!(after_run(&tree, &command, rewrote, 50), Recording::Leave);
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
}

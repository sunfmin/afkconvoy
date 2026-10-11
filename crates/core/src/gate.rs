//! The gate, and green runs of it on record.
//!
//! A record says: this command passed on this tree. It is one ref on the
//! remote, named for the tree and the command, so it stands wherever the same
//! content is about to land — in another worktree, on another machine.

use flux_rs::attrs::*;

use crate::claims::NAMESPACE;
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
pub fn record_ref(tree: &Tree, command: &GateCommand) -> String {
    format!("{NAMESPACE}/gate/{}-{:016x}", tree.name, command.key)
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
        if self.at <= now { now - self.at <= RECORD_TTL } else { self.at - now <= SKEW }
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

// A run finished at `at` is believed at `now`.
defs! {
    fn fresh(at: int, now: int) -> bool {
        (at <= now && now - at <= RECORD_TTL) || (at > now && at - now <= SKEW)
    }
}

/// What a run of the gate does to its tree's record on the remote.
#[derive(Debug, PartialEq, Eq)]
pub enum Recording {
    /// Green, on a committed tree: put the run on record.
    Write(GreenRun),
    /// Red, on a committed tree: the tree's record goes — the latest run of a
    /// tree is the one believed.
    Drop,
    /// The run was not of a committed tree: it proves nothing about one.
    Leave,
}

/// What a run of `command` in a worktree whose commit holds `tree` does to
/// the record, given what the code that ran it saw.
///
/// A run is of the committed tree only when the worktree was exactly its
/// commit before and after: a command that rewrites a tracked file passes on
/// what it wrote, not on what is committed.
#[spec(fn(tree: &Tree[@t], command: &GateCommand[@c], ran: Ran, now: u64)
    -> Recording)]
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

    /// A record of one tree offered as a run of another.
    #[should_fail]
    #[spec(fn(subject: &str, tested: &Tree, landing: &Tree[@l], command: &GateCommand)
        -> Option<GreenRun{g: g.tree == l}>)]
    fn a_record_of_another_tree(
        subject: &str,
        tested: &Tree,
        landing: &Tree,
        command: &GateCommand,
    ) -> Option<GreenRun> {
        let _ = landing;
        GreenRun::recorded(subject, tested, command)
    }

    /// A record believed without a look at its age.
    #[should_fail]
    #[spec(fn(run: &GreenRun[@g], now: u64) -> bool[super::fresh(g.at, now)])]
    fn a_record_believed_whatever_its_age(run: &GreenRun, now: u64) -> bool {
        let _ = (run, now);
        true
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
    fn the_ref_is_named_for_the_tree_and_the_command() {
        let name = record_ref(&tree(), &command());
        assert!(name.starts_with("refs/convoy/gate/4b825dc642cb6eb9a060e54bf8d69288fbee4904-"));
        assert_eq!(name.len(), "refs/convoy/gate/".len() + 40 + 1 + 16);
        assert_ne!(name, record_ref(&tree(), &GateCommand::new("make check")));
        assert_eq!(key("make test"), key("make test"));
    }
}

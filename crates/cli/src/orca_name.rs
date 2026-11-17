//! What Orca's sidebar calls each worktree afkconvoy adds for a landing, a
//! cut or a watch (sunfmin/control#1226): Orca names one it did not make by
//! its repo and branch, `Detached HEAD @ <sha>` for a candidate's, which
//! says nothing of whose work it is. Every name is built here, from the
//! candidate's record, in the tracker's issue numbers — never a PR's.
//!
//! The name and the gate's state go to Orca as its worktree metadata
//! (`orca worktree set`). Orca lists a worktree it did not make some seconds
//! after git adds it, so a label is set from a thread of its own, again
//! until Orca takes it or `FIND` runs out; nothing of a landing waits on it.
//! Orca not running, or refusing, is said once in the process's log, and the
//! landing goes on.
//!
//! No label links an issue (`--issue`): the fleet tells an issue's worker's
//! worktree by the issue Orca links it to (`orca::Worktree::of_issue`), and a
//! candidate's linked to the same issue would be taken for it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use afkconvoy_core::landing::Of;
use afkconvoy_core::tracker::Tracker;

/// How long a label is set again while Orca does not list its worktree yet.
const FIND: Duration = Duration::from_secs(90);
/// How long between two tries.
const AGAIN: Duration = Duration::from_secs(2);
/// How long a label that ends waits for its last state to reach Orca.
const LAST: Duration = Duration::from_secs(3);

/// What a worktree is, as its Orca entry names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Shown<'a> {
    /// A candidate gated alone: whose PR it is, and the PR's title, which
    /// names a watch's or a cut's change.
    Landing { of: &'a Of, title: &'a str },
    /// A pair of a round's pairing tree: the issues of its two nodes' PRs,
    /// in the order they asked.
    Pair { first: Vec<u64>, second: Vec<u64> },
    /// A round's tip: the issues of the round's PRs, and which of its gates
    /// runs there.
    Round { issues: Vec<u64>, gate: &'a str },
    /// A cut's checkout, of the version it makes.
    Cut { version: &'a str },
    /// A watch's checkout, by what its change fixes.
    Watch { fixes: &'a str },
}

impl Shown<'_> {
    /// Its name in Orca's sidebar, its issues as `tracker` numbers them.
    pub(crate) fn name(&self, tracker: &Tracker) -> String {
        match self {
            Shown::Landing { of, title } => match of {
                Of::Issue(n) => format!("landing {}", issues(tracker, &[*n])),
                Of::Elsewhere(issue) => format!("landing {issue}"),
                Of::Watch(_) => format!("landing watch · {title}"),
                Of::Cut(_) => format!("landing cut · {title}"),
            },
            Shown::Pair { first, second } => {
                format!("pair {} + {}", issues(tracker, first), issues(tracker, second))
            }
            Shown::Round { issues: of, gate } => {
                format!("round of {} · {gate} gate", issues(tracker, of))
            }
            Shown::Cut { version } => format!("cut v{version}"),
            Shown::Watch { fixes } => format!("watch · {fixes}"),
        }
    }
}

/// `numbers` as `tracker` names them, the tracker said once:
/// `control#1, #2` of another tracker, `#1, #2` of the repo's own; `?`
/// where none is known.
fn issues(tracker: &Tracker, numbers: &[u64]) -> String {
    let named = match tracker.named() {
        None => String::new(),
        Some(tracker) => {
            let name = tracker.as_str();
            name.rsplit('/').next().unwrap_or(name).to_string()
        }
    };
    let mut said: Vec<String> = numbers.iter().map(|n| format!("#{n}")).collect();
    match said.first_mut() {
        Some(first) => first.insert_str(0, &named),
        None => said.push(format!("{named}#?")),
    }
    said.join(", ")
}

/// A gate's state, as a label's comment says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Gating,
    Green,
    Red,
}

impl State {
    /// `Green` where the run was, `Red` otherwise.
    pub(crate) fn of(green: bool) -> State {
        if green { State::Green } else { State::Red }
    }

    fn said(self) -> &'static str {
        match self {
            State::Gating => "gating",
            State::Green => "green",
            State::Red => "red",
        }
    }
}

/// What a label is to say: its name, and its comment where it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    name: String,
    comment: Option<State>,
}

/// One worktree's Orca entry, as afkconvoy names it. Dropped, it stops
/// trying; `end` gives Orca its last state first.
pub(crate) struct Label {
    wanted: Option<Sender<Wanted>>,
    ended: Option<Receiver<()>>,
}

impl Label {
    /// Names the worktree at `path` `name` in Orca, with `comment` where it
    /// has one, from a thread of its own: no caller waits on Orca.
    pub(crate) fn show(path: &Path, name: String, comment: Option<State>) -> Label {
        let mut label = Label { wanted: None, ended: None };
        if !crate::orca::running() {
            say_once(path, "orca is not running");
            return label;
        }
        let (wanted, asked) = channel();
        let (done, ended) = channel();
        let at = path.to_path_buf();
        let first = Wanted { name, comment };
        let spawned = std::thread::Builder::new().name("orca name".to_string()).spawn(move || {
            keep(&at, first, &asked);
            let _ = done.send(());
        });
        if let Err(error) = spawned {
            say_once(path, &format!("{error}"));
            return label;
        }
        label.wanted = Some(wanted);
        label.ended = Some(ended);
        label
    }

    /// Names the worktree at `path` `name` in Orca, now and once: one Orca
    /// made, which it lists from the start, or one it has listed already.
    pub(crate) fn once(path: &Path, name: String) {
        if !crate::orca::running() {
            return say_once(path, "orca is not running");
        }
        if let Err(error) = set(path, &Wanted { name, comment: None }) {
            say_once(path, &format!("{error:#}"));
        }
    }

    /// Names it `name` from now on, with `comment`.
    pub(crate) fn set(&self, name: String, comment: Option<State>) {
        if let Some(wanted) = &self.wanted {
            let _ = wanted.send(Wanted { name, comment });
        }
    }

    /// Its gate ended in `state`, under `name`: it is said, waiting at most
    /// `LAST` for Orca, and the label stops.
    pub(crate) fn end(mut self, name: String, state: State) {
        self.set(name, Some(state));
        self.wanted = None;
        if let Some(ended) = self.ended.take() {
            let _ = ended.recv_timeout(LAST);
        }
    }
}

/// Sets what is `wanted` of the worktree at `path` until Orca took the last
/// of it, and the label stopped asking — or, Orca not taking it, until
/// `FIND` ran out, which is said once.
fn keep(path: &Path, first: Wanted, asked: &Receiver<Wanted>) {
    let until = Instant::now() + FIND;
    let mut wanted = first;
    let mut taken: Option<Wanted> = None;
    let mut refused = String::new();
    let mut open = true;
    loop {
        if taken.as_ref() != Some(&wanted) {
            match set(path, &wanted) {
                Ok(()) => taken = Some(wanted.clone()),
                Err(error) => refused = format!("{error:#}"),
            }
        }
        if !open {
            break;
        }
        let next = if taken.as_ref() == Some(&wanted) {
            asked.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else if Instant::now() >= until {
            break;
        } else {
            asked.recv_timeout(AGAIN)
        };
        match next {
            Ok(next) => wanted = next,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => open = false,
        }
        // What is still asked replaces what was: only the last counts.
        while let Ok(next) = asked.try_recv() {
            wanted = next;
        }
    }
    if taken.as_ref() != Some(&wanted) {
        say_once(path, &refused);
    }
}

/// One `orca worktree set` of `wanted` on the worktree at `path`.
fn set(path: &Path, wanted: &Wanted) -> anyhow::Result<()> {
    let selector = format!("path:{}", path.display());
    let mut args = vec!["worktree", "set", "--worktree", &selector, "--display-name", &wanted.name];
    if let Some(state) = wanted.comment {
        args.extend(["--comment", state.said()]);
    }
    args.push("--json");
    crate::orca::orca(&args).map(|_| ())
}

/// Whether this process said already that Orca left a worktree unnamed.
static SAID: AtomicBool = AtomicBool::new(false);

/// Says, once in this process, that Orca did not name the worktree at
/// `path`, and why: the landing goes on all the same.
fn say_once(path: &Path, why: &str) {
    if !SAID.swap(true, Ordering::SeqCst) {
        eprintln!(
            "warning: orca did not name the worktree {} ({why}); the landing goes on without it",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use afkconvoy_core::config::RepoName;

    fn control() -> Tracker {
        let tracker = RepoName::read("sunfmin/control").unwrap();
        Tracker::Other { tracker, repo: RepoName::read("sunfmin/guild").unwrap() }
    }

    #[test]
    fn each_kind_is_named_in_the_trackers_issue_numbers() {
        let of = Of::Issue(1215);
        let named = |shown: Shown| shown.name(&control());
        assert_eq!(named(Shown::Landing { of: &of, title: "x" }), "landing control#1215");
        assert_eq!(
            named(Shown::Pair { first: vec![1201], second: vec![1203] }),
            "pair control#1201 + control#1203"
        );
        assert_eq!(
            named(Shown::Pair { first: vec![1, 2], second: vec![] }),
            "pair control#1, #2 + control#?"
        );
        assert_eq!(
            named(Shown::Round { issues: vec![1, 2, 3], gate: "slow" }),
            "round of control#1, #2, #3 · slow gate"
        );
        assert_eq!(named(Shown::Cut { version: "0.5.99" }), "cut v0.5.99");
        assert_eq!(named(Shown::Watch { fixes: "Fix the tick" }), "watch · Fix the tick");
        let watch = Of::Watch("studio".to_string());
        assert_eq!(
            named(Shown::Landing { of: &watch, title: "Fix the tick" }),
            "landing watch · Fix the tick"
        );
    }

    #[test]
    fn the_repos_own_tracker_names_its_issues_by_number_alone() {
        let shown = Shown::Pair { first: vec![7], second: vec![8, 9] };
        assert_eq!(shown.name(&Tracker::Own), "pair #7 + #8, #9");
    }
}

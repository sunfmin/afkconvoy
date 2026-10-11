//! A worker that stopped without a PR: what its worktree holds, and what the
//! tick does about what it declared.

use flux_rs::attrs::*;

use crate::lease::stamp_age;
use crate::verdict::{Phase, Verdict};
use crate::worker::{GRACE, WorkerState};

/// One worktree's git progress against the base branch as the REMOTE has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Commits on the branch past the base; `None` when the count could not be
    /// read at all — unreadable is not zero.
    pub commits_ahead: Option<u64>,
    /// Anything uncommitted or untracked.
    pub dirty: bool,
    /// When the last commit was made, and when a file was last touched.
    pub last_commit: Option<u64>,
    pub touched: Option<u64>,
}

/// The proof that a worktree's branch holds no work: no commit past the base
/// and nothing uncommitted. `Workless::of` is the only way to one, so what
/// asks for one — closing an issue as needing no change — cannot be done over
/// work, or over a branch nobody could read.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(ahead: int, dirty: bool)]
#[invariant(ahead == 0 && !dirty)]
// The fields are what the invariant is stated on: nothing reads them back.
#[allow(dead_code)]
pub struct Workless {
    #[field(u64[ahead])]
    ahead: u64,
    #[field(bool[dirty])]
    dirty: bool,
}

impl Workless {
    pub fn of(progress: &Progress) -> Option<Workless> {
        match progress.commits_ahead {
            Some(ahead) => {
                if ahead == 0 && !progress.dirty {
                    Some(Workless { ahead, dirty: progress.dirty })
                } else {
                    None
                }
            }
            None => None,
        }
    }
}

/// Why a stopped worker's claim is handed to a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// It declared `needs-human`.
    NeedsHuman,
    /// It declared `already-satisfied` over work sitting on its branch.
    Refuted,
    /// It declared a verdict naming no phase the fleet knows.
    UnknownPhase,
}

impl Why {
    /// The reason as the escalation states it; `said` points at what the
    /// worker wrote.
    pub fn reason(self, said: &str) -> String {
        match self {
            Why::NeedsHuman => format!("The worker declared `needs-human`: {said}"),
            Why::Refuted => format!(
                "The worker declared `already-satisfied`, but its branch holds work (commits \
                 past the base, or uncommitted files): {said}"
            ),
            Why::UnknownPhase => {
                format!("The worker declared a verdict the fleet cannot read: {said}")
            }
        }
    }
}

/// What the tick does with a PR-less claim.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Nothing: the worker is at work, showed a sign of life within grace, or
    /// stopped in a way this decision does not settle.
    Leave,
    /// The issue needed no change: close it.
    Close(Workless),
    Escalate(Why),
}

/// How long ago the most recent sign of life was: terminal activity, the last
/// commit, a file touched. `None` when none is known — which is never "within
/// grace". Commits ahead and a dirty tree are standing facts, true until the
/// branch lands: never signs of life.
pub fn quiet_for(terminal: Option<u64>, progress: &Progress, now: u64) -> Option<u64> {
    let committed = match progress.last_commit {
        Some(at) => stamp_age(at, now),
        None => None,
    };
    let touched = match progress.touched {
        Some(at) => stamp_age(at, now),
        None => None,
    };
    least(least(terminal, committed), touched)
}

fn least(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if a <= b { a } else { b }),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// Routes one PR-less claim from its worker's state, its worktree's progress
/// and the latest verdict on its issue. Nothing is retried: a failure
/// escalates (docs/adr/0005).
pub fn route(
    state: WorkerState,
    progress: &Progress,
    verdict: Option<&Verdict>,
    now: u64,
) -> Route {
    let WorkerState::Idle { quiet } = state else { return Route::Leave };
    if let Some(quiet) = quiet_for(quiet, progress, now)
        && quiet < GRACE
    {
        return Route::Leave;
    }
    let Some(verdict) = verdict else { return Route::Leave };
    match verdict.phase {
        Some(Phase::AlreadySatisfied) => match Workless::of(progress) {
            Some(workless) => Route::Close(workless),
            None => Route::Escalate(Why::Refuted),
        },
        Some(Phase::NeedsHuman) => Route::Escalate(Why::NeedsHuman),
        Some(Phase::Blocked) => Route::Leave,
        None => Route::Escalate(Why::UnknownPhase),
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// An issue closed as needing no change over a branch with commits on it.
    #[should_fail]
    fn a_branch_with_commits_called_workless(ahead: u64) -> Workless {
        Workless { ahead, dirty: false }
    }

    /// …or over uncommitted files.
    #[should_fail]
    fn a_dirty_tree_called_workless(dirty: bool) -> Workless {
        Workless { ahead: 0, dirty }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 100_000;

    fn progress(ahead: u64, dirty: bool) -> Progress {
        Progress { commits_ahead: Some(ahead), dirty, last_commit: Some(1), touched: Some(1) }
    }

    fn said(phase: Option<Phase>) -> Verdict {
        Verdict { phase, blocked_by: vec![], comment: 0 }
    }

    const STOPPED: WorkerState = WorkerState::Idle { quiet: Some(GRACE) };

    #[test]
    fn a_busy_worker_and_a_recent_sign_of_life_are_left() {
        let verdict = said(Some(Phase::NeedsHuman));
        assert_eq!(
            route(WorkerState::Busy, &progress(0, false), Some(&verdict), NOW),
            Route::Leave
        );
        let just = WorkerState::Idle { quiet: Some(GRACE - 1) };
        assert_eq!(route(just, &progress(0, false), Some(&verdict), NOW), Route::Leave);
        let committed = Progress { last_commit: Some(NOW - 10), ..progress(0, false) };
        assert_eq!(route(STOPPED, &committed, Some(&verdict), NOW), Route::Leave);
        let touched = Progress { touched: Some(NOW - 10), ..progress(0, false) };
        assert_eq!(route(STOPPED, &touched, Some(&verdict), NOW), Route::Leave);
    }

    #[test]
    fn commits_ahead_and_a_dirty_tree_are_no_signs_of_life() {
        let verdict = said(Some(Phase::NeedsHuman));
        let route = route(STOPPED, &progress(9, true), Some(&verdict), NOW);
        assert_eq!(route, Route::Escalate(Why::NeedsHuman));
        // nor is a stamp from the future, or no stamp at all
        let unknown = Progress { last_commit: Some(NOW * 2), touched: None, ..progress(0, false) };
        assert_eq!(quiet_for(None, &unknown, NOW), None);
        assert_eq!(quiet_for(Some(400), &progress(0, false), NOW), Some(400));
    }

    #[test]
    fn a_verdict_is_routed_by_who_can_supply_what_it_lacks() {
        let idle = WorkerState::Idle { quiet: None };
        let satisfied = said(Some(Phase::AlreadySatisfied));
        assert!(matches!(route(idle, &progress(0, false), Some(&satisfied), NOW), Route::Close(_)));
        for refuted in [progress(1, false), progress(0, true)] {
            assert_eq!(route(idle, &refuted, Some(&satisfied), NOW), Route::Escalate(Why::Refuted));
        }
        let unreadable = Progress { commits_ahead: None, ..progress(0, false) };
        assert_eq!(
            route(idle, &unreadable, Some(&satisfied), NOW),
            Route::Escalate(Why::Refuted),
            "unreadable is not zero"
        );
        assert_eq!(
            route(idle, &progress(0, false), Some(&said(None)), NOW),
            Route::Escalate(Why::UnknownPhase)
        );
        assert_eq!(route(idle, &progress(0, false), None, NOW), Route::Leave);
        assert!(Why::NeedsHuman.reason("https://x/1").contains("https://x/1"));
    }
}

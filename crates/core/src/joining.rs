//! A worker whose PR is open and not on the landing train yet: the silence
//! ladder with its one restart — nudge → restart → nudge → escalate — and the
//! record of that restart on the PR.

use flux_rs::attrs::*;

use crate::record;
use crate::stopped::{Nudged, Progress, Silence, Silent, Tail, Unnudged, silence};
use crate::worker::WorkerState;

/// The kind of record a PR carries of its join.
pub const JOIN: &str = "convoy:join";

/// The comment that records on a PR that the worker joining it was restarted.
pub fn join_comment(restarted: u64) -> String {
    format!(
        "{}\nafkconvoy: the worker joining this PR to the landing train went silent, and was \
         replaced by a new one in the same worktree. This is done once per PR.",
        record::marker(JOIN, &[("restarted", &restarted.to_string())])
    )
}

/// When the worker joining a PR was restarted, from the PR's comments' bodies:
/// the first record of it, whichever host wrote it.
pub fn restarted_at<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Option<u64> {
    for body in bodies {
        let at = record::read_marker(JOIN, body).and_then(|fields| fields.number("restarted"));
        if at.is_some() {
            return at;
        }
    }
    None
}

/// The proof that a PR's join never had its restart, used up by the one
/// restart it allows. Only `join_route` makes one.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(had: int)]
#[invariant(had == 0)]
// The field is what the invariant is stated on: nothing reads it back.
#[allow(dead_code)]
pub struct Unrestarted {
    #[field(u64[had])]
    had: u64,
}

/// How many restarts a PR's join has had, as its record says.
#[spec(fn(restarted: Option<u64>) -> u64{had: had <= 1})]
fn restarts(restarted: Option<u64>) -> u64 {
    match restarted {
        Some(_) => 1,
        None => 0,
    }
}

/// What the tick does with a claim whose PR is joining.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinRoute {
    /// Nothing: the worker is at work, or showed a sign of life within grace.
    Leave,
    /// Its worker is gone: one is started onto the join at once. That is a
    /// continuation, not the restart.
    Continue,
    Nudge(Unnudged),
    /// Silent past its nudge: replace it, once.
    Restart(Unrestarted),
    /// Silent past the restart and the nudge after it: escalate, with the PR,
    /// the branch and the worktree kept.
    Stalled(Silent),
}

/// Routes one claim whose PR is open and not on the train, from its worker's
/// state, its worktree's progress, the nudge this host recorded, and the
/// restart the PR records. What the worker stopped on — a join conflict it is
/// resolving, a gate it is fixing — changes nothing: the rungs are the same.
pub fn join_route(
    state: WorkerState,
    progress: &Progress,
    nudged: Nudged,
    restarted: Option<u64>,
    now: u64,
) -> JoinRoute {
    match silence(state, progress, nudged, restarted, now) {
        Silence::Gone => JoinRoute::Continue,
        Silence::Lively => JoinRoute::Leave,
        Silence::Nudge(unnudged) => JoinRoute::Nudge(unnudged),
        Silence::Stalled(silent) => {
            let had = restarts(restarted);
            if had == 0 {
                JoinRoute::Restart(Unrestarted { had })
            } else {
                JoinRoute::Stalled(silent)
            }
        }
    }
}

/// One step of restarting a joining worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartStep {
    /// Record the restart on the PR.
    Record,
    /// Close the idle session and start a worker in the same worktree,
    /// briefed only to join the train.
    Replace,
}

/// A restart under way: whether the PR records it.
#[refined_by(recorded: bool)]
struct Restarting {
    #[field(bool[recorded])]
    recorded: bool,
}

impl Restarting {
    #[spec(fn(_unrestarted: Unrestarted) -> Restarting[false])]
    fn begin(_unrestarted: Unrestarted) -> Restarting {
        Restarting { recorded: false }
    }

    #[spec(fn(self: &mut Self[false]) -> RestartStep ensures self: Self[true])]
    fn record(&mut self) -> RestartStep {
        self.recorded = true;
        RestartStep::Record
    }

    /// Only once the PR says so: a worker replaced with nothing recorded
    /// could be replaced again, by this host or another.
    #[spec(fn(&Self[true]) -> RestartStep)]
    fn replace(&self) -> RestartStep {
        RestartStep::Replace
    }
}

/// Restarts a joining worker: record it on the PR → replace the worker. It
/// uses up the proof that the join had no restart, so none has two. Cut after
/// the record, the join has had its restart all the same: whatever worker is
/// there next is nudged, then escalated.
pub fn restart<E, F>(unrestarted: Unrestarted, mut carry_out: F) -> Result<(), E>
where
    F: FnMut(RestartStep) -> Result<(), E>,
{
    let mut restarting = Restarting::begin(unrestarted);
    carry_out(restarting.record())?;
    carry_out(restarting.replace())
}

/// Why a claim whose joining worker stayed silent is handed to a human, with
/// where it stopped.
pub fn stalled_reason(pr: u64, _silent: &Silent, tail: &Tail) -> String {
    let why = format!(
        "The worker opened PR #{pr} and stopped before it was on the landing train. It stayed \
         silent after a nudge, one restart, and a second nudge. The PR, its branch and the \
         worktree are kept."
    );
    if tail.is_empty() {
        return why;
    }
    format!("{why} Its terminal ended with:\n\n```\n{}\n```", tail.text())
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A join that had its restart, restarted again.
    #[should_fail]
    fn a_second_restart(had: u64) -> Unrestarted {
        Unrestarted { had }
    }

    /// A worker replaced before the PR records it.
    #[should_fail]
    fn a_worker_replaced_before_the_restart_is_recorded(unrestarted: Unrestarted) -> RestartStep {
        Restarting::begin(unrestarted).replace()
    }

    /// One restart recorded twice.
    #[should_fail]
    fn a_restart_recorded_twice(unrestarted: Unrestarted) {
        let mut restarting = Restarting::begin(unrestarted);
        restarting.record();
        restarting.record();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::GRACE;

    const NOW: u64 = 100_000;
    const STOPPED: WorkerState = WorkerState::Idle { quiet: Some(GRACE) };

    fn work() -> Progress {
        Progress { commits_ahead: Some(2), dirty: false, last_commit: Some(1), touched: Some(1) }
    }

    fn at(nudged: Nudged, restarted: Option<u64>) -> JoinRoute {
        join_route(STOPPED, &work(), nudged, restarted, NOW)
    }

    #[test]
    fn the_ladder_is_nudge_restart_nudge_escalate_with_a_grace_before_each_rung() {
        let long_ago = NOW - GRACE;
        assert!(matches!(at(Nudged::Never, None), JoinRoute::Nudge(_)));
        assert_eq!(at(Nudged::At(long_ago + 1), None), JoinRoute::Leave);
        assert!(matches!(at(Nudged::At(long_ago), None), JoinRoute::Restart(_)));
        // the worker started in its place was never nudged, and has its own grace
        assert_eq!(at(Nudged::Never, Some(long_ago + 1)), JoinRoute::Leave);
        assert!(matches!(at(Nudged::Never, Some(long_ago)), JoinRoute::Nudge(_)));
        assert_eq!(at(Nudged::At(long_ago + 1), Some(1)), JoinRoute::Leave);
        let JoinRoute::Stalled(silent) = at(Nudged::At(long_ago), Some(1)) else {
            panic!("past its one restart a silence is not restarted again")
        };
        let reason = stalled_reason(70, &silent, &Tail::of(&["Proceed? [y/n]".to_string()]));
        assert!(reason.contains("PR #70") && reason.contains("are kept"), "{reason}");
        assert!(reason.ends_with("```\nProceed? [y/n]\n```"), "{reason}");
    }

    #[test]
    fn a_worker_at_work_is_left_and_one_gone_is_continued_restarted_or_not() {
        for restarted in [None, Some(1)] {
            for nudged in [Nudged::Never, Nudged::At(1)] {
                let busy = join_route(WorkerState::Busy, &work(), nudged, restarted, NOW);
                assert_eq!(busy, JoinRoute::Leave);
                let gone = join_route(WorkerState::Gone, &work(), nudged, restarted, NOW);
                assert_eq!(gone, JoinRoute::Continue);
            }
        }
    }

    #[test]
    fn the_restart_is_read_back_from_the_pr_whoever_wrote_it() {
        let said = join_comment(1234);
        assert!(said.starts_with("<!--convoy:join restarted=1234-->\n"), "{said}");
        assert_eq!(restarted_at(["looks good", said.as_str()]), Some(1234));
        assert_eq!(restarted_at([said.as_str(), join_comment(9999).as_str()]), Some(1234));
        assert_eq!(restarted_at(["<!--convoy:join restarted=soon-->", "convoy:join"]), None);
    }

    #[test]
    fn a_restart_records_then_replaces() {
        let take = || match at(Nudged::At(1), None) {
            JoinRoute::Restart(unrestarted) => unrestarted,
            other => panic!("{other:?}"),
        };
        let mut steps = vec![];
        let done: Result<(), ()> = restart(take(), |step| {
            steps.push(step);
            Ok(())
        });
        assert_eq!((done, steps), (Ok(()), vec![RestartStep::Record, RestartStep::Replace]));
        // a record GitHub refused replaces nobody
        let mut steps = vec![];
        let refused = restart(take(), |step| {
            steps.push(step);
            Err("refused")
        });
        assert_eq!((refused, steps), (Err("refused"), vec![RestartStep::Record]));
    }
}

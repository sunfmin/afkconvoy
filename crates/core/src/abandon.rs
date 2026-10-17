//! Abandon: giving up a landing train whose worker stayed silent, with
//! nothing landed. No PR fails by it: each joins the next train — once.

use flux_rs::attrs::*;

use crate::joining::JoinRecord;
use crate::stopped::{Nudged, Progress, Silence, Silent, Unnudged, silence};
use crate::worker::WorkerState;

/// What the tick does about a train worker that stopped on the tip it was
/// told about, with the train not landed.
#[derive(Debug, PartialEq, Eq)]
pub enum TrainRoute {
    /// Nothing: it showed a sign of life within grace — or it is not there,
    /// which is `train::tend`'s to answer.
    Leave,
    Nudge(Unnudged),
    /// Silent past its nudge: give the train up.
    Abandon(Silent),
}

/// Routes the train worker from its state, its worktree's progress, the nudge
/// this host recorded and when it was last handed its work: started, or told
/// of the train's tip. The train worker holds no claim and declares no
/// verdict: its silence is all there is.
pub fn train_route(
    state: WorkerState,
    progress: &Progress,
    nudged: Nudged,
    handed: Option<u64>,
    now: u64,
) -> TrainRoute {
    match silence(state, progress, nudged, handed, now) {
        Silence::Gone | Silence::Lively => TrainRoute::Leave,
        Silence::Nudge(unnudged) => TrainRoute::Nudge(unnudged),
        Silence::Stalled(silent) => TrainRoute::Abandon(silent),
    }
}

/// Where a PR on a train being abandoned stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It was never taken off an abandoned train: it joins the next one.
    First,
    /// It was, once already: its issue is handed to a human.
    Second,
}

pub fn standing(record: &JoinRecord) -> Standing {
    match record.abandoned {
        None => Standing::First,
        Some(_) => Standing::Second,
    }
}

/// A PR of this fleet's on the train being abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rider {
    pub issue: u64,
    pub pr: u64,
    pub standing: Standing,
}

/// One step of abandoning a train.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbandonStep {
    /// Delete the train's refs, at the tip the abandon read. From here every
    /// PR that was on it is joining again.
    Drop,
    /// Clear the train's worktree — its silent worker closed, what it held
    /// discarded — and keep it.
    Clear,
    /// Record on the PR that it was taken off an abandoned train.
    Record { issue: u64, pr: u64 },
    /// Tell the PR's worker, when one is there, to join the next train.
    Tell { issue: u64, pr: u64 },
    /// Escalate the PR's claim, with the PR, its branch and its worktree kept.
    Escalate { issue: u64, pr: u64 },
}

/// An abandon under way: whether the train's refs are deleted.
#[refined_by(dropped: bool)]
struct Abandoning {
    #[field(bool[dropped])]
    dropped: bool,
}

impl Abandoning {
    #[spec(fn(_silent: Silent) -> Abandoning[false])]
    fn begin(_silent: Silent) -> Abandoning {
        Abandoning { dropped: false }
    }

    #[spec(fn(self: &mut Self[false]) -> AbandonStep ensures self: Self[true])]
    fn drop_train(&mut self) -> AbandonStep {
        self.dropped = true;
        AbandonStep::Drop
    }

    /// Only once the train is gone: a worktree cleared under a train that
    /// stands would get a new worker started on it by the next tick.
    #[spec(fn(&Self[true]) -> AbandonStep)]
    fn clear(&self) -> AbandonStep {
        AbandonStep::Clear
    }

    /// Only once the train is gone: recorded sooner, a PR would be marked as
    /// taken off a train that still stands — and escalated by the abandon
    /// that follows a failed one.
    #[spec(fn(&Self[true], rider: &Rider) -> AbandonStep)]
    fn record(&self, rider: &Rider) -> AbandonStep {
        AbandonStep::Record { issue: rider.issue, pr: rider.pr }
    }

    /// Only once the train is gone: told sooner, the worker would join the
    /// very train being given up.
    #[spec(fn(&Self[true], rider: &Rider) -> AbandonStep)]
    fn tell(&self, rider: &Rider) -> AbandonStep {
        AbandonStep::Tell { issue: rider.issue, pr: rider.pr }
    }

    #[spec(fn(&Self[true], rider: &Rider) -> AbandonStep)]
    fn escalate(&self, rider: &Rider) -> AbandonStep {
        AbandonStep::Escalate { issue: rider.issue, pr: rider.pr }
    }
}

/// What an abandon came to, by issue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Abandoned {
    /// Their PRs join the next train.
    pub again: Vec<u64>,
    /// Taken off an abandoned train a second time: handed to a human.
    pub escalated: Vec<u64>,
}

/// Abandons a train: delete its refs → clear its worktree → for each rider,
/// record it and tell its worker, or escalate it. It uses up the proof that
/// the train worker is silent, so no train is given up under a worker at
/// work. Cut anywhere, nothing is lost: the refs' deletion is the abandon,
/// and a rider it did not get to is a PR that joins again unrecorded.
pub fn abandon<E, F>(silent: Silent, riders: &[Rider], mut carry_out: F) -> Result<Abandoned, E>
where
    F: FnMut(AbandonStep) -> Result<(), E>,
{
    let mut abandoning = Abandoning::begin(silent);
    carry_out(abandoning.drop_train())?;
    carry_out(abandoning.clear())?;
    let mut done = Abandoned::default();
    for rider in riders {
        match rider.standing {
            Standing::First => {
                carry_out(abandoning.record(rider))?;
                carry_out(abandoning.tell(rider))?;
                done.again.push(rider.issue);
            }
            Standing::Second => {
                carry_out(abandoning.escalate(rider))?;
                done.escalated.push(rider.issue);
            }
        }
    }
    Ok(done)
}

/// Why the claim of a PR taken off an abandoned train a second time is handed
/// to a human.
pub fn abandoned_reason(pr: u64) -> String {
    format!(
        "PR #{pr} was on a landing train that was given up — its worker stayed silent — for the \
         second time. The PR and its branch are kept."
    )
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A PR recorded as taken off a train that still stands.
    #[should_fail]
    fn a_rider_recorded_before_the_train_is_dropped(silent: Silent, rider: &Rider) -> AbandonStep {
        Abandoning::begin(silent).record(rider)
    }

    /// A worker told to join again while the train being given up is there.
    #[should_fail]
    fn a_worker_told_before_the_train_is_dropped(silent: Silent, rider: &Rider) -> AbandonStep {
        Abandoning::begin(silent).tell(rider)
    }

    /// The train's worktree cleared under a train that stands.
    #[should_fail]
    fn a_worktree_cleared_before_the_train_is_dropped(silent: Silent) -> AbandonStep {
        Abandoning::begin(silent).clear()
    }

    /// One train dropped twice.
    #[should_fail]
    fn a_train_dropped_twice(silent: Silent) {
        let mut abandoning = Abandoning::begin(silent);
        abandoning.drop_train();
        abandoning.drop_train();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::GRACE;

    const NOW: u64 = 100_000;
    const STOPPED: WorkerState = WorkerState::Idle { quiet: Some(GRACE) };

    fn held() -> Progress {
        Progress { commits_ahead: Some(2), dirty: true, last_commit: Some(1), touched: Some(1) }
    }

    fn silent() -> Silent {
        match train_route(STOPPED, &held(), Nudged::At(1), Some(1), NOW) {
            TrainRoute::Abandon(silent) => silent,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_stopped_train_worker_is_left_within_grace_nudged_once_then_the_train_abandoned() {
        let long_ago = NOW - GRACE;
        let route = |nudged, told| train_route(STOPPED, &held(), nudged, told, NOW);
        // told of the tip moments ago, or committed a fix: it shows life
        assert_eq!(route(Nudged::Never, Some(long_ago + 1)), TrainRoute::Leave);
        let fixing = Progress { last_commit: Some(NOW - 10), ..held() };
        let left = train_route(STOPPED, &fixing, Nudged::Never, Some(1), NOW);
        assert_eq!(left, TrainRoute::Leave);
        assert!(matches!(route(Nudged::Never, Some(long_ago)), TrainRoute::Nudge(_)));
        assert_eq!(route(Nudged::At(long_ago + 1), Some(1)), TrainRoute::Leave);
        assert!(matches!(route(Nudged::At(long_ago), Some(1)), TrainRoute::Abandon(_)));
        // one at work, or gone, is never this decision's
        for state in [WorkerState::Busy, WorkerState::Gone] {
            let route = train_route(state, &held(), Nudged::At(1), Some(1), NOW);
            assert_eq!(route, TrainRoute::Leave);
        }
    }

    #[test]
    fn an_abandon_drops_the_train_first_and_fails_no_pr() {
        let once = JoinRecord { abandoned: Some(5), ..JoinRecord::default() };
        let restarted = JoinRecord { restarted: Some(5), ..JoinRecord::default() };
        assert_eq!(standing(&JoinRecord::default()), Standing::First);
        assert_eq!(standing(&restarted), Standing::First, "a restart is not an abandon");
        assert_eq!(standing(&once), Standing::Second);
        let riders = [
            Rider { issue: 7, pr: 70, standing: Standing::First },
            Rider { issue: 8, pr: 80, standing: Standing::Second },
        ];
        let mut steps = vec![];
        let done: Result<Abandoned, ()> = abandon(silent(), &riders, |step| {
            steps.push(step);
            Ok(())
        });
        assert_eq!(done, Ok(Abandoned { again: vec![7], escalated: vec![8] }));
        assert_eq!(
            steps,
            [
                AbandonStep::Drop,
                AbandonStep::Clear,
                AbandonStep::Record { issue: 7, pr: 70 },
                AbandonStep::Tell { issue: 7, pr: 70 },
                AbandonStep::Escalate { issue: 8, pr: 80 },
            ]
        );
        assert!(abandoned_reason(80).contains("PR #80") && abandoned_reason(80).contains("kept"));
    }

    #[test]
    fn a_train_that_moved_under_the_abandon_is_left_with_nothing_recorded() {
        let riders = [Rider { issue: 7, pr: 70, standing: Standing::First }];
        let mut steps = vec![];
        let refused = abandon(silent(), &riders, |step| {
            steps.push(step);
            Err("the train moved")
        });
        assert_eq!((refused, steps), (Err("the train moved"), vec![AbandonStep::Drop]));
    }
}

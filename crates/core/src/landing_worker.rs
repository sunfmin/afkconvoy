//! A worker whose PR is open and not on the base branch yet: the silence
//! ladder with its one restart — nudge → restart → nudge → escalate — and the
//! record of that restart on the PR.

use flux_rs::attrs::*;

use crate::record;
use crate::stopped::{Background, Nudged, Progress, Silence, Silent, Tail, Unnudged, silence};
use crate::worker::WorkerState;

/// The kind of record a PR carries of its landing.
pub const JOIN: &str = "convoy:landing";

/// What a PR's one marker comment records of its landing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LandingRecord {
    /// When the worker landing it was restarted.
    pub restarted: Option<u64>,
    /// Each switch of its worker onto another harness, oldest first.
    pub switched: Vec<Switch>,
    /// Which of the PR's comments carries the record, when one does: the one
    /// a new fact is written into.
    pub comment: Option<usize>,
}

/// One switch of a PR's worker onto another harness (`afkconvoy switch`): no
/// restart, and nothing the silence ladder counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switch {
    /// When, in epoch seconds.
    pub at: u64,
    /// The harness the worker ended ran on, and the one its successor runs on,
    /// each by its key.
    pub from: String,
    pub to: String,
    /// The fleet instance that switched it.
    pub by: String,
}

impl Switch {
    /// As the record's field writes it: `at/from/to/by`.
    fn written(&self) -> String {
        format!("{}/{}/{}/{}", self.at, self.from, self.to, self.by)
    }

    /// One the record's field wrote; `None` for anything else.
    fn read(written: &str) -> Option<Switch> {
        let mut parts = written.splitn(4, '/');
        let at = record::fleet_number(parts.next()?)?;
        let (from, to, by) = (parts.next()?, parts.next()?, parts.next()?);
        if [from, to, by].iter().any(|part| part.is_empty()) {
            return None;
        }
        Some(Switch { at, from: from.to_string(), to: to.to_string(), by: by.to_string() })
    }
}

/// The comment that carries a PR's join record.
pub fn join_comment(record: &LandingRecord) -> String {
    let stamp = |at: Option<u64>| at.map(|at| at.to_string()).unwrap_or_default();
    let restarted = stamp(record.restarted);
    let switched: Vec<String> = record.switched.iter().map(Switch::written).collect();
    let switched = switched.join(",");
    let mut body = record::marker(JOIN, &[("restarted", &restarted), ("switched", &switched)]);
    if record.restarted.is_some() {
        body.push_str(
            "\nafkconvoy: the worker landing this PR went silent, and was \
             replaced by a new one in the same worktree. This is done once per PR.",
        );
    }
    for switch in &record.switched {
        body.push_str(&format!(
            "\nafkconvoy: the worker of this PR was switched from `{}` to `{}` by `{}` at {}: \
             a new worker in the same worktree, on the same branch, at the same stage.",
            switch.from, switch.to, switch.by, switch.at
        ));
    }
    body
}

/// A PR's join record, from its comments' bodies, oldest first. The first
/// comment with the marker is the record's; a fact a later one states — two
/// hosts wrote at once — counts all the same.
pub fn read_join<'a>(bodies: impl IntoIterator<Item = &'a str>) -> LandingRecord {
    let mut record = LandingRecord::default();
    for (at, body) in bodies.into_iter().enumerate() {
        let Some(fields) =
            record::read_marker(JOIN, body).or_else(|| record::read_marker("convoy:join", body))
        else {
            continue;
        };
        record.comment = record.comment.or(Some(at));
        record.restarted = record.restarted.or(fields.number("restarted"));
        let switched = fields.get("switched").unwrap_or("").split(',').filter_map(Switch::read);
        for switch in switched {
            if !record.switched.contains(&switch) {
                record.switched.push(switch);
            }
        }
    }
    record
}

/// What a landing worker's worktree last heard from its turn (`land`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answered {
    /// It never asked for one: the worker is still preparing the branch.
    Never,
    /// A turn is being asked for right now: queued, waiting on a
    /// predecessor, or gating.
    Asking,
    /// The last turn answered this `outcome`.
    Said(String),
}

impl Answered {
    /// Whether the last turn answered a wait (`Ending::waits`) or failed
    /// before giving the worker anything to change; either is asked again.
    pub fn waits(&self) -> bool {
        let Answered::Said(outcome) = self else { return false };
        outcome == "error"
            || crate::ending::Ending::ALL.iter().any(|e| e.word() == outcome && e.waits())
    }
}

/// Whether the worker of a landing claim is changing the tree, and its claim
/// takes a slot: its harness has a turn open, and no turn of the PR's is
/// being asked for or last answered a wait. Resolving a conflict, fixing a
/// red gate, or still preparing the branch is changing it.
pub fn changing_tree(state: Option<WorkerState>, answered: &Answered) -> bool {
    matches!(state, Some(WorkerState::Busy))
        && !matches!(answered, Answered::Asking)
        && !answered.waits()
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

/// How many restarts a PR's landing has had, as its record says.
#[spec(fn(restarted: Option<u64>) -> u64{had: had <= 1})]
fn restarts(restarted: Option<u64>) -> u64 {
    match restarted {
        Some(_) => 1,
        None => 0,
    }
}

/// What the tick does with a claim whose PR is landing.
#[derive(Debug, PartialEq, Eq)]
pub enum LandingRoute {
    /// Nothing: the worker is at work, or showed a sign of life within grace.
    Leave,
    /// Nothing: it stopped, and waits on its own background work, running
    /// since this epoch second.
    Waiting(u64),
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

/// Routes one claim whose PR is open and not landed, from its worker's
/// state, its worktree's progress, the nudge this host recorded, the restart
/// the PR records, and when the worker was last handed its work — started,
/// or restarted. What the worker stopped on — a merge conflict it is
/// resolving, a gate it is fixing — changes nothing: the rungs are the same.
pub fn join_route(
    state: WorkerState,
    progress: &Progress,
    nudged: Nudged,
    restarted: Option<u64>,
    (handed, background): (Option<u64>, Option<Background>),
    now: u64,
) -> LandingRoute {
    match silence(state, progress, nudged, handed, background, now) {
        Silence::Gone => LandingRoute::Continue,
        Silence::Lively => LandingRoute::Leave,
        Silence::Waiting(since) => LandingRoute::Waiting(since),
        Silence::Nudge(unnudged) => LandingRoute::Nudge(unnudged),
        Silence::Stalled(silent) => {
            let had = restarts(restarted);
            if had == 0 {
                LandingRoute::Restart(Unrestarted { had })
            } else {
                LandingRoute::Stalled(silent)
            }
        }
    }
}

/// One step of restarting a landing worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartStep {
    /// Record the restart on the PR.
    Record,
    /// Close the idle session and start a worker in the same worktree,
    /// briefed only to land the PR.
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

/// Restarts a landing worker: record it on the PR → replace the worker. It
/// uses up the proof that the join had no restart, so none has two. Cut after
/// the record, the landing has had its restart all the same: whatever worker is
/// there next is nudged, then escalated.
pub fn restart<E, F>(unrestarted: Unrestarted, mut carry_out: F) -> Result<(), E>
where
    F: FnMut(RestartStep) -> Result<(), E>,
{
    let mut restarting = Restarting::begin(unrestarted);
    carry_out(restarting.record())?;
    carry_out(restarting.replace())
}

/// Why a claim whose landing worker stayed silent is handed to a human, with
/// where it stopped.
pub fn stalled_reason(pr: u64, _silent: &Silent, tail: &Tail) -> String {
    let why = format!(
        "The worker opened PR #{pr} and stopped before it was on the base branch. It stayed \
         silent after a nudge, one restart, and a second nudge. The PR and its branch are \
         kept."
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
        Progress {
            commits_ahead: Some(2),
            trees_equal: Some(false),
            dirty: false,
            last_commit: Some(1),
            touched: Some(1),
        }
    }

    fn at(nudged: Nudged, restarted: Option<u64>) -> LandingRoute {
        join_route(STOPPED, &work(), nudged, restarted, (restarted, None), NOW)
    }

    #[test]
    fn only_a_busy_worker_with_its_tree_to_change_is_at_work() {
        let busy = Some(WorkerState::Busy);
        for answered in ["conflict", "speculative_conflict", "gate_red"] {
            assert!(changing_tree(busy, &Answered::Said(answered.into())), "{answered}");
        }
        assert!(changing_tree(busy, &Answered::Never), "preparing the branch");
        assert!(!changing_tree(busy, &Answered::Asking), "the turn is queued");
        for wait in crate::ending::Ending::ALL.iter().filter(|e| e.waits()) {
            let said = Answered::Said(wait.word().into());
            assert!(!changing_tree(busy, &said), "{}", wait.word());
        }
        let failed = Answered::Said("error".into());
        assert!(failed.waits(), "a failed ask is retried");
        assert!(!changing_tree(busy, &failed), "a failed ask leaves the tree unchanged");
        let conflict = Answered::Said("conflict".into());
        assert!(!changing_tree(None, &conflict), "no worktree");
        assert!(!changing_tree(Some(WorkerState::Gone), &conflict), "gone");
        assert!(!changing_tree(Some(STOPPED), &conflict), "idle");
    }

    #[test]
    fn the_ladder_is_nudge_restart_nudge_escalate_with_a_grace_before_each_rung() {
        let long_ago = NOW - GRACE;
        assert!(matches!(at(Nudged::Never, None), LandingRoute::Nudge(_)));
        assert_eq!(at(Nudged::At(long_ago + 1), None), LandingRoute::Leave);
        assert!(matches!(at(Nudged::At(long_ago), None), LandingRoute::Restart(_)));
        // the worker started in its place was never nudged, and has its own grace
        assert_eq!(at(Nudged::Never, Some(long_ago + 1)), LandingRoute::Leave);
        assert!(matches!(at(Nudged::Never, Some(long_ago)), LandingRoute::Nudge(_)));
        assert_eq!(at(Nudged::At(long_ago + 1), Some(1)), LandingRoute::Leave);
        let LandingRoute::Stalled(silent) = at(Nudged::At(long_ago), Some(1)) else {
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
                let busy = join_route(
                    WorkerState::Busy,
                    &work(),
                    nudged,
                    restarted,
                    (restarted, None),
                    NOW,
                );
                assert_eq!(busy, LandingRoute::Leave);
                let gone = join_route(
                    WorkerState::Gone,
                    &work(),
                    nudged,
                    restarted,
                    (restarted, None),
                    NOW,
                );
                assert_eq!(gone, LandingRoute::Continue);
            }
        }
    }

    #[test]
    fn a_switch_is_recorded_beside_the_restart_and_counts_as_none() {
        let switch = |at, from: &str, to: &str| Switch {
            at,
            from: from.to_string(),
            to: to.to_string(),
            by: "a-h1-1".to_string(),
        };
        let record = LandingRecord {
            switched: vec![switch(10, "claude-code", "codex"), switch(20, "codex", "pi")],
            ..LandingRecord::default()
        };
        let said = join_comment(&record);
        assert!(
            said.starts_with(
                "<!--convoy:landing switched=10/claude-code/codex/a-h1-1,20/codex/pi/a-h1-1-->\n"
            ),
            "{said}"
        );
        assert!(said.contains("switched from `claude-code` to `codex` by `a-h1-1`"), "{said}");
        let read = read_join([said.as_str()]);
        assert_eq!(read, LandingRecord { comment: Some(0), ..record });
        assert_eq!(read.restarted, None, "a switch is no restart");
        // what the fleet did not write is no switch
        assert_eq!(read_join(["<!--convoy:landing switched=soon/a/b/c,1/a//c-->"]).switched, []);
    }

    #[test]
    fn a_restart_records_then_replaces() {
        let take = || match at(Nudged::At(1), None) {
            LandingRoute::Restart(unrestarted) => unrestarted,
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

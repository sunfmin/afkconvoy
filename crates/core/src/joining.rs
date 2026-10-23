//! A worker whose PR is open and not on the landing train yet: the silence
//! ladder with its one restart — nudge → restart → nudge → escalate — and the
//! record of that restart on the PR.

use flux_rs::attrs::*;

use crate::record;
use crate::stopped::{Background, Nudged, Progress, Silence, Silent, Tail, Unnudged, silence};
use crate::worker::WorkerState;

/// The kind of record a PR carries of its join.
pub const JOIN: &str = "convoy:join";

/// What a PR's one marker comment records of its join.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JoinRecord {
    /// When the worker joining it was restarted.
    pub restarted: Option<u64>,
    /// When it was taken off an abandoned train.
    pub abandoned: Option<u64>,
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
pub fn join_comment(record: &JoinRecord) -> String {
    let stamp = |at: Option<u64>| at.map(|at| at.to_string()).unwrap_or_default();
    let (restarted, abandoned) = (stamp(record.restarted), stamp(record.abandoned));
    let switched: Vec<String> = record.switched.iter().map(Switch::written).collect();
    let switched = switched.join(",");
    let mut body = record::marker(
        JOIN,
        &[("restarted", &restarted), ("abandoned", &abandoned), ("switched", &switched)],
    );
    if record.restarted.is_some() {
        body.push_str(
            "\nafkconvoy: the worker joining this PR to the landing train went silent, and was \
             replaced by a new one in the same worktree. This is done once per PR.",
        );
    }
    if record.abandoned.is_some() {
        body.push_str(
            "\nafkconvoy: the landing train this PR was on was given up with nothing landed — \
             its worker stayed silent — and the PR joins the next one. A second time, the issue \
             is handed to a human instead.",
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
pub fn read_join<'a>(bodies: impl IntoIterator<Item = &'a str>) -> JoinRecord {
    let mut record = JoinRecord::default();
    for (at, body) in bodies.into_iter().enumerate() {
        let Some(fields) = record::read_marker(JOIN, body) else { continue };
        record.comment = record.comment.or(Some(at));
        record.restarted = record.restarted.or(fields.number("restarted"));
        record.abandoned = record.abandoned.or(fields.number("abandoned"));
        let switched = fields.get("switched").unwrap_or("").split(',').filter_map(Switch::read);
        for switch in switched {
            if !record.switched.contains(&switch) {
                record.switched.push(switch);
            }
        }
    }
    record
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

/// Routes one claim whose PR is open and not on the train, from its worker's
/// state, its worktree's progress, the nudge this host recorded, the restart
/// the PR records, and when the worker was last handed its work — started,
/// or restarted. What the worker stopped on — a join conflict it is
/// resolving, a gate it is fixing — changes nothing: the rungs are the same.
pub fn join_route(
    state: WorkerState,
    progress: &Progress,
    nudged: Nudged,
    restarted: Option<u64>,
    (handed, background): (Option<u64>, Option<Background>),
    now: u64,
) -> JoinRoute {
    match silence(state, progress, nudged, handed, background, now) {
        Silence::Gone => JoinRoute::Continue,
        Silence::Lively => JoinRoute::Leave,
        Silence::Waiting(since) => JoinRoute::Waiting(since),
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

    fn at(nudged: Nudged, restarted: Option<u64>) -> JoinRoute {
        join_route(STOPPED, &work(), nudged, restarted, (restarted, None), NOW)
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
                let busy = join_route(
                    WorkerState::Busy,
                    &work(),
                    nudged,
                    restarted,
                    (restarted, None),
                    NOW,
                );
                assert_eq!(busy, JoinRoute::Leave);
                let gone = join_route(
                    WorkerState::Gone,
                    &work(),
                    nudged,
                    restarted,
                    (restarted, None),
                    NOW,
                );
                assert_eq!(gone, JoinRoute::Continue);
            }
        }
    }

    #[test]
    fn a_join_record_is_one_comment_read_back_whoever_wrote_it() {
        let restarted = JoinRecord { restarted: Some(1234), ..JoinRecord::default() };
        let said = join_comment(&restarted);
        assert!(said.starts_with("<!--convoy:join restarted=1234-->\n"), "{said}");
        assert_eq!(
            read_join(["looks good", said.as_str()]),
            JoinRecord { comment: Some(1), ..restarted.clone() }
        );
        let both = JoinRecord { abandoned: Some(2000), ..restarted };
        let said = join_comment(&both);
        assert!(said.starts_with("<!--convoy:join restarted=1234 abandoned=2000-->\n"), "{said}");
        assert!(said.contains("replaced by a new one") && said.contains("joins the next one"));
        assert_eq!(read_join([said.as_str()]), JoinRecord { comment: Some(0), ..both });
        // two hosts wrote at once: the first comment is the record's, every fact counts
        let other = join_comment(&JoinRecord { abandoned: Some(9), ..JoinRecord::default() });
        let other = other.as_str();
        let read = read_join([join_comment(&JoinRecord::default()).as_str(), other]);
        let read_back =
            JoinRecord { abandoned: Some(9), comment: Some(0), ..JoinRecord::default() };
        assert_eq!(read, read_back);
        assert_eq!(read_join(["<!--convoy:join restarted=soon-->"]).restarted, None);
        assert_eq!(read_join(["convoy:join restarted=1"]), JoinRecord::default());
    }

    #[test]
    fn a_switch_is_recorded_beside_the_restart_and_counts_as_none() {
        let switch = |at, from: &str, to: &str| Switch {
            at,
            from: from.to_string(),
            to: to.to_string(),
            by: "a-h1-1".to_string(),
        };
        let record = JoinRecord {
            switched: vec![switch(10, "claude-code", "codex"), switch(20, "codex", "pi")],
            ..JoinRecord::default()
        };
        let said = join_comment(&record);
        assert!(
            said.starts_with(
                "<!--convoy:join switched=10/claude-code/codex/a-h1-1,20/codex/pi/a-h1-1-->\n"
            ),
            "{said}"
        );
        assert!(said.contains("switched from `claude-code` to `codex` by `a-h1-1`"), "{said}");
        let read = read_join([said.as_str()]);
        assert_eq!(read, JoinRecord { comment: Some(0), ..record });
        assert_eq!(read.restarted, None, "a switch is no restart");
        // what the fleet did not write is no switch
        assert_eq!(read_join(["<!--convoy:join switched=soon/a/b/c,1/a//c-->"]).switched, []);
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

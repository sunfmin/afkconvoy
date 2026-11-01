//! Where an issue is in its workflow, and how much rework it has had: both
//! derived from what was declared on it, in order, and stored nowhere
//! (docs/adr/0010, docs/adr/0011). A `Place` takes a loaded workflow as given
//! and checks nothing of it.

use flux_rs::attrs::*;

use crate::team::{Consult, Pass, Performer, Stage, Work, Workflow};
use crate::verdict::Phase;

/// Why a landing worker hands its issue to a human.
pub const LANDING_ESCALATION: &str =
    "the landing worker declared `escalate`, which it hands to a human";

/// What ends a stage or answers an escalation: a stage or consult verdict, or
/// a ruling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declared {
    Pass,
    Reject,
    Blocked,
    AlreadySatisfied,
    Escalate,
    /// The consult ends the issue with its reason.
    Close,
    /// The consult waits for evidence or a human to send it back.
    Park,
    /// What a consult or a human decides on an escalated issue.
    Ruling,
}

impl Declared {
    pub const ALL: [Declared; 8] = [
        Declared::Pass,
        Declared::Reject,
        Declared::Blocked,
        Declared::AlreadySatisfied,
        Declared::Escalate,
        Declared::Close,
        Declared::Park,
        Declared::Ruling,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Declared::Pass => "pass",
            Declared::Reject => "reject",
            Declared::Blocked => Phase::Blocked.word(),
            Declared::AlreadySatisfied => Phase::AlreadySatisfied.word(),
            Declared::Escalate => Phase::Escalate.word(),
            Declared::Close => "close",
            Declared::Park => "park",
            Declared::Ruling => "ruling",
        }
    }

    pub fn read(word: &str) -> Option<Declared> {
        Declared::ALL.into_iter().find(|declared| declared.word() == word)
    }
}

/// One of the two limits on rework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breaker {
    /// On an issue's rework in total.
    Rework,
    /// On one stage's rejections in a row.
    ConsecutiveRework,
}

impl Breaker {
    /// The key of a workflow that sets it.
    pub fn key(self) -> &'static str {
        match self {
            Breaker::Rework => "max_rework",
            Breaker::ConsecutiveRework => "max_consecutive_rework",
        }
    }
}

/// A failure the fleet saw of a stage's worker, which no stage record of
/// the worker's shows: the fleet leaves the record of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// It stopped with no verdict, and its nudge changed nothing.
    Silent,
    /// Marked as writing nothing, it left a commit on the branch.
    LeftCommit,
    /// It declared `already-satisfied` over a branch that holds work.
    Refuted,
    /// It declared `blocked`, and the wait would never end: it named no
    /// blocker, or one that will not close, or one whose waiting closes a
    /// cycle.
    Unblockable,
    /// It was handed to a human before any consult saw it: what put it
    /// there is the comment of its escalation.
    Escalated,
}

impl Failure {
    pub const ALL: [Failure; 5] = [
        Failure::Silent,
        Failure::LeftCommit,
        Failure::Refuted,
        Failure::Unblockable,
        Failure::Escalated,
    ];

    /// The word a stage record states it by.
    pub fn word(self) -> &'static str {
        match self {
            Failure::Silent => "failed-silent",
            Failure::LeftCommit => "failed-left-commit",
            Failure::Refuted => "failed-refuted",
            Failure::Unblockable => "failed-unblockable",
            Failure::Escalated => "failed-escalated",
        }
    }

    pub fn read(word: &str) -> Option<Failure> {
        Failure::ALL.into_iter().find(|failure| failure.word() == word)
    }

    /// The failure in the few words a tick's line has room for.
    pub fn brief(self) -> &'static str {
        match self {
            Failure::Silent => "its worker went silent",
            Failure::LeftCommit => "left a commit it may not",
            Failure::Refuted => "already-satisfied over a branch that holds work",
            Failure::Unblockable => "blocked on what will never clear",
            Failure::Escalated => "handed to a human before its consult saw it",
        }
    }
}

/// What a stage's consult is asked to rule on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsultOn {
    /// The `escalate` the stage declared.
    Escalate,
    /// A hand-off that would have gone to a human, for this reason.
    HandOff(Reason),
}

/// Why an issue is handed to a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Its stage declared `escalate`, and hands that to a human: it says
    /// `escalate: { to: human }`.
    Verdict,
    /// Its stage's consult let the escalation, or the hand-off, through.
    Consult,
    /// Its worker failed, as the fleet saw and recorded.
    Failed(Failure),
    /// What was declared is no verdict its stage may declare.
    NoValidVerdict,
    /// One more rework would have passed this breaker.
    Breaker(Breaker),
    /// A stage record names a stage the workflow no longer has: the workflow
    /// changed under the issue, and where that leaves it is not guessed.
    WorkflowChanged,
}

impl Reason {
    /// The reason in the few words a tick's line has room for.
    pub fn brief(self) -> String {
        match self {
            Reason::Verdict => "declared escalate, to a human".to_string(),
            Reason::Consult => "consult let it through".to_string(),
            Reason::Failed(failure) => failure.brief().to_string(),
            Reason::NoValidVerdict => "no valid verdict".to_string(),
            Reason::Breaker(breaker) => format!("breaker {} tripped", breaker.key()),
            Reason::WorkflowChanged => "its workflow changed under it".to_string(),
        }
    }

    /// The reason as a human is told it, of an issue escalated at `stage`.
    pub fn said(self, stage: &Stage) -> String {
        let name = &stage.name;
        match self {
            Reason::Verdict => {
                format!("the stage `{name}` declared `escalate`, which it hands to a human")
            }
            Reason::Consult => format!("the consult of the stage `{name}` let it through"),
            Reason::Failed(failure) => format!("the stage `{name}` failed: {}", failure.brief()),
            Reason::NoValidVerdict => {
                format!("no valid verdict was declared at the stage `{name}`")
            }
            Reason::Breaker(breaker) => format!(
                "the breaker `{}` tripped: `{name}` would have run again once more than it allows",
                breaker.key()
            ),
            Reason::WorkflowChanged => format!(
                "the workflow changed under this issue: one of its stage records names a stage \
                 the workflow no longer has. Where that leaves the issue is not guessed; it was \
                 at the stage `{name}`"
            ),
        }
    }

    /// Whether `other` is the same kind of hand-off: a consult that ruled on
    /// one is not asked of the next of its kind. Both breakers are one kind.
    pub fn same(self, other: Reason) -> bool {
        match (self, other) {
            (Reason::Breaker(_), Reason::Breaker(_)) => true,
            (one, other) => one == other,
        }
    }
}

/// Where an issue is in its workflow: `at_stage` while it is at a stage, one
/// of a workflow of `n` stages, of a team of `roles` roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[refined_by(at_stage: bool, n: int, roles: int)]
pub enum Position<'w> {
    /// A worker is to perform the stage, as this skill of this role.
    #[variant((&Stage[@s], &Work[s.roles]) -> Position[true, s.n, s.roles])]
    Perform(&'w Stage, &'w Work),
    /// The stage is a human's: nothing happens until they declare its verdict.
    #[variant((&Stage[@s]) -> Position[true, s.n, s.roles])]
    Waiting(&'w Stage),
    /// A worker is to rule on what the stage handed on, as this consult —
    /// a skill of a named role: its `escalate`, or a
    /// hand-off that would have gone to a human.
    #[variant((&Stage[@s], &Consult{v: v.by_role => v.roles == s.roles}, ConsultOn) -> Position[true, s.n, s.roles])]
    Consult(&'w Stage, &'w Consult, ConsultOn),
    /// Parked on what the stage declared it blocked by. The stage runs again
    /// when those close, which counts as no rework.
    #[variant((&Stage[@s]) -> Position[true, s.n, s.roles])]
    Parked(&'w Stage),
    /// A consult parked it for evidence; only an explicit send brings it back.
    #[variant((&Stage[@s]) -> Position[true, s.n, s.roles])]
    ParkedForEvidence(&'w Stage),
    /// Its last stage passed to the landing train.
    #[variant(Position[false, 0, 0])]
    Landing,
    /// Landing waits on the issues its worker named; closing them resumes
    /// landing, without performing a stage again.
    #[variant(Position[false, 0, 0])]
    LandingParked,
    /// Landing was handed to a human. Their ruling resumes landing.
    #[variant(Position[false, 0, 0])]
    LandingEscalating,
    /// Nothing more to do.
    #[variant(Position[false, 0, 0])]
    Ended,
    /// Handed to a human. Under their ruling the stage runs again.
    #[variant((&Stage[@s], Reason) -> Position[true, s.n, s.roles])]
    Escalating(&'w Stage, Reason),
}

impl<'w> Position<'w> {
    /// Whether the issue has reached landing, including a wait declared there.
    pub fn landing(self) -> bool {
        matches!(self, Position::Landing | Position::LandingParked | Position::LandingEscalating)
    }

    /// The stage the issue is at; `None` once it has left its stages.
    #[spec(fn(Position[@p]) -> Option<&Stage{v: p.at_stage && v.n == p.n && v.roles == p.roles}>)]
    pub fn stage(self) -> Option<&'w Stage> {
        match self {
            Position::Perform(stage, _)
            | Position::Waiting(stage)
            | Position::Consult(stage, ..)
            | Position::Parked(stage)
            | Position::ParkedForEvidence(stage)
            | Position::Escalating(stage, _) => Some(stage),
            Position::Landing
            | Position::LandingParked
            | Position::LandingEscalating
            | Position::Ended => None,
        }
    }

    /// The stage a worker is to be started on, and as whom: the stage to
    /// perform, or the one a park ended at — which runs again once nothing
    /// blocks the issue. `None` where no stage's worker is to be started.
    #[spec(fn(Position[@p]) -> Option<(&Stage, &Work{v: p.at_stage && v.roles == p.roles})>)]
    pub fn start(self) -> Option<(&'w Stage, &'w Work)> {
        match self {
            Position::Perform(stage, work) => Some((stage, work)),
            Position::Parked(stage) => match &stage.by {
                Performer::Role(work) => Some((stage, work)),
                Performer::Human => None,
            },
            _ => None,
        }
    }

    /// The breaker that handed the issue on, to a human or to its stage's
    /// consult; `None` where none did.
    pub fn breaker(self) -> Option<Breaker> {
        match self {
            Position::Escalating(_, Reason::Breaker(breaker))
            | Position::Consult(_, _, ConsultOn::HandOff(Reason::Breaker(breaker))) => {
                Some(breaker)
            }
            _ => None,
        }
    }

    /// Who a worker is to be started as, of a role; `None` where none is to
    /// be.
    pub fn worker(self) -> Option<&'w Work> {
        match self {
            Position::Perform(_, work) | Position::Consult(_, Consult::Role(work), _) => Some(work),
            _ => None,
        }
    }
}

/// How often something happened, under the breaker that limits it. It never
/// holds more than the breaker allows: what would pass it is not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[refined_by(times: int, limit: int)]
#[invariant(times <= limit)]
pub(crate) struct Count {
    #[field(u64[times])]
    times: u64,
    #[field(u64[limit])]
    limit: u64,
}

impl Count {
    #[spec(fn(limit: u64) -> Count[0, limit])]
    fn none(limit: u64) -> Count {
        Count { times: 0, limit }
    }

    /// The count after one more; `None` when that would pass the breaker.
    #[spec(fn(&Count[@c]) -> Option<Count[c.times + 1, c.limit]>)]
    fn more(&self) -> Option<Count> {
        if self.times < self.limit {
            Some(Count { times: self.times + 1, limit: self.limit })
        } else {
            None
        }
    }

    #[spec(fn(&Count[@c]) -> u64[c.times])]
    pub(crate) fn times(&self) -> u64 {
        self.times
    }

    /// How many more the breaker allows.
    #[spec(fn(&Count[@c]) -> u64[c.limit - c.times])]
    fn left(&self) -> u64 {
        self.limit - self.times
    }
}

/// An issue's position in a workflow of `n` stages, of a team of `roles`
/// roles, with its two rework counts. The stage it is at, while at one, is a
/// stage of that workflow, and it holds one count of rejections in a row for
/// each of the `n`.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(n: int, roles: int)]
pub struct Place<'w> {
    #[field(&Workflow[n, roles])]
    workflow: &'w Workflow,
    #[field(Position{v: v.at_stage => v.n == n && v.roles == roles})]
    pub position: Position<'w>,
    total: Count,
    /// Each stage's rejections in a row, at the stage's place.
    #[field(Vec<Count>[n])]
    consecutive: Vec<Count>,
    /// The hand-offs a consult ruled on since a human last put the issue at
    /// a stage: the next of each kind goes to a human.
    ruled: Vec<Reason>,
}

impl Clone for Place<'_> {
    #[spec(fn(&Place[@p]) -> Place[p])]
    fn clone(&self) -> Self {
        Place {
            workflow: self.workflow,
            position: self.position,
            total: self.total,
            consecutive: self.consecutive.clone(),
            ruled: self.ruled.clone(),
        }
    }
}

/// The position of an issue that comes to `stage`.
#[spec(fn(&Stage[@s]) -> Position[true, s.n, s.roles])]
fn at(stage: &Stage) -> Position<'_> {
    match &stage.by {
        Performer::Human => Position::Waiting(stage),
        Performer::Role(work) => Position::Perform(stage, work),
    }
}

impl<'w> Place<'w> {
    /// Where an issue is that has just entered `workflow`: at its first
    /// stage, with no rework.
    #[spec(fn(&Workflow[@w]) -> Place[w.n, w.roles])]
    pub fn enter(workflow: &'w Workflow) -> Place<'w> {
        let mut consecutive: Vec<Count> = vec![];
        while consecutive.len() < workflow.stages().len() {
            consecutive.push(Count::none(workflow.max_consecutive_rework));
        }
        Place {
            workflow,
            position: at(workflow.first()),
            total: Count::none(workflow.max_rework),
            consecutive,
            ruled: vec![],
        }
    }

    /// Where the issue is once a human puts it at `stage` — by ruling on it
    /// there, or by sending it there: the stage is to be performed, and both
    /// counts start over.
    #[spec(fn(Place[@p], &Stage{v: v.n == p.n && v.roles == p.roles}) -> Place[p])]
    pub fn put(self, stage: &'w Stage) -> Place<'w> {
        Place { position: at(stage), ..Place::enter(self.workflow) }
    }

    /// Where the issue is once the `escalate` of its stage is set aside, a
    /// fleet lock having explained it (`lock::explains`): at that stage again,
    /// to be performed, with both counts as they were — it is no rework. An
    /// issue that is neither with a consult nor with a human for that verdict
    /// stays where it is.
    #[spec(fn(Place[@p]) -> Place[p])]
    pub fn again(self) -> Place<'w> {
        let position = match self.position {
            Position::Consult(stage, _, ConsultOn::Escalate)
            | Position::Escalating(stage, Reason::Verdict) => at(stage),
            Position::Consult(..)
            | Position::Perform(..)
            | Position::Waiting(_)
            | Position::Parked(_)
            | Position::ParkedForEvidence(_)
            | Position::Escalating(..)
            | Position::Landing
            | Position::LandingParked
            | Position::LandingEscalating
            | Position::Ended => self.position,
        };
        Place { position, ..self }
    }

    /// The issue's rework in total, since it entered or a human last put it
    /// at a stage. The rework a breaker refused is not in it.
    pub fn rework(&self) -> u64 {
        self.total.times()
    }

    /// The rework the issue has left before its breaker stops it.
    pub fn rework_left(&self) -> u64 {
        self.total.left()
    }

    /// The rejections `stage` has declared in a row, with no `pass` of its
    /// own between. `stage` is one of the issue's workflow.
    #[spec(fn(&Place[@p], &Stage{v: v.n == p.n}) -> u64)]
    pub fn consecutive_rework(&self, stage: &Stage) -> u64 {
        self.consecutive[stage.at()].times()
    }

    /// The rejections in a row `stage`, one of the issue's workflow, has left
    /// before its breaker stops it.
    #[spec(fn(&Place[@p], &Stage{v: v.n == p.n}) -> u64)]
    pub fn consecutive_rework_left(&self, stage: &Stage) -> u64 {
        self.consecutive[stage.at()].left()
    }

    /// Where an issue at `stage` goes that would be handed to a human for
    /// `reason`: to the consult the stage names, unless it ruled on a
    /// hand-off of that kind since a human last put the issue at a stage —
    /// then, and where the stage names none, to a human.
    #[spec(fn(&Place[@p], &Stage[@s], Reason) -> Position[true, s.n, s.roles])]
    fn handed(&self, stage: &'w Stage, reason: Reason) -> Position<'w> {
        let ruled = self.ruled.iter().any(|ruled| ruled.same(reason));
        match &stage.consult {
            Some(consult) if !ruled => {
                Position::Consult(stage, consult, ConsultOn::HandOff(reason))
            }
            _ => Position::Escalating(stage, reason),
        }
    }

    /// Whether a hand-off for `reason` from where the issue is goes to a
    /// consult: it is at a stage that names one, and that consult is not
    /// what failed.
    pub fn consults(&self, reason: Reason) -> bool {
        match self.position {
            Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage) => {
                matches!(self.handed(stage, reason), Position::Consult(..))
            }
            _ => false,
        }
    }

    /// Where the issue is once the fleet saw its stage's worker fail for
    /// `failure`: with the stage's consult (`handed`), or with a human when
    /// the consult itself is what failed. An issue already escalated, landing
    /// or ended stays where it is.
    #[spec(fn(Place[@p], Failure) -> Place[p])]
    pub fn failed(self, failure: Failure) -> Place<'w> {
        let reason = Reason::Failed(failure);
        let position = match self.position {
            Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage) => {
                self.handed(stage, reason)
            }
            Position::Consult(stage, ..) => Position::Escalating(stage, reason),
            Position::ParkedForEvidence(_)
            | Position::Escalating(..)
            | Position::Landing
            | Position::LandingParked
            | Position::LandingEscalating
            | Position::Ended => self.position,
        };
        Place { position, ..self }
    }

    /// Where the issue is once the workflow changed under it, a record naming
    /// a stage it no longer has: handed on from the stage it was at. One
    /// already escalated, landing or ended stays where it is.
    #[spec(fn(Place[@p]) -> Place[p])]
    pub fn changed(self) -> Place<'w> {
        let reason = Reason::WorkflowChanged;
        let position = match self.position {
            Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage) => {
                self.handed(stage, reason)
            }
            Position::Consult(stage, ..) => Position::Escalating(stage, reason),
            Position::ParkedForEvidence(_)
            | Position::Escalating(..)
            | Position::Landing
            | Position::LandingParked
            | Position::LandingEscalating
            | Position::Ended => self.position,
        };
        Place { position, ..self }
    }

    /// Where the issue is once its consult sends it to `stage`: there, to be
    /// performed, as a ruling would leave it — the rework counts are not
    /// started over: only a human's word does that. An issue that is not
    /// with a consult stays where it is.
    #[spec(fn(Place[@p], &Stage{v: v.n == p.n && v.roles == p.roles}) -> Place[p])]
    pub fn sent_by_consult(self, stage: &'w Stage) -> Place<'w> {
        let Position::Consult(_, _, on) = self.position else { return self };
        let mut next = self.clone();
        match on {
            ConsultOn::HandOff(reason) => {
                next.ruled.push(reason);
                next.position = at(stage);
            }
            ConsultOn::Escalate => match self.total.more() {
                Some(total) => {
                    next.total = total;
                    next.position = at(stage);
                }
                None => {
                    next.position = Position::Escalating(stage, Reason::Breaker(Breaker::Rework))
                }
            },
        }
        next
    }

    /// Where the fleet's record of `failure` of the last verdict declared at
    /// `stage` leaves an issue that verdict moved on: back at that stage, the
    /// counts as they were, and handed on from there.
    #[spec(fn(Place[@p], &Stage{v: v.n == p.n && v.roles == p.roles}, Failure) -> Place[p])]
    pub fn refuted(mut self, stage: &'w Stage, failure: Failure) -> Place<'w> {
        self.position = at(stage);
        self.failed(failure)
    }

    /// Whether `declared` is something that may be declared where the issue is.
    pub fn takes(&self, declared: Declared) -> bool {
        self.valid(declared).is_some()
    }

    /// Where the issue is once `declared` is declared on it. What may not be
    /// declared where it is escalates it from a stage, and changes nothing
    /// once it is escalated, landing or ended.
    #[spec(fn(Place[@p], Declared) -> Place[p])]
    pub fn after(self, declared: Declared) -> Place<'w> {
        if let Some(next) = self.valid(declared) {
            return next;
        }
        let reason = Reason::NoValidVerdict;
        let position = match self.position {
            Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage) => {
                self.handed(stage, reason)
            }
            // A consult's own failure is a human's.
            Position::Consult(stage, ..) => Position::Escalating(stage, reason),
            Position::ParkedForEvidence(_)
            | Position::Escalating(..)
            | Position::Landing
            | Position::LandingParked
            | Position::LandingEscalating
            | Position::Ended => self.position,
        };
        Place { position, ..self }
    }

    /// The place after `declared`; `None` when it may not be declared here.
    #[spec(fn(&Place[@p], Declared) -> Option<Place[p]>)]
    fn valid(&self, declared: Declared) -> Option<Place<'w>> {
        let mut next = self.clone();
        next.position = match (self.position, declared) {
            (Position::Landing | Position::LandingParked, Declared::Blocked) => {
                Position::LandingParked
            }
            (Position::Landing | Position::LandingParked, Declared::Escalate) => {
                Position::LandingEscalating
            }
            (Position::LandingEscalating, Declared::Ruling) => Position::Landing,
            (
                Position::ParkedForEvidence(_)
                | Position::Landing
                | Position::LandingParked
                | Position::LandingEscalating
                | Position::Ended,
                _,
            ) => {
                return None;
            }
            // A ruling on an escalated issue is a human's.
            (Position::Escalating(stage, _), Declared::Ruling) => {
                return Some(next.put(stage));
            }
            (Position::Escalating(..), _) => return None,
            // A consult's ruling on an `escalate` is rework: the stage that
            // asked runs again.
            (Position::Consult(stage, _, ConsultOn::Escalate), Declared::Ruling) => {
                match self.total.more() {
                    Some(total) => {
                        next.total = total;
                        at(stage)
                    }
                    None => Position::Escalating(stage, Reason::Breaker(Breaker::Rework)),
                }
            }
            // One on a hand-off is not, and starts no count over: the stage
            // runs again, and the next hand-off of that kind is a human's.
            (Position::Consult(stage, _, ConsultOn::HandOff(reason)), Declared::Ruling) => {
                next.ruled.push(reason);
                at(stage)
            }
            (Position::Consult(stage, ..), Declared::Escalate) => {
                Position::Escalating(stage, Reason::Consult)
            }
            (Position::Consult(..), Declared::Close) => Position::Ended,
            (Position::Consult(stage, ..), Declared::Park) => Position::ParkedForEvidence(stage),
            (Position::Consult(..), _) => return None,
            (
                Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage),
                declared,
            ) => match declared {
                Declared::Pass => {
                    next.consecutive[stage.at()] =
                        Count::none(self.workflow.max_consecutive_rework);
                    match stage.pass() {
                        Pass::Stage(to) => at(self.workflow.stage(to)),
                        Pass::Land => Position::Landing,
                        Pass::Done => Position::Ended,
                    }
                }
                Declared::Reject => {
                    let back = self.workflow.stage(stage.reject()?);
                    let in_a_row = self.consecutive[stage.at()].more();
                    match (in_a_row, self.total.more()) {
                        (None, _) => self.handed(back, Reason::Breaker(Breaker::ConsecutiveRework)),
                        (_, None) => self.handed(back, Reason::Breaker(Breaker::Rework)),
                        (Some(in_a_row), Some(total)) => {
                            next.total = total;
                            next.consecutive[stage.at()] = in_a_row;
                            at(back)
                        }
                    }
                }
                Declared::Blocked => Position::Parked(stage),
                Declared::AlreadySatisfied => Position::Ended,
                Declared::Escalate => match &stage.consult {
                    Some(consult) => Position::Consult(stage, consult, ConsultOn::Escalate),
                    None => Position::Escalating(stage, Reason::Verdict),
                },
                Declared::Close | Declared::Park | Declared::Ruling => return None,
            },
        };
        Some(next)
    }
}

/// Where an issue of `workflow` is, given everything declared on it since it
/// entered, oldest first.
#[spec(fn(&Workflow[@w], &[Declared]) -> Place[w.n, w.roles])]
pub fn position<'w>(workflow: &'w Workflow, declared: &[Declared]) -> Place<'w> {
    let mut place = Place::enter(workflow);
    for declared in declared {
        place = place.after(*declared);
    }
    place
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A count read for a stage the issue's workflow lacks.
    #[should_fail]
    fn a_count_read_for_a_stage_the_workflow_lacks(place: &Place, stage: &Stage) -> u64 {
        place.consecutive_rework(stage)
    }
}

#[cfg(test)]
mod tests {
    use super::Declared::*;
    use super::*;
    use crate::team::tests::{FEATURE, one};
    use crate::team::{Team, WORKFLOW_KEYS};

    /// The line of `FEATURE` that names the consult of `build`.
    const CONSULT_OF_BUILD: &str = "    escalate: { to: reviewer, do: rule }\n";

    /// Where an issue of the team's `feature` workflow is after `declared`: its
    /// position in a few words, its rework in total, and `review`'s and
    /// `sign-off`'s rejections in a row.
    fn after(team: &Team, declared: &[Declared]) -> (String, u64, [u64; 2]) {
        let workflow = team.workflow("feature").unwrap();
        let place = position(workflow, declared);
        let said = match place.position {
            Position::Perform(stage, work) => {
                format!("{} by {}: {}", stage.name, team.role_of(work).name, work.skill)
            }
            Position::Waiting(stage) => format!("{} by a human", stage.name),
            Position::Consult(stage, Consult::Role(work), _) => {
                let role = &team.role_of(work).name;
                format!("consult on {} by {role}: {}", stage.name, work.skill)
            }
            Position::ParkedForEvidence(stage) => format!("parked for evidence at {}", stage.name),
            Position::Parked(stage) => format!("parked at {}", stage.name),
            Position::Landing => "landing".to_string(),
            Position::LandingParked => "parked at land".to_string(),
            Position::LandingEscalating => "escalating at land".to_string(),
            Position::Ended => "ended".to_string(),
            Position::Escalating(stage, reason) => {
                format!("escalating at {}: {reason:?}", stage.name)
            }
        };
        let [.., review, sign_off] = workflow.stages() else { panic!("two stages at least") };
        let in_a_row = [place.consecutive_rework(review), place.consecutive_rework(sign_off)];
        (said, place.rework(), in_a_row)
    }

    fn at(team: &Team, declared: &[Declared]) -> String {
        after(team, declared).0
    }

    #[test]
    fn pass_and_reject_follow_the_workflow() {
        let team = one(FEATURE);
        assert_eq!(at(&team, &[]), "plan by dev: plan");
        assert_eq!(at(&team, &[Pass]), "build by dev: implement");
        assert_eq!(at(&team, &[Pass, Pass]), "review by reviewer: review");
        assert_eq!(at(&team, &[Pass, Pass, Reject]), "build by dev: implement");
        assert_eq!(at(&team, &[Pass, Pass, Pass]), "sign-off by a human");
        assert_eq!(at(&team, &[Pass, Pass, Pass, Reject]), "plan by dev: plan");
        assert_eq!(at(&team, &[Pass, Pass, Pass, Pass]), "landing");
        let triage = one("stages:\n  - { name: a, by: dev, do: plan, on: { pass: done } }\n");
        let ended = position(triage.workflow("feature").unwrap(), &[Pass]);
        assert_eq!(ended.position, Position::Ended);
    }

    #[test]
    fn the_other_three_verdicts_mean_the_same_at_every_stage() {
        let team = one(FEATURE);
        assert_eq!(at(&team, &[Blocked]), "parked at plan");
        // The stage runs again once its blockers close, and that is no rework.
        assert_eq!(after(&team, &[Blocked, Pass]), ("build by dev: implement".into(), 0, [0, 0]));
        assert_eq!(at(&team, &[Pass, Blocked, Blocked]), "parked at build");
        assert_eq!(at(&team, &[AlreadySatisfied]), "ended");
        assert_eq!(at(&team, &[Pass, Pass, AlreadySatisfied]), "ended");
        // `build` names a consult; `plan` and a human's stage name none.
        assert_eq!(at(&team, &[Pass, Escalate]), "consult on build by reviewer: rule");
        assert_eq!(at(&team, &[Escalate]), "escalating at plan: Verdict");
        assert_eq!(at(&team, &[Pass, Pass, Pass, Escalate]), "escalating at sign-off: Verdict");
    }

    #[test]
    fn what_a_stage_may_not_declare_is_no_valid_verdict() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        // `plan` has no `reject`.
        assert!(!Place::enter(workflow).takes(Reject));
        assert_eq!(at(&team, &[Reject]), "escalating at plan: NoValidVerdict");
        assert_eq!(
            at(&team, &[Pass, Pass, Blocked, Ruling]),
            "escalating at review: NoValidVerdict"
        );
        // A consult rules or lets the escalation through, and nothing else.
        assert_eq!(at(&team, &[Pass, Escalate, Pass]), "escalating at build: NoValidVerdict");
        // An ended issue, or one waiting for a ruling, stays where it is.
        for left in [&[AlreadySatisfied][..], &[Escalate]] {
            let place = position(workflow, left);
            for declared in Declared::ALL.into_iter().filter(|declared| *declared != Ruling) {
                assert!(!place.takes(declared), "{left:?} then {declared:?}");
                assert_eq!(place.clone().after(declared), place);
            }
        }
        assert!(!position(workflow, &[AlreadySatisfied]).takes(Ruling));
    }

    #[test]
    fn landing_parks_or_escalates_without_returning_to_a_stage() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let landing = position(workflow, &[Pass, Pass, Pass, Pass]);
        for declared in Declared::ALL {
            assert_eq!(landing.takes(declared), matches!(declared, Blocked | Escalate));
            if !matches!(declared, Blocked | Escalate) {
                assert_eq!(landing.clone().after(declared), landing);
            }
        }
        let parked = landing.clone().after(Blocked);
        assert_eq!(parked.position, Position::LandingParked);
        assert!(parked.position.stage().is_none());
        let escalated = parked.after(Escalate);
        assert_eq!(escalated.position, Position::LandingEscalating);
        assert_eq!(escalated.after(Ruling), landing);
    }

    #[test]
    fn consult_outcomes_preserve_total_and_consecutive_rework_even_at_a_breaker() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        for declared in [
            vec![Pass, Escalate],
            vec![Pass, Pass, Reject, Escalate],
            vec![Pass, Pass, Reject, Pass, Reject, Pass, Reject],
        ] {
            let place = position(workflow, &declared);
            assert!(matches!(place.position, Position::Consult(..)), "{declared:?}");
            for verdict in [Close, Park] {
                let next = place.clone().after(verdict);
                assert_eq!(next.rework(), place.rework());
                assert_eq!(next.rework_left(), place.rework_left());
                for stage in workflow.stages() {
                    assert_eq!(next.consecutive_rework(stage), place.consecutive_rework(stage));
                }
                assert!(next.position.start().is_none());
                assert!(next.position.breaker().is_none());
                assert!(matches!(next.position, Position::Ended | Position::ParkedForEvidence(_)));
            }
        }
    }

    #[test]
    fn rework_is_counted_in_total_and_per_stage_in_a_row() {
        let team = one(FEATURE);
        let once = [Pass, Pass, Reject];
        assert_eq!(after(&team, &once), ("build by dev: implement".into(), 1, [1, 0]));
        let twice = [&once[..], &[Pass, Reject]].concat();
        assert_eq!(after(&team, &twice).1, 2);
        assert_eq!(after(&team, &twice).2, [2, 0]);
        // A pass of a stage starts that stage's count over, and no other's.
        let passed = [&twice[..], &[Pass, Pass]].concat();
        assert_eq!(after(&team, &passed), ("sign-off by a human".into(), 2, [0, 0]));
        let back = [&passed[..], &[Reject, Pass, Pass, Pass]].concat();
        assert_eq!(after(&team, &back), ("sign-off by a human".into(), 3, [0, 1]));
    }

    #[test]
    fn a_breaker_hands_the_rework_that_would_pass_it_on() {
        let team = one(FEATURE);
        // `review` rejects a third time in a row: max_consecutive_rework is 2.
        // `build`, which the rejection leads back to, names a consult.
        let twice = [Pass, Pass, Reject, Pass, Reject, Pass];
        let third = [&twice[..], &[Reject]].concat();
        let consult = "consult on build by reviewer: rule".to_string();
        assert_eq!(after(&team, &third), (consult.clone(), 2, [2, 0]));
        let place = position(team.workflow("feature").unwrap(), &third);
        assert_eq!(place.position.breaker(), Some(Breaker::ConsecutiveRework));
        // Its ruling runs `build` again and starts no count over…
        let ruled = [&third[..], &[Ruling]].concat();
        assert_eq!(after(&team, &ruled), ("build by dev: implement".into(), 2, [2, 0]));
        // …and the breaker that trips again goes to a human: either one.
        let again = [&ruled[..], &[Pass, Reject]].concat();
        let tripped = "escalating at build: Breaker(ConsecutiveRework)".to_string();
        assert_eq!(after(&team, &again), (tripped, 2, [2, 0]));
        // A fourth rework in total, with no stage three times in a row.
        let three = [&twice[..], &[Pass, Reject, Pass, Pass]].concat();
        assert_eq!(after(&team, &three), ("review by reviewer: review".into(), 3, [0, 1]));
        let fourth = [&three[..], &[Reject]].concat();
        assert_eq!(after(&team, &fourth), (consult, 3, [0, 1]));
        // A breaker set to 0 allows no rework at all; a stage that goes to a
        // human or names no consult escalates for the watch.
        let strict = one(&format!("max_rework: 0\n{FEATURE}"));
        assert_eq!(at(&strict, &[Pass, Pass, Reject]), "consult on build by reviewer: rule");
        let human = FEATURE.replace(CONSULT_OF_BUILD, "    escalate: { to: human }\n");
        let alone = one(&format!("max_rework: 0\n{human}"));
        assert_eq!(at(&alone, &[Pass, Pass, Reject]), "escalating at build: Breaker(Rework)");
        let stock = one(&format!("max_rework: 0\n{}", FEATURE.replace(CONSULT_OF_BUILD, "")));
        assert_eq!(at(&stock, &[Pass, Pass, Reject]), "escalating at build: Breaker(Rework)");
    }

    #[test]
    fn a_human_ruling_starts_both_counts_over_and_a_consults_is_rework() {
        let team = one(FEATURE);
        // A breaker its consult let through.
        let tripped = [Pass, Pass, Reject, Pass, Reject, Pass, Reject, Escalate];
        assert_eq!(at(&team, &tripped), "escalating at build: Consult");
        // The stage the rejection led back to runs again, under the ruling.
        let ruled = [&tripped[..], &[Ruling]].concat();
        assert_eq!(after(&team, &ruled), ("build by dev: implement".into(), 0, [0, 0]));
        let on = [&ruled[..], &[Pass, Reject]].concat();
        assert_eq!(after(&team, &on), ("build by dev: implement".into(), 1, [1, 0]));
        let waiting = [Pass, Pass, Pass, Escalate, Ruling];
        assert_eq!(at(&team, &waiting), "sign-off by a human");

        // The stage that asked a consult runs again under its ruling.
        let asked = [Pass, Pass, Reject, Escalate];
        assert_eq!(after(&team, &asked), ("consult on build by reviewer: rule".into(), 1, [1, 0]));
        let ruled = [&asked[..], &[Ruling]].concat();
        assert_eq!(after(&team, &ruled), ("build by dev: implement".into(), 2, [1, 0]));
        // Or the consult lets it through, which is no rework.
        let through = [&asked[..], &[Escalate]].concat();
        assert_eq!(after(&team, &through), ("escalating at build: Consult".into(), 1, [1, 0]));
        // A consult's ruling is held to the breaker like any rework.
        let most = [&ruled[..], &[Escalate, Ruling, Escalate, Ruling]].concat();
        assert_eq!(at(&team, &most), "escalating at build: Breaker(Rework)");
    }

    #[test]
    fn every_hand_off_goes_to_the_consult_first_and_the_same_one_again_to_a_human() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let built = position(workflow, &[Pass]);
        let said = |place: &Place| match place.position {
            Position::Consult(stage, _, on) => format!("consult at {}: {on:?}", stage.name),
            Position::Escalating(stage, reason) => format!("human at {}: {reason:?}", stage.name),
            other => format!("{:?}", other.stage().map(|stage| &stage.name)),
        };
        // A failure the fleet saw, and a verdict `build` may not declare.
        let silent = built.clone().failed(Failure::Silent);
        assert_eq!(said(&silent), "consult at build: HandOff(Failed(Silent))");
        assert!(built.consults(Reason::Failed(Failure::Silent)));
        assert_eq!(said(&built.clone().after(Reject)), "consult at build: HandOff(NoValidVerdict)");
        // Its ruling runs the stage again, with no rework…
        let ruled = silent.after(Ruling);
        assert_eq!((said(&ruled), ruled.rework()), ("Some(\"build\")".to_string(), 0));
        // …and the same failure again is a human's; another kind is the consult's.
        assert!(!ruled.consults(Reason::Failed(Failure::Silent)));
        assert_eq!(said(&ruled.clone().failed(Failure::Silent)), "human at build: Failed(Silent)");
        let commit = ruled.clone().failed(Failure::LeftCommit);
        assert_eq!(said(&commit), "consult at build: HandOff(Failed(LeftCommit))");
        // The consult's own failure is a human's, whatever it was asked.
        let asked = built.clone().after(Escalate);
        assert!(!asked.consults(Reason::Failed(Failure::Silent)));
        assert_eq!(said(&asked.clone().failed(Failure::Silent)), "human at build: Failed(Silent)");
        assert_eq!(said(&asked.after(Pass)), "human at build: NoValidVerdict");
        // A human putting the issue at a stage starts what the consult ruled on over.
        let put = ruled.put(&workflow.stages()[1]);
        assert!(put.consults(Reason::Failed(Failure::Silent)));
        // A stage that goes to a human hands every failure to one.
        let planning = Place::enter(workflow);
        assert!(!planning.consults(Reason::Failed(Failure::Silent)));
        assert_eq!(said(&planning.failed(Failure::Refuted)), "human at plan: Failed(Refuted)");
        for failure in Failure::ALL {
            assert_eq!(Failure::read(failure.word()), Some(failure));
        }
    }

    #[test]
    fn a_consult_sends_the_issue_to_any_stage_with_the_counts_as_they_were() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let review = &workflow.stages()[2];
        let silent = position(workflow, &[Pass, Pass, Reject]).failed(Failure::Silent);
        let sent = silent.sent_by_consult(&workflow.stages()[0]);
        assert_eq!(sent.position.start().map(|(stage, _)| stage.name.as_str()), Some("plan"));
        assert_eq!((sent.rework(), sent.consecutive_rework(review)), (1, 1));
        // Sending answers an `escalate` as a ruling does: it is rework.
        let asked = position(workflow, &[Pass, Escalate]).sent_by_consult(review);
        assert_eq!(
            (asked.position.stage().map(|s| s.name.as_str()), asked.rework()),
            (Some("review"), 1)
        );
        // An issue that is not with a consult is not sent by one.
        let built = position(workflow, &[Pass]);
        assert_eq!(built.clone().sent_by_consult(review), built);
    }

    #[test]
    fn an_escalate_set_aside_puts_the_issue_at_its_stage_again_with_no_rework() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let again = |declared: &[Declared]| {
            let place = position(workflow, declared).again();
            (place.position.start().map(|(stage, _)| stage.name.clone()), place.rework())
        };
        // With a human, and with the consult `build` names.
        assert_eq!(again(&[Escalate]), (Some("plan".into()), 0));
        assert_eq!(again(&[Pass, Pass, Reject, Escalate]), (Some("build".into()), 1));
        // Nothing else is set aside: a breaker, a consult's word, an end.
        for stands in [&[Reject][..], &[Pass, Escalate, Escalate], &[AlreadySatisfied], &[Pass]] {
            let place = position(workflow, stands);
            assert_eq!(place.clone().again(), place, "{stands:?}");
        }
    }

    #[test]
    fn the_words_read_back_and_a_breaker_is_named_by_its_key() {
        for declared in Declared::ALL {
            assert_eq!(Declared::read(declared.word()), Some(declared));
        }
        assert_eq!(Declared::read("needs-human"), None);
        for breaker in [Breaker::Rework, Breaker::ConsecutiveRework] {
            assert!(WORKFLOW_KEYS.contains(&breaker.key()));
        }
        let team = one(FEATURE);
        let place = position(team.workflow("feature").unwrap(), &[Reject]);
        let Position::Escalating(stage, reason) = place.position else { panic!("escalating") };
        assert_eq!(reason.said(stage), "no valid verdict was declared at the stage `plan`");
        assert_eq!(place.position.worker(), None);
    }

    #[test]
    fn a_worker_is_started_on_the_stage_to_perform_or_the_one_a_park_ended_at() {
        let team = one(FEATURE);
        let workflow = team.workflow("feature").unwrap();
        let start = |declared: &[Declared]| {
            let started = position(workflow, declared).position.start();
            started.map(|(stage, work)| format!("{} by {}", stage.name, team.role_of(work).name))
        };
        assert_eq!(start(&[]).as_deref(), Some("plan by dev"));
        assert_eq!(start(&[Pass, Pass, Reject]).as_deref(), Some("build by dev"));
        assert_eq!(start(&[Pass, Blocked]).as_deref(), Some("build by dev"));
        // A human's stage, a consult, an escalation and an end start nobody's stage.
        for none in [&[Pass, Pass, Pass][..], &[Pass, Escalate], &[Escalate], &[AlreadySatisfied]] {
            assert_eq!(start(none), None, "{none:?}");
        }
        assert_eq!(start(&[Pass, Pass, Pass, Blocked]), None, "parked at a human's stage");
    }
}

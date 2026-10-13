//! Where an issue is in its workflow, and how much rework it has had: both
//! derived from what was declared on it, in order, and stored nowhere
//! (docs/adr/0010, docs/adr/0011). A `Place` takes a loaded workflow as given
//! and checks nothing of it.

use flux_rs::attrs::*;

use crate::team::{Pass, Performer, Stage, Work, Workflow};
use crate::verdict::Phase;

/// What ends a stage or answers an escalation: one of the five verdicts, or
/// a ruling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declared {
    Pass,
    Reject,
    Blocked,
    AlreadySatisfied,
    Escalate,
    /// What a consult or a human decides on an escalated issue.
    Ruling,
}

impl Declared {
    pub const ALL: [Declared; 6] = [
        Declared::Pass,
        Declared::Reject,
        Declared::Blocked,
        Declared::AlreadySatisfied,
        Declared::Escalate,
        Declared::Ruling,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Declared::Pass => "pass",
            Declared::Reject => "reject",
            Declared::Blocked => Phase::Blocked.word(),
            Declared::AlreadySatisfied => Phase::AlreadySatisfied.word(),
            Declared::Escalate => Phase::Escalate.word(),
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

/// Why an issue is handed to a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Its stage declared `escalate` and names no consult.
    Verdict,
    /// Its stage's consult let the escalation through.
    Consult,
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
            Reason::Verdict => "declared escalate, and names no consult".to_string(),
            Reason::Consult => "consult let it through".to_string(),
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
                format!("the stage `{name}` declared `escalate`, and it names no consult")
            }
            Reason::Consult => {
                format!("the consult of the stage `{name}` let its `escalate` through")
            }
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
    /// A worker is to rule on the stage's `escalate`, as this skill of this role.
    #[variant((&Stage[@s], &Work[s.roles]) -> Position[true, s.n, s.roles])]
    Consult(&'w Stage, &'w Work),
    /// Parked on what the stage declared it blocked by. The stage runs again
    /// when those close, which counts as no rework.
    #[variant((&Stage[@s]) -> Position[true, s.n, s.roles])]
    Parked(&'w Stage),
    /// Its last stage passed to the landing train.
    #[variant(Position[false, 0, 0])]
    Landing,
    /// Nothing more to do.
    #[variant(Position[false, 0, 0])]
    Ended,
    /// Handed to a human. Under their ruling the stage runs again.
    #[variant((&Stage[@s], Reason) -> Position[true, s.n, s.roles])]
    Escalating(&'w Stage, Reason),
}

impl<'w> Position<'w> {
    /// The stage the issue is at; `None` once it has left its stages.
    #[spec(fn(Position[@p]) -> Option<&Stage{v: p.at_stage && v.n == p.n && v.roles == p.roles}>)]
    pub fn stage(self) -> Option<&'w Stage> {
        match self {
            Position::Perform(stage, _)
            | Position::Waiting(stage)
            | Position::Consult(stage, _)
            | Position::Parked(stage)
            | Position::Escalating(stage, _) => Some(stage),
            Position::Landing | Position::Ended => None,
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

    /// The breaker that handed the issue to a human; `None` where none did.
    pub fn breaker(self) -> Option<Breaker> {
        match self {
            Position::Escalating(_, Reason::Breaker(breaker)) => Some(breaker),
            _ => None,
        }
    }

    /// Who a worker is to be started as; `None` where none is to be.
    pub fn worker(self) -> Option<&'w Work> {
        match self {
            Position::Perform(_, work) | Position::Consult(_, work) => Some(work),
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
}

impl Clone for Place<'_> {
    #[spec(fn(&Place[@p]) -> Place[p])]
    fn clone(&self) -> Self {
        Place {
            workflow: self.workflow,
            position: self.position,
            total: self.total,
            consecutive: self.consecutive.clone(),
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
            Position::Consult(stage, _) | Position::Escalating(stage, Reason::Verdict) => at(stage),
            Position::Perform(..)
            | Position::Waiting(_)
            | Position::Parked(_)
            | Position::Escalating(..)
            | Position::Landing
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
        let position = match self.position {
            Position::Perform(stage, _)
            | Position::Waiting(stage)
            | Position::Consult(stage, _)
            | Position::Parked(stage) => Position::Escalating(stage, Reason::NoValidVerdict),
            Position::Escalating(..) | Position::Landing | Position::Ended => self.position,
        };
        Place { position, ..self }
    }

    /// The place after `declared`; `None` when it may not be declared here.
    #[spec(fn(&Place[@p], Declared) -> Option<Place[p]>)]
    fn valid(&self, declared: Declared) -> Option<Place<'w>> {
        let mut next = self.clone();
        next.position = match (self.position, declared) {
            (Position::Landing | Position::Ended, _) => return None,
            // A ruling on an escalated issue is a human's.
            (Position::Escalating(stage, _), Declared::Ruling) => {
                return Some(next.put(stage));
            }
            (Position::Escalating(..), _) => return None,
            // A consult's ruling is rework: the stage that asked runs again.
            (Position::Consult(stage, _), Declared::Ruling) => match self.total.more() {
                Some(total) => {
                    next.total = total;
                    at(stage)
                }
                None => Position::Escalating(stage, Reason::Breaker(Breaker::Rework)),
            },
            (Position::Consult(stage, _), Declared::Escalate) => {
                Position::Escalating(stage, Reason::Consult)
            }
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
                        (None, _) => {
                            Position::Escalating(back, Reason::Breaker(Breaker::ConsecutiveRework))
                        }
                        (_, None) => Position::Escalating(back, Reason::Breaker(Breaker::Rework)),
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
                    Some(consult) => Position::Consult(stage, consult),
                    None => Position::Escalating(stage, Reason::Verdict),
                },
                Declared::Ruling => return None,
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
            Position::Consult(stage, work) => {
                let role = &team.role_of(work).name;
                format!("consult on {} by {role}: {}", stage.name, work.skill)
            }
            Position::Parked(stage) => format!("parked at {}", stage.name),
            Position::Landing => "landing".to_string(),
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
        // Nothing is declared on an issue that has left its stages or waits
        // for a ruling: it stays where it is, and why.
        for left in [&[AlreadySatisfied][..], &[Pass, Pass, Pass, Pass], &[Escalate]] {
            let place = position(workflow, left);
            for declared in Declared::ALL.into_iter().filter(|declared| *declared != Ruling) {
                assert!(!place.takes(declared), "{left:?} then {declared:?}");
                assert_eq!(place.clone().after(declared), place);
            }
        }
        assert!(!position(workflow, &[AlreadySatisfied]).takes(Ruling));
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
    fn a_breaker_escalates_the_rework_that_would_pass_it() {
        let team = one(FEATURE);
        // `review` rejects a third time in a row: max_consecutive_rework is 2.
        let twice = [Pass, Pass, Reject, Pass, Reject, Pass];
        let third = [&twice[..], &[Reject]].concat();
        let tripped = "escalating at build: Breaker(ConsecutiveRework)".to_string();
        assert_eq!(after(&team, &third), (tripped, 2, [2, 0]));
        // A fourth rework in total, with no stage three times in a row.
        let three = [&twice[..], &[Pass, Reject, Pass, Pass]].concat();
        assert_eq!(after(&team, &three), ("review by reviewer: review".into(), 3, [0, 1]));
        let fourth = [&three[..], &[Reject]].concat();
        let tripped = "escalating at build: Breaker(Rework)".to_string();
        assert_eq!(after(&team, &fourth), (tripped, 3, [0, 1]));
        // A breaker set to 0 allows no rework at all.
        let strict = one(&format!("max_rework: 0\n{FEATURE}"));
        assert_eq!(at(&strict, &[Pass, Pass, Reject]), "escalating at build: Breaker(Rework)");
    }

    #[test]
    fn a_human_ruling_starts_both_counts_over_and_a_consults_is_rework() {
        let team = one(FEATURE);
        let tripped = [Pass, Pass, Reject, Pass, Reject, Pass, Reject];
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

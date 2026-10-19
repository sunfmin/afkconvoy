//! What the tick before saw, kept by `run` until the next one, so that tick's
//! line can say why it did what it did when the cause changed outside this
//! host in between: a claim another fleet instance took, an issue labelled,
//! closed or given a blocker on GitHub. It words the line, and nothing else:
//! what a tick does is decided by what its own rebuild reads.

use std::collections::BTreeMap;

use crate::claims::Claim;
use crate::lease::is_stale;
use crate::plan::{Issue, Mine, Status};

/// What one tick saw as it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Before {
    /// The tick, counted by its `run`.
    pub tick: u64,
    /// The claims this fleet instance held on issues.
    pub held: Vec<Held>,
    /// When this instance's heartbeat was stamped, as the remote had it.
    pub beat: Option<u64>,
    /// Every open issue, by number.
    pub open: BTreeMap<u64, Looked>,
}

/// One claim this fleet instance held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub number: u64,
    /// The commit it was created at (`Claim::id`): a takeover carries it on,
    /// a new claim on the same issue does not.
    pub id: String,
    /// It had an open PR, which may land before the next tick — the fleet
    /// closes its issue itself then — or the train had landed it.
    pub landing: bool,
}

/// One open issue as a tick saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Looked {
    /// Its labels kept the fleet from dispatching it (`plan::label_bar`).
    pub barred: bool,
    /// How many open issues blocked it.
    pub open_blockers: u64,
}

/// A claim of this fleet instance that another one holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lost {
    pub issue: u64,
    pub instance: String,
    pub host: String,
    /// It was taken as a stale claim: this instance's heartbeat was past the
    /// lease when it was. Otherwise by a takeover.
    pub stale: bool,
}

impl Lost {
    /// As a tick's line says it: `lost #46 to instance beta on host h2 (takeover)`.
    pub fn said(&self) -> String {
        let how = if self.stale { "stale: past the lease" } else { "takeover" };
        format!("lost #{} to instance {} on host {} ({how})", self.issue, self.instance, self.host)
    }
}

impl Before {
    /// What a tick saw as it ended: `mine` and `issues` as its rebuild read
    /// them, `claims` and `beat` as its steps left them. `barred` says of an
    /// issue's labels whether they keep it from being dispatched.
    pub fn saw(
        mine: &[Mine],
        issues: &[Issue],
        (claims, me, beat): (&[Claim], &str, Option<u64>),
        barred: impl Fn(&[String]) -> bool,
    ) -> Before {
        let landing = |number: u64| {
            mine.iter().any(|mine| {
                mine.number == number
                    && matches!(mine.status, Status::Joining | Status::Joined | Status::Landed)
            })
        };
        let held = claims
            .iter()
            .filter(|claim| claim.owner.as_ref().is_some_and(|owner| owner.instance == me))
            .map(|claim| Held {
                number: claim.number,
                id: claim.id.clone(),
                landing: landing(claim.number),
            })
            .collect();
        let open = issues
            .iter()
            .map(|issue| {
                let looked =
                    Looked { barred: barred(&issue.labels), open_blockers: issue.open_blockers };
                (issue.number, looked)
            })
            .collect();
        Before { tick: 0, held, beat, open }
    }

    /// The claims this instance held then that another instance holds in
    /// `claims` now — the same claim, taken: one released and claimed anew
    /// was not lost.
    pub fn lost(&self, claims: &[Claim], me: &str) -> Vec<Lost> {
        let mut lost = vec![];
        for held in &self.held {
            let now =
                claims.iter().find(|claim| claim.number == held.number && claim.id == held.id);
            let Some(owner) = now.and_then(|claim| claim.owner.as_ref()) else { continue };
            if owner.instance != me {
                lost.push(Lost {
                    issue: held.number,
                    instance: owner.instance.clone(),
                    host: owner.host.clone(),
                    stale: is_stale(self.beat, owner.ts),
                });
            }
        }
        lost
    }

    /// The tick since which issue `number` was labelled for the fleet, when
    /// that tick saw it open and barred by its labels.
    pub fn labelled(&self, number: u64) -> Option<u64> {
        self.open.get(&number).filter(|looked| looked.barred).map(|_| self.tick)
    }

    /// The tick since which issue `number` was blocked, when that tick saw it
    /// open and blocked by none.
    pub fn unblocked(&self, number: u64) -> Option<u64> {
        self.open.get(&number).filter(|looked| looked.open_blockers == 0).map(|_| self.tick)
    }

    /// Whether issue `number` was closed by something other than the fleet:
    /// this instance held it then, and not for a landing, which closes it
    /// too.
    pub fn closed_outside(&self, number: u64) -> bool {
        self.held.iter().any(|held| held.number == number && !held.landing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::Owner;

    fn claim(number: u64, id: &str, instance: &str, ts: u64) -> Claim {
        let owner = Owner { instance: instance.into(), host: format!("{instance}-host"), ts };
        Claim { number, owner: Some(owner), sha: format!("{id}-now"), id: id.into(), since: None }
    }

    fn held(number: u64, id: &str) -> Held {
        Held { number, id: id.into(), landing: false }
    }

    fn before(held: Vec<Held>, beat: Option<u64>) -> Before {
        Before { tick: 7, held, beat, open: BTreeMap::new() }
    }

    #[test]
    fn a_claim_another_instance_took_is_lost_by_takeover_or_as_stale() {
        let then = before(vec![held(3, "a"), held(5, "b"), held(9, "c")], Some(1000));
        let now =
            [claim(3, "a", "beta", 1100), claim(5, "b", "beta", 6000), claim(9, "c", "me", 1)];
        let lost = then.lost(&now, "me");
        assert_eq!(
            lost.iter().map(Lost::said).collect::<Vec<_>>(),
            [
                "lost #3 to instance beta on host beta-host (takeover)",
                "lost #5 to instance beta on host beta-host (stale: past the lease)",
            ]
        );
    }

    #[test]
    fn a_claim_released_and_made_anew_or_gone_was_not_lost() {
        let then = before(vec![held(3, "a"), held(5, "b")], Some(1000));
        assert_eq!(then.lost(&[claim(3, "new", "beta", 1100)], "me"), []);
    }

    #[test]
    fn with_no_heartbeat_then_a_taken_claim_was_stale() {
        let then = before(vec![held(3, "a")], None);
        assert!(then.lost(&[claim(3, "a", "beta", 1100)], "me")[0].stale);
    }

    #[test]
    fn an_issue_was_labelled_or_unblocked_since_a_tick_that_saw_it_otherwise() {
        let mut then = before(vec![], None);
        then.open.insert(3, Looked { barred: true, open_blockers: 0 });
        then.open.insert(5, Looked { barred: false, open_blockers: 1 });
        assert_eq!((then.labelled(3), then.labelled(5), then.labelled(9)), (Some(7), None, None));
        assert_eq!(
            (then.unblocked(3), then.unblocked(5), then.unblocked(9)),
            (Some(7), None, None)
        );
    }

    #[test]
    fn only_a_claim_held_but_not_for_a_landing_was_closed_outside() {
        let then =
            before(vec![held(3, "a"), Held { number: 5, id: "b".into(), landing: true }], None);
        assert_eq!(
            (then.closed_outside(3), then.closed_outside(5), then.closed_outside(9)),
            (true, false, false)
        );
    }
}

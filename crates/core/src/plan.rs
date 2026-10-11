//! The working set: everything a tick acts on, assembled from what GitHub and
//! the remote's refs say now. The same state gives the same working set,
//! whatever order the reads returned it in.

use flux_rs::attrs::*;
use serde::Serialize;

use crate::claims::{Claim, InstanceId};
use crate::closing::issues_closed_by;
use crate::config::Config;
use crate::lease::is_stale;

/// One open issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    /// How many open issues block it.
    pub open_blockers: u64,
}

/// One open pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub head_branch: String,
    pub head_sha: String,
    /// The issues it closes (`PullRequest::closing`).
    pub closes: Vec<u64>,
}

impl PullRequest {
    /// This PR as it is once its branch is at `head`.
    pub fn at_head(&self, head: &str) -> PullRequest {
        PullRequest { head_sha: head.to_string(), ..self.clone() }
    }

    /// The issues a PR closes, from the ones GitHub linked and its body.
    pub fn closing(body: &str, linked: &[u64]) -> Vec<u64> {
        issues_closed_by(body, linked)
    }
}

/// The open PR that closes issue `number`: the highest-numbered when several
/// do — the latest attempt is the live one.
pub fn closing_pr(prs: &[PullRequest], number: u64) -> Option<&PullRequest> {
    let mut latest: Option<&PullRequest> = None;
    for pr in prs {
        if pr.closes.contains(&number) && latest.is_none_or(|found| pr.number > found.number) {
            latest = Some(pr);
        }
    }
    latest
}

/// Why an issue's labels keep a fleet from dispatching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelBar {
    /// It carries this epic label: it is never dispatched, whoever claims it.
    Epic(String),
    /// It lacks this, the ready label.
    NotReady(String),
}

impl LabelBar {
    pub fn reason(&self) -> String {
        match self {
            LabelBar::Epic(label) => format!("epic label ({label})"),
            LabelBar::NotReady(ready) => format!("no {ready} label"),
        }
    }
}

/// What in an issue's labels keeps a fleet from dispatching it; `None` when
/// they allow it. The one home of the label rules: the frontier reads it, and
/// so does the standing of a blocker, so the two cannot disagree.
pub fn label_bar(labels: &[String], config: &Config) -> Option<LabelBar> {
    if let Some(epic) = labels.iter().find(|label| config.epic_labels.contains(label)) {
        return Some(LabelBar::Epic(epic.clone()));
    }
    if !labels.contains(&config.ready_label) {
        return Some(LabelBar::NotReady(config.ready_label.clone()));
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dispatchable {
    pub number: u64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Excluded {
    pub number: u64,
    pub reason: String,
}

/// Where one of my claims stands: what the tick does with it next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// No open PR closes the issue: its worker is still at it, or stopped.
    NoPr,
    /// An open PR closes it, and that PR is not on the landing train yet.
    Joining,
    /// Its open PR is on the landing train, at the head the PR has now.
    Joined,
    /// The landing train put its work on the base branch, and the landing was
    /// cut short before the issue was closed: to be closed, then settled.
    Landed,
    /// The issue is closed: the claim is to be settled and released.
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mine {
    pub number: u64,
    /// `None` when the issue is closed: only open issues are gathered.
    pub title: Option<String>,
    pub status: Status,
    pub pr: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerClaim {
    pub number: u64,
    /// `None`: the claim's record names nobody.
    pub instance: Option<String>,
    /// What a take or a delete of the claim is leased to.
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkingSet {
    /// The issues dispatchable right now, lowest number first.
    pub frontier: Vec<Dispatchable>,
    /// Every other open issue, with the reason it is not on the frontier.
    pub excluded: Vec<Excluded>,
    pub mine: Vec<Mine>,
    /// Claims of peers whose heartbeat is within the lease: never touched.
    pub peer_live: Vec<PeerClaim>,
    /// Claims of peers presumed dead, on open issues: work to take.
    pub stale: Vec<PeerClaim>,
    /// Claims of peers presumed dead, on closed issues: locks to delete.
    pub stale_closed: Vec<PeerClaim>,
    /// How many more claims this fleet may take.
    pub free_slots: usize,
    pub now: u64,
}

/// What the effect layer read, in whatever order it came.
pub struct Observed<'a> {
    pub issues: &'a [Issue],
    pub prs: &'a [PullRequest],
    pub claims: &'a [Claim],
    /// instance → the time of its heartbeat.
    pub heartbeats: &'a [(String, u64)],
    /// The claimed issues that are closed (`issues` holds only open ones).
    pub closed: &'a [u64],
    /// The issues whose open PR is on the landing train at its current head.
    pub joined: &'a [u64],
    /// My open issues the landing train landed under my claim.
    pub landed: &'a [u64],
}

/// How many more claims a fleet holding `held` may take under `concurrency`:
/// what is left under the bound, and nothing at or over it.
#[spec(fn(concurrency: usize, held: usize) -> usize{free:
    (held >= concurrency => free == 0) && (held < concurrency => held + free == concurrency)})]
// Spelled out, not `saturating_sub`: Flux proves the spec from the branch.
#[allow(clippy::implicit_saturating_sub)]
pub fn free_slots(concurrency: usize, held: usize) -> usize {
    if held >= concurrency { 0 } else { concurrency - held }
}

/// Assembles the working set of the fleet instance `me` (`None`: of nobody —
/// every claim is a peer's).
pub fn assemble(seen: &Observed, me: Option<&InstanceId>, now: u64, config: &Config) -> WorkingSet {
    let mut issues: Vec<&Issue> = seen.issues.iter().collect();
    issues.sort_by_key(|issue| issue.number);
    let mut claims: Vec<&Claim> = seen.claims.iter().collect();
    claims.sort_by_key(|claim| claim.number);

    let mut set = WorkingSet {
        frontier: vec![],
        excluded: vec![],
        mine: vec![],
        peer_live: vec![],
        stale: vec![],
        stale_closed: vec![],
        free_slots: 0,
        now,
    };
    for issue in &issues {
        let claimed = claims.iter().any(|claim| claim.number == issue.number);
        let reason = if let Some(bar) = label_bar(&issue.labels, config) {
            bar.reason()
        } else if claimed {
            "already claimed".to_string()
        } else if closing_pr(seen.prs, issue.number).is_some() {
            "has an open linked PR".to_string()
        } else if issue.open_blockers > 0 {
            format!("{} open blocker(s)", issue.open_blockers)
        } else {
            set.frontier.push(Dispatchable { number: issue.number, title: issue.title.clone() });
            continue;
        };
        set.excluded.push(Excluded { number: issue.number, reason });
    }
    for claim in claims {
        let closed = seen.closed.contains(&claim.number);
        if me.is_some_and(|me| claim.is_held_by(me)) {
            let pr = closing_pr(seen.prs, claim.number).map(|pr| pr.number);
            let status = if closed {
                Status::Closed
            } else if seen.landed.contains(&claim.number) {
                Status::Landed
            } else if pr.is_none() {
                Status::NoPr
            } else if seen.joined.contains(&claim.number) {
                Status::Joined
            } else {
                Status::Joining
            };
            let issue = issues.iter().find(|issue| issue.number == claim.number);
            let title = issue.map(|issue| issue.title.clone());
            set.mine.push(Mine { number: claim.number, title, status, pr });
            continue;
        }
        let instance = claim.owner.as_ref().map(|owner| owner.instance.clone());
        let beat = instance.as_ref().and_then(|instance| {
            seen.heartbeats.iter().find(|(name, _)| name == instance).map(|(_, ts)| *ts)
        });
        let row = PeerClaim { number: claim.number, instance, sha: claim.sha.clone() };
        if !is_stale(beat, now) {
            set.peer_live.push(row);
        } else if closed {
            set.stale_closed.push(row);
        } else {
            set.stale.push(row);
        }
    }
    set.free_slots = free_slots(config.concurrency(), set.mine.len());
    set
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A fleet at its bound has no slot, whatever it would like.
    #[should_fail]
    #[spec(fn(concurrency: usize, held: usize{held >= concurrency}) -> usize{free: free > 0})]
    fn a_slot_past_the_bound(concurrency: usize, held: usize) -> usize {
        free_slots(concurrency, held)
    }

    /// Slots taken never push the claims held past the bound.
    #[spec(fn(concurrency: usize, held: usize, taken: usize) -> usize{after:
        after <= concurrency || after == held})]
    fn taking_stays_under_the_bound(concurrency: usize, held: usize, taken: usize) -> usize {
        let free = free_slots(concurrency, held);
        if taken <= free { held + taken } else { held }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::Owner;
    use crate::config::Settings;
    use crate::lease::LEASE;

    fn config(concurrency: usize) -> Config {
        Config::new(Settings {
            base_branch: Some("main".into()),
            gate_command: Some("true".into()),
            concurrency: Some(concurrency),
            ..Settings::default()
        })
        .unwrap()
    }

    fn issue(number: u64, labels: &[&str]) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            open_blockers: 0,
        }
    }

    fn pr(number: u64, closes: u64) -> PullRequest {
        PullRequest {
            number,
            title: format!("pr {number}"),
            head_branch: format!("b{number}"),
            head_sha: format!("sha{number}"),
            closes: vec![closes],
        }
    }

    fn claim(number: u64, instance: &str) -> Claim {
        let owner = Owner { instance: instance.into(), host: "h".into(), ts: 0 };
        Claim { number, owner: Some(owner), sha: format!("c{number}") }
    }

    fn me() -> InstanceId {
        InstanceId::parse("me").unwrap()
    }

    const READY: &[&str] = &["ready-for-agent"];

    #[test]
    fn the_frontier_is_every_issue_nothing_bars() {
        let mut blocked = issue(6, READY);
        blocked.open_blockers = 2;
        let issues = [
            issue(9, READY),
            issue(1, READY),
            issue(2, &["bug"]),
            issue(3, &["ready-for-agent", "epic"]),
            issue(4, READY),
            issue(5, READY),
            blocked,
        ];
        let seen = Observed {
            issues: &issues,
            prs: &[pr(40, 5)],
            claims: &[claim(4, "peer")],
            heartbeats: &[("peer".into(), 1000)],
            closed: &[],
            joined: &[],
            landed: &[],
        };
        let set = assemble(&seen, Some(&me()), 1000, &config(3));
        let frontier: Vec<u64> = set.frontier.iter().map(|d| d.number).collect();
        assert_eq!(frontier, [1, 9], "in issue-number order");
        let reasons: Vec<(u64, &str)> =
            set.excluded.iter().map(|e| (e.number, e.reason.as_str())).collect();
        assert_eq!(
            reasons,
            [
                (2, "no ready-for-agent label"),
                (3, "epic label (epic)"),
                (4, "already claimed"),
                (5, "has an open linked PR"),
                (6, "2 open blocker(s)"),
            ]
        );
    }

    #[test]
    fn claims_are_partitioned_by_owner_and_liveness() {
        let issues = [issue(1, READY), issue(2, READY), issue(3, READY), issue(5, READY)];
        let nobody = Claim { number: 5, owner: None, sha: "c5".into() };
        let claims = [
            claim(1, "me"),
            claim(2, "live"),
            claim(3, "dead"),
            claim(4, "dead"),
            nobody,
            claim(6, "me"),
            claim(7, "me"),
            claim(8, "me"),
            claim(9, "me"),
        ];
        let seen = Observed {
            issues: &issues,
            prs: &[pr(70, 7), pr(80, 8)],
            claims: &claims,
            // my own heartbeat has expired: my claims stay mine
            heartbeats: &[
                ("live".into(), 10_000 - LEASE),
                ("dead".into(), 9_999 - LEASE),
                ("me".into(), 1),
            ],
            closed: &[4, 6],
            joined: &[8],
            landed: &[9],
        };
        let set = assemble(&seen, Some(&me()), 10_000, &config(5));
        let mine: Vec<_> = set.mine.iter().map(|m| (m.number, m.status, m.pr)).collect();
        assert_eq!(
            mine,
            [
                (1, Status::NoPr, None),
                (6, Status::Closed, None),
                (7, Status::Joining, Some(70)),
                (8, Status::Joined, Some(80)),
                (9, Status::Landed, None),
            ]
        );
        assert_eq!(set.mine[0].title.as_deref(), Some("issue 1"));
        assert_eq!(set.mine[1].title, None);
        assert_eq!(set.peer_live.iter().map(|c| c.number).collect::<Vec<_>>(), [2]);
        assert_eq!(set.stale.iter().map(|c| c.number).collect::<Vec<_>>(), [3, 5]);
        assert_eq!(set.stale[1].instance, None, "a claim that names nobody is stale to everyone");
        assert_eq!(
            set.stale_closed.iter().map(|c| (c.number, c.sha.as_str())).collect::<Vec<_>>(),
            [(4, "c4")]
        );
        assert_eq!(set.free_slots, 0);
    }

    #[test]
    fn a_heartbeat_from_the_future_is_stale() {
        let seen = Observed {
            issues: &[issue(1, READY)],
            prs: &[],
            claims: &[claim(1, "peer")],
            heartbeats: &[("peer".into(), 5000)],
            closed: &[],
            joined: &[],
            landed: &[],
        };
        assert_eq!(assemble(&seen, None, 1000, &config(3)).stale.len(), 1);
    }

    #[test]
    fn free_slots_never_go_below_zero() {
        assert_eq!(free_slots(3, 0), 3);
        assert_eq!(free_slots(3, 3), 0);
        assert_eq!(free_slots(3, 7), 0);
    }

    #[test]
    fn the_same_state_gives_the_same_working_set_in_any_order() {
        let mut issues = vec![issue(3, READY), issue(1, READY), issue(2, READY), issue(8, &[])];
        let mut prs = vec![pr(21, 2), pr(20, 2), pr(22, 8)];
        let mut claims = vec![claim(3, "me"), claim(8, "peer"), claim(9, "gone")];
        let mut beats = vec![("peer".to_string(), 990u64), ("me".to_string(), 1000)];
        let config = config(2);
        let build =
            |issues: &[Issue], prs: &[PullRequest], claims: &[Claim], beats: &[(String, u64)]| {
                let seen = Observed {
                    issues,
                    prs,
                    claims,
                    heartbeats: beats,
                    closed: &[9],
                    joined: &[],
                    landed: &[],
                };
                assemble(&seen, Some(&me()), 1000, &config)
            };
        let first = build(&issues, &prs, &claims, &beats);
        for _ in 0..6 {
            issues.rotate_left(1);
            prs.reverse();
            claims.rotate_left(1);
            beats.reverse();
            assert_eq!(build(&issues, &prs, &claims, &beats), first);
        }
        assert_eq!(closing_pr(&prs, 2).unwrap().number, 21, "the latest PR is the live one");
    }
}

//! A heavy command runs alone on its host (docs/adr/0019): the queue of
//! `afkconvoy exclusive` commands that every fleet of a host shares, held by
//! its host runner (docs/adr/0023). The order it runs in, how many are ahead
//! of a ticket, and since when a worker has waited in it are decided here;
//! holding it, and the line each asker is kept on, are the effect layer's.

use serde::{Deserialize, Serialize};

/// One `exclusive` command in the queue, as its ticket says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// The order it joined the queue in, as the host runner numbers it: of
    /// two that asked the same second, the lower runs first.
    pub number: u64,
    /// The process that waits, or runs the command.
    pub pid: u32,
    /// The command, as one line.
    pub command: String,
    /// The worktree it was run in — or the directory, outside a checkout.
    pub dir: String,
    /// The issue of the worker that ran it, where its environment says one.
    pub issue: Option<u64>,
    /// A cut goes ahead of waiting worker commands so their queue cannot
    /// consume its time limit. Running commands keep their lanes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
    /// A light gate's repo lane, independent of both host lanes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_lane: Option<String>,
    /// The shared git directory, identifying a repo across its worktrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// A joining turn, which excludes another turn of its repo even when
    /// the two branches configure different kinds of gate.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_turn: bool,
    /// An immutable candidate worktree may overlap another candidate of
    /// its repo. Ordinary commands and serial barriers retain exclusion.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub candidate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<crate::candidate::Validation>,
    /// When it asked to run, in epoch seconds: within each priority the one
    /// that asked first runs first, including in the next host runner.
    pub asked: u64,
    /// When it began to run; `None`: it waits.
    pub ran: Option<u64>,
}

/// How many of `queue` are ahead of its ticket `number`: it runs once none
/// is. `None`: `number` is in it no more.
pub fn ahead(queue: &[Ticket], number: u64) -> Option<usize> {
    let mine = queue.iter().find(|ticket| ticket.number == number)?;
    Some(
        queue
            .iter()
            .filter(|ticket| same_lane(ticket, mine) && place(ticket) < place(mine))
            .count(),
    )
}

/// Commands contend only with others in the same lane.
pub fn same_lane(a: &Ticket, b: &Ticket) -> bool {
    a.repo_lane == b.repo_lane
}

/// A repo has one identity across its worktrees. Older tickets identify
/// only their directory and still exclude another command from there.
pub fn same_repo(a: &Ticket, b: &Ticket) -> bool {
    a.repo.as_deref().unwrap_or(&a.dir) == b.repo.as_deref().unwrap_or(&b.dir)
}

/// Whether this ticket can start with the host's lane counts. Earlier
/// requests reserve available lanes in order, but a second request of a
/// running repo waits without holding another repo back from a free lane.
pub fn ready(queue: &[Ticket], number: u64, worker_lanes: u64) -> bool {
    scheduling(queue, worker_lanes, false)
        .iter()
        .any(|request| request.number == number && request.state == State::Eligible)
}

/// The scheduler's decision for a request at this observation. Eligible
/// requests reserve lanes in scheduler order; this is no future start order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    Eligible,
    Waiting,
}

/// A request occupying capacity now, or reserving it earlier in FIFO order.
#[derive(Debug, Clone, Serialize)]
pub struct Holder {
    pub number: u64,
    pub state: State,
}

/// One actual constraint, with the requests that hold or reserve it.
#[derive(Debug, Clone, Serialize)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub requests: Vec<Holder>,
    pub capacity: Option<u64>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    SameRepo,
    WorkerCapacity,
    RepoCapacity,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scheduling {
    pub number: u64,
    pub state: State,
    pub reasons: Vec<Constraint>,
}

/// Interpret every request with the very same reservations used to grant a
/// lane, with cuts before ordinary waiters. A same-repo waiter reserves nothing. Turns exclude each other
/// across worker and repo lanes too. Pausing prevents every new grant.
pub fn scheduling(queue: &[Ticket], worker_lanes: u64, paused: bool) -> Vec<Scheduling> {
    let ordered = in_order(queue.to_vec());
    let mut admitted: Vec<&Ticket> = ordered.iter().filter(|t| t.ran.is_some()).collect();
    let mut decisions: Vec<Scheduling> = admitted
        .iter()
        .map(|ticket| Scheduling { number: ticket.number, state: State::Running, reasons: vec![] })
        .collect();
    for ticket in ordered.iter().filter(|t| t.ran.is_none()) {
        let limit = if ticket.repo_lane.is_some() { 1 } else { worker_lanes };
        let occupied: Vec<_> = admitted.iter().copied().filter(|t| same_lane(t, ticket)).collect();
        let own: Vec<_> = admitted
            .iter()
            .copied()
            .filter(|t| {
                (same_lane(t, ticket) || (t.is_turn && ticket.is_turn))
                    && same_repo(t, ticket)
                    && !(t.candidate && ticket.candidate)
            })
            .collect();
        let mut reasons = vec![];
        if paused {
            reasons.push(Constraint {
                kind: ConstraintKind::Paused,
                requests: vec![],
                capacity: None,
                explanation:
                    "the host runner has paused new grants while moving to another release".into(),
            });
        }
        if !own.is_empty() {
            reasons.push(Constraint {
                kind: ConstraintKind::SameRepo,
                requests: holders(&own),
                capacity: None,
                explanation: "this repo already holds or reserves this lane, or another turn of this repo holds or reserves a different lane".into(),
            });
        }
        if occupied.len() as u64 >= limit {
            reasons.push(Constraint {
                kind: if ticket.repo_lane.is_some() {
                    ConstraintKind::RepoCapacity
                } else {
                    ConstraintKind::WorkerCapacity
                },
                requests: holders(&occupied),
                capacity: Some(limit),
                explanation: format!("all {limit} {} lane places are held by running requests or reserved by earlier eligible requests", lane(ticket)),
            });
        }
        let state = if reasons.is_empty() { State::Eligible } else { State::Waiting };
        if state == State::Eligible {
            admitted.push(ticket);
        }
        decisions.push(Scheduling { number: ticket.number, state, reasons });
    }
    decisions
}

fn holders(tickets: &[&Ticket]) -> Vec<Holder> {
    tickets
        .iter()
        .map(|ticket| Holder {
            number: ticket.number,
            state: if ticket.ran.is_some() { State::Running } else { State::Eligible },
        })
        .collect()
}

/// Actual occupancy counts only running requests, never reservations. Free
/// capacity does not imply that a request of an occupied repo can use it.
#[derive(Debug, Serialize)]
pub struct Capacity {
    pub capacity: u64,
    pub used: u64,
    pub free: u64,
}

#[derive(Debug, Serialize)]
pub struct RepoLane {
    pub repo: String,
    #[serde(flatten)]
    pub occupancy: Capacity,
}

/// A ticket with durations at one observation time and its scheduler decision.
#[derive(Debug, Serialize)]
pub struct Observed {
    #[serde(flatten)]
    pub ticket: Ticket,
    pub lane: String,
    pub elapsed_seconds: Option<u64>,
    pub waiting_seconds: u64,
    pub scheduling: Scheduling,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub paused: bool,
    pub worker: Capacity,
    pub repos: Vec<RepoLane>,
    pub running: Vec<Observed>,
    pub waiting: Vec<Observed>,
}

/// Common queue facts for operator output, doctor and the panel. Running
/// and waiting arrays each put cuts first, then follow ask time and request number. Repo lanes
/// are separate from worker capacity and sorted by their whole identity.
pub fn snapshot(queue: &[Ticket], worker_lanes: u64, paused: bool, now: u64) -> Snapshot {
    let ordered = in_order(queue.to_vec());
    let decisions = scheduling(&ordered, worker_lanes, paused);
    let mut repos: Vec<String> = ordered.iter().filter_map(|t| t.repo_lane.clone()).collect();
    repos.sort();
    repos.dedup();
    let repos = repos
        .into_iter()
        .map(|repo| RepoLane { occupancy: occupancy(&ordered, Some(&repo), 1), repo })
        .collect();
    let mut result = Snapshot {
        paused,
        worker: occupancy(&ordered, None, worker_lanes),
        repos,
        running: vec![],
        waiting: vec![],
    };
    for (ticket, scheduling) in ordered.into_iter().zip(decisions) {
        let observed = Observed {
            lane: lane(&ticket),
            elapsed_seconds: ticket.ran.map(|ran| now.saturating_sub(ran)),
            waiting_seconds: ticket.ran.unwrap_or(now).saturating_sub(ticket.asked),
            scheduling,
            ticket,
        };
        if observed.ticket.ran.is_some() {
            result.running.push(observed);
        } else {
            result.waiting.push(observed);
        }
    }
    result
}

fn occupancy(queue: &[Ticket], repo_lane: Option<&str>, capacity: u64) -> Capacity {
    let used =
        queue.iter().filter(|t| t.ran.is_some() && t.repo_lane.as_deref() == repo_lane).count()
            as u64;
    Capacity { capacity, used, free: capacity.saturating_sub(used) }
}

/// The lane a ticket names in the log, doctor and the panel.
pub fn lane(ticket: &Ticket) -> String {
    match &ticket.repo_lane {
        Some(repo) => format!("repo {repo}"),
        None => "worker".to_string(),
    }
}

/// Stable scheduler order: running first, then waiting; within each group,
/// cuts first, then ask time and request number. Position alone never
/// predicts which request starts next.
pub fn in_order(mut queue: Vec<Ticket>) -> Vec<Ticket> {
    queue.sort_by_key(place);
    queue
}

/// Running commands first, then waiting cuts, then other waiters; each
/// group keeps ask time and ticket order within its lanes.
fn place(ticket: &Ticket) -> (bool, bool, u64, u64) {
    (ticket.ran.is_none(), !ticket.cut, ticket.asked, ticket.number)
}

/// Since when a worker in the worktree `dir` has waited in the queue — or run
/// what it waited for, its wait over — by the tickets of `live`: since the
/// first of its that asked. `None`: it has nothing in the queue. A ticket
/// counts for `dir` when it was run there or in a directory under it.
pub fn queued_since(live: &[Ticket], dir: &str) -> Option<u64> {
    let dir = dir.trim_end_matches('/');
    let mut since: Option<u64> = None;
    for ticket in live {
        if !within(&ticket.dir, dir) {
            continue;
        }
        since = match since {
            Some(earlier) if earlier <= ticket.asked => Some(earlier),
            _ => Some(ticket.asked),
        };
    }
    since
}

/// Whether the directory `of` is `dir` or one under it.
fn within(of: &str, dir: &str) -> bool {
    match of.strip_prefix(dir) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// What the log says of a ticket that has to wait: `ahead` are before it.
pub fn waits_line(ticket: &Ticket, ahead: usize) -> String {
    let before = match ahead {
        1 => "1 exclusive command".to_string(),
        many => format!("{many} exclusive commands"),
    };
    format!("waits behind {before} on this host to run `{}`", ticket.command)
}

/// What the log says of a ticket that runs its command in no queue, as `why`
/// says: no host runner runs, or the one there cannot hold it.
pub fn at_once_line(ticket: &Ticket, why: &str) -> String {
    format!("runs `{}` at once, in no queue: {why}", ticket.command)
}

/// What the log says of a ticket whose host runner ended as it waited: it
/// waits in the next one's queue, as it first asked.
pub fn rejoins_line(ticket: &Ticket) -> String {
    format!(
        "the host runner ended as `{}` waited: it waits in the next one's queue, from when it \
         asked",
        ticket.command
    )
}

/// What the log says of a ticket that runs its command at `now`.
pub fn runs_line(ticket: &Ticket, now: u64, ahead_of: usize) -> String {
    let waited = now.saturating_sub(ticket.asked);
    let _ = ahead_of;
    let line =
        format!("runs `{}` in the {} lane, after waiting {waited}s", ticket.command, lane(ticket));
    if ticket.cut {
        format!("runs as a cut, ahead of waiting worker commands; {line}")
    } else {
        line
    }
}

/// What the log says of a ticket whose command ended at `now`, as `ended`
/// says: `exit 0`, `killed by signal 9`.
pub fn ended_line(ticket: &Ticket, ended: &str, now: u64) -> String {
    let ran = now.saturating_sub(ticket.ran.unwrap_or(ticket.asked));
    format!("`{}` ended ({ended}) after {ran}s", ticket.command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(number: u64, dir: &str, asked: u64) -> Ticket {
        Ticket {
            number,
            pid: 1,
            command: "cargo test".into(),
            dir: dir.into(),
            issue: Some(7),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            validation: None,
            asked,
            ran: None,
        }
    }

    #[test]
    fn a_ticket_runs_once_none_that_asked_before_it_is_left() {
        let queue = [ticket(4, "/a", 0), ticket(9, "/b", 0), ticket(12, "/c", 0)];
        assert_eq!(ahead(&queue, 4), Some(0));
        assert_eq!(ahead(&queue, 12), Some(2));
        assert_eq!(ahead(&queue, 5), None, "one that left");
        let order = in_order(vec![ticket(9, "/b", 0), ticket(4, "/a", 0)]);
        let numbers: Vec<u64> = order.iter().map(|ticket| ticket.number).collect();
        assert_eq!(numbers, [4, 9]);
    }

    #[test]
    fn one_that_asked_again_of_the_next_host_runner_keeps_its_place_but_one_runs_first() {
        // 1 joined first and runs; 3 asked before 2, of the host runner that ended.
        let mut runs = ticket(1, "/a", 200);
        runs.ran = Some(210);
        let queue = [runs, ticket(2, "/b", 150), ticket(3, "/c", 100), ticket(4, "/d", 100)];
        let order = in_order(queue.to_vec());
        let numbers: Vec<u64> = order.iter().map(|ticket| ticket.number).collect();
        assert_eq!(numbers, [1, 3, 4, 2]);
        assert_eq!(ahead(&queue, 3), Some(1));
        assert_eq!(ahead(&queue, 2), Some(3));
    }

    #[test]
    fn light_repos_have_independent_turns() {
        let mut a = ticket(1, "/a", 100);
        a.repo_lane = Some("a".into());
        let mut b = ticket(2, "/b", 101);
        b.repo_lane = Some("b".into());
        let mut another_a = ticket(3, "/a", 102);
        another_a.repo_lane = Some("a".into());
        let queue = [a, b, another_a, ticket(4, "/c", 103)];
        assert_eq!(ahead(&queue, 2), Some(0));
        assert_eq!(ahead(&queue, 3), Some(1));
        assert_eq!(ahead(&queue, 4), Some(0));
    }

    #[test]
    fn two_lanes_admit_different_repos_and_skip_a_second_turn_of_a_running_repo() {
        let mut a = ticket(1, "/a/one", 100);
        a.repo = Some("a".into());
        a.ran = Some(100);
        let mut another_a = ticket(2, "/a/two", 101);
        another_a.repo = Some("a".into());
        let mut b = ticket(3, "/b/one", 102);
        b.repo = Some("b".into());
        let queue = [a, another_a, b];
        assert!(!ready(&queue, 2, 2));
        assert!(ready(&queue, 3, 2));
        assert!(!ready(&queue, 3, 1));
    }

    #[test]
    fn only_independent_candidate_worktrees_may_overlap_in_one_repo() {
        let mut a = ticket(1, "/a/source-one", 0);
        a.repo = Some("a".into());
        a.is_turn = true;
        a.candidate = true;
        a.ran = Some(0);
        let mut b = ticket(2, "/a/source-two", 1);
        b.repo = Some("a".into());
        b.is_turn = true;
        b.candidate = true;
        assert!(ready(&[a.clone(), b.clone()], 2, 2));
        assert!(!ready(&[a.clone(), b.clone()], 2, 1));
        b.candidate = false;
        assert!(!ready(&[a.clone(), b.clone()], 2, 2), "ordinary work retains exclusion");
        a.candidate = false;
        b.candidate = true;
        assert!(!ready(&[a, b], 2, 2), "a barrier excludes speculative validation");
    }

    #[test]
    fn an_older_ticket_has_no_train_priority() {
        let older = r#"{"number":1,"pid":2,"command":"cargo test","dir":"/a","issue":null,"asked":100,"ran":null}"#;
        let ticket: Ticket = serde_json::from_str(older).unwrap();
        assert_eq!(lane(&ticket), "worker");
    }

    #[test]
    fn a_worktree_waits_since_its_first_ticket_asked_and_counts_no_others() {
        let live = [
            ticket(1, "/w/issue-7", 300),
            ticket(2, "/w/issue-7/crates/cli", 200),
            ticket(3, "/w/issue-70", 100),
        ];
        assert_eq!(queued_since(&live, "/w/issue-7"), Some(200));
        assert_eq!(queued_since(&live, "/w/issue-7/"), Some(200));
        assert_eq!(queued_since(&live, "/w/issue-70"), Some(100));
        assert_eq!(queued_since(&live, "/w/issue-8"), None);
    }

    #[test]
    fn each_line_names_the_command() {
        let mut waits = ticket(2, "/a", 100);
        assert_eq!(
            waits_line(&waits, 1),
            "waits behind 1 exclusive command on this host to run `cargo test`"
        );
        assert_eq!(
            runs_line(&waits, 130, 0),
            "runs `cargo test` in the worker lane, after waiting 30s"
        );
        assert_eq!(
            at_once_line(&waits, "no host runner runs on this host"),
            "runs `cargo test` at once, in no queue: no host runner runs on this host"
        );
        assert_eq!(
            rejoins_line(&waits),
            "the host runner ended as `cargo test` waited: it waits in the next one's queue, \
             from when it asked"
        );
        waits.ran = Some(130);
        assert_eq!(ended_line(&waits, "exit 0", 175), "`cargo test` ended (exit 0) after 45s");
    }
}

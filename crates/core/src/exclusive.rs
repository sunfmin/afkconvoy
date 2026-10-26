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
    /// Whether it is the landing train's: ahead of other waiting commands.
    /// Older askers name no priority and remain ordinary commands.
    #[serde(default)]
    pub train: bool,
    /// A cut's command has the same priority as a train's, so workers'
    /// queued gates cannot run out its gate or cut limit.
    #[serde(default)]
    pub cut: bool,
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
    Some(queue.iter().filter(|ticket| place(ticket) < place(mine)).count())
}

/// `queue` in the order it runs: the first runs — or is about to — and the
/// rest wait.
pub fn in_order(mut queue: Vec<Ticket>) -> Vec<Ticket> {
    queue.sort_by_key(place);
    queue
}

/// Where `ticket` stands in the order the queue runs: one that runs before
/// every one that waits, then trains before other waiters, then the one
/// that asked first, then the one that joined first.
fn place(ticket: &Ticket) -> (bool, bool, u64, u64) {
    (ticket.ran.is_none(), !(ticket.train || ticket.cut), ticket.asked, ticket.number)
}

/// How many earlier ordinary commands `ticket` runs ahead of as a train's
/// command gets its turn. Commands already running are never passed.
pub fn runs_ahead_of(ticket: &Ticket, queue: &[Ticket]) -> usize {
    if !ticket.train && !ticket.cut {
        return 0;
    }
    queue
        .iter()
        .filter(|other| {
            other.ran.is_none()
                && !other.train
                && !other.cut
                && (other.asked, other.number) < (ticket.asked, ticket.number)
        })
        .count()
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
    let line = format!("runs `{}` alone on this host, after waiting {waited}s", ticket.command);
    if ticket.train {
        return format!(
            "runs ahead of {ahead_of} waiting commands, as the landing train's; {line}"
        );
    }
    if ticket.cut {
        return format!("runs ahead of {ahead_of} waiting commands, as a cut's; {line}");
    }
    line
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
            train: false,
            cut: false,
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
    fn trains_pass_waiting_workers_in_request_order_and_never_the_running_command() {
        let mut running = ticket(1, "/a", 100);
        running.ran = Some(100);
        let worker = ticket(2, "/b", 101);
        let mut train = ticket(4, "/a/convoy-train", 102);
        train.train = true;
        let mut later_train = ticket(5, "/b/convoy-train-2", 102);
        later_train.train = true;
        let queue = [running, worker, ticket(3, "/c", 101), train.clone()];
        assert_eq!(ahead(&queue, 4), Some(1));
        assert_eq!(runs_ahead_of(&train, &queue), 2);
        assert_eq!(runs_ahead_of(&queue[1], &queue), 0);
        let mut queue = queue.to_vec();
        queue.push(later_train.clone());
        // A worker that asks after the trains is not one they passed.
        queue.push(ticket(6, "/d", 103));
        assert_eq!(ahead(&queue, 5), Some(2));
        assert_eq!(runs_ahead_of(&later_train, &queue), 2);
        let order: Vec<_> = in_order(queue).iter().map(|ticket| ticket.number).collect();
        assert_eq!(order, [1, 4, 5, 2, 3, 6]);
        assert_eq!(
            runs_line(&train, 130, 2),
            "runs ahead of 2 waiting commands, as the landing train's; \
             runs `cargo test` alone on this host, after waiting 28s"
        );
    }

    #[test]
    fn an_older_ticket_has_no_train_priority() {
        let older = r#"{"number":1,"pid":2,"command":"cargo test","dir":"/a","issue":null,"asked":100,"ran":null}"#;
        let ticket: Ticket = serde_json::from_str(older).unwrap();
        assert!(!ticket.train);
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
            "runs `cargo test` alone on this host, after waiting 30s"
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

//! A heavy command runs alone on its host (docs/adr/0019): the queue of
//! `afkconvoy exclusive` commands that every fleet of a host shares, as its
//! tickets say it stands. The place a new ticket takes, which one runs, and
//! since when a worker has waited in it are decided here; the tickets' files,
//! and the locks that say whose process is alive, are the effect layer's.

use serde::{Deserialize, Serialize};

/// One `exclusive` command in the queue, as its ticket says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// Its place in the queue: the lower asked first.
    pub number: u64,
    /// The process that waits, or runs the command.
    pub pid: u32,
    /// The command, as one line.
    pub command: String,
    /// The worktree it was run in — or the directory, outside a checkout.
    pub dir: String,
    /// The issue of the worker that ran it, where its environment says one.
    pub issue: Option<u64>,
    /// When it asked to run, in epoch seconds.
    pub asked: u64,
    /// When it began to run; `None`: it waits.
    pub ran: Option<u64>,
}

/// The place a ticket takes in a queue whose tickets hold `numbers`: after
/// every one of them.
pub fn next_number(numbers: &[u64]) -> u64 {
    let mut highest = 0;
    for &number in numbers {
        if number > highest {
            highest = number;
        }
    }
    highest.saturating_add(1)
}

/// How many of `live` — the tickets of live processes — are ahead of the
/// ticket `number`: it runs once none is.
pub fn ahead(live: &[Ticket], number: u64) -> usize {
    live.iter().filter(|ticket| ticket.number < number).count()
}

/// `live` in the order it runs: the first runs — or is about to — and the
/// rest wait.
pub fn in_order(mut live: Vec<Ticket>) -> Vec<Ticket> {
    live.sort_by_key(|ticket| ticket.number);
    live
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

/// What the log says of a ticket that runs its command at `now`.
pub fn runs_line(ticket: &Ticket, now: u64) -> String {
    let waited = now.saturating_sub(ticket.asked);
    format!("runs `{}` alone on this host, after waiting {waited}s", ticket.command)
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
            asked,
            ran: None,
        }
    }

    #[test]
    fn a_ticket_takes_its_place_after_every_ticket_there() {
        assert_eq!(next_number(&[]), 1);
        assert_eq!(next_number(&[3, 9, 4]), 10);
        assert_eq!(next_number(&[u64::MAX]), u64::MAX, "never wraps to the front");
    }

    #[test]
    fn a_ticket_runs_once_no_live_one_asked_before_it() {
        let live = [ticket(4, "/a", 0), ticket(9, "/b", 0), ticket(12, "/c", 0)];
        assert_eq!(ahead(&live, 4), 0);
        assert_eq!(ahead(&live, 12), 2);
        let order = in_order(vec![ticket(9, "/b", 0), ticket(4, "/a", 0)]);
        let numbers: Vec<u64> = order.iter().map(|ticket| ticket.number).collect();
        assert_eq!(numbers, [4, 9]);
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
            runs_line(&waits, 130),
            "runs `cargo test` alone on this host, after waiting 30s"
        );
        waits.ran = Some(130);
        assert_eq!(ended_line(&waits, "exit 0", 175), "`cargo test` ended (exit 0) after 45s");
    }
}

//! The holder of a host's exclusive queue (docs/adr/0019, docs/adr/0023): the
//! one process of a host that keeps every ticket waiting or running, grants
//! each its lane in the order `exclusive` decides, and ends the hold of a
//! command that held others up past the host's bound (docs/adr/0031).
//!
//! A host process embeds it — afkconvoy's host runner, or another program's
//! daemon — and says through `Host` what only it knows: the time, the host's
//! lanes and bound, the tickets it holds outside this queue, and what must
//! be done before a ticket leaves. How an asker reaches the holder, and is
//! told its turn, is the embedder's: each asker's thread `join`s, waits for
//! its `turn`, and `leave`s once its command ended or its line closed.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::exclusive::{
    Lanes, Ticket, ahead, in_order, is_fast, limit, ready, same_lane, same_repo,
};

/// How often a waiting turn looks again, besides being woken by a change.
const CHECK_EVERY: Duration = Duration::from_millis(100);

/// What only the process that embeds the holder knows.
pub trait Host<N>: Send + Sync {
    /// The time, in epoch seconds: when a ticket's command begins to run.
    fn now(&self) -> u64;

    /// How many commands the host lets run at once in each lane, now: a
    /// changed setting applies to the next grant.
    fn lanes(&self) -> Lanes;

    /// How long, in seconds, a running command may hold others up before its
    /// asker is told to end it, now: a changed setting applies to the command
    /// already running.
    fn bound(&self) -> u64;

    /// Tickets running outside this queue that hold lanes of the same host
    /// — a lease a command's descendants inherit, say — to be counted
    /// beside its own. One of them that this queue already runs is not
    /// counted twice. Errors: they cannot be read.
    fn beside(&self) -> Result<Vec<Ticket<N>>, String> {
        Ok(vec![])
    }

    /// Whether a waiting `ticket` may be granted a lane at all, beside what
    /// the lanes allow: its repo is free to be worked in, say.
    fn grantable(&self, ticket: &Ticket<N>) -> bool {
        let _ = ticket;
        true
    }

    /// Done as `ticket` leaves the queue → whether it may leave: `false`
    /// keeps its lane held, as what it ran could not be confirmed ended.
    fn leaving(&self, ticket: &Ticket<N>) -> bool {
        let _ = ticket;
        true
    }
}

/// How a ticket's wait for its turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// Its command runs, from this time in epoch seconds.
    Runs(u64),
    /// It left the queue as it waited.
    Left,
    /// The holder was closed as it waited: it waits in the next holder's
    /// queue, from when it first asked.
    Closed,
}

/// What the holder tells the asker of a command that held others up past
/// the bound: end it. It keeps its lane until it leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// The bound it reached, in seconds.
    pub bound: u64,
    /// How many it held up.
    pub waiting: usize,
}

/// A host's exclusive queue, as one process holds it: commands run in the
/// lanes the host says, at most one of each repo in each lane. Requests keep
/// their order, and each leaves as its asker leaves. A clone is the same.
pub struct Holder<N, H> {
    held: Arc<(Mutex<Joined<N>>, Condvar)>,
    host: Arc<H>,
}

impl<N, H> Clone for Holder<N, H> {
    fn clone(&self) -> Self {
        Holder { held: self.held.clone(), host: self.host.clone() }
    }
}

/// Every ticket in the queue, waiting or running, and the number the next
/// to join takes.
struct Joined<N> {
    tickets: Vec<Ticket<N>>,
    next: u64,
    /// A move to another holder lets running commands end before any other
    /// starts.
    paused: bool,
    closed: bool,
    /// How long each running ticket has held others up.
    waited: BTreeMap<u64, Duration>,
    counted_at: Instant,
}

impl<N> Joined<N> {
    fn new() -> Self {
        Joined {
            tickets: vec![],
            next: 0,
            paused: false,
            closed: false,
            waited: BTreeMap::new(),
            counted_at: Instant::now(),
        }
    }

    /// The waiters the running command `number` actually holds up: a free
    /// lane means another repo need not wait, and a repo's second turn waits
    /// only for its own first turn.
    fn waiting_for(&self, number: u64, lanes: Lanes) -> usize {
        let Some(running) = self.tickets.iter().find(|t| t.number == number && t.running()) else {
            return 0;
        };
        let limit = limit(running, lanes);
        let occupied =
            self.tickets.iter().filter(|t| t.running() && same_lane(t, running)).count() as u64;
        self.tickets
            .iter()
            .filter(|waiting| {
                if waiting.running()
                    || !(same_lane(running, waiting)
                        || (running.is_turn && waiting.is_turn && same_repo(running, waiting)))
                {
                    return false;
                }
                let owner = self.tickets.iter().find(|t| {
                    t.running()
                        && same_repo(t, waiting)
                        && (same_lane(t, waiting) || (t.is_turn && waiting.is_turn))
                        && !(t.candidate && waiting.candidate)
                        && !(is_fast(t) && is_fast(waiting))
                });
                match owner {
                    Some(owner) => owner.number == number,
                    None => occupied >= limit,
                }
            })
            .count()
    }

    /// Accounts for the interval before any queue change, so short waits,
    /// gaps without waiters and a changing set of waiters count correctly.
    fn count(&mut self, now: Instant, lanes: Lanes) {
        let since = now.saturating_duration_since(self.counted_at);
        let holding: Vec<u64> = self
            .tickets
            .iter()
            .filter(|ticket| ticket.running())
            .map(|ticket| ticket.number)
            .filter(|number| self.waiting_for(*number, lanes) > 0)
            .collect();
        for number in holding {
            let waited = self.waited.entry(number).or_default();
            *waited = waited.saturating_add(since);
        }
        self.counted_at = now;
    }
}

impl<N: Clone, H: Host<N>> Holder<N, H> {
    /// An empty queue, held for `host`.
    pub fn new(host: H) -> Self {
        Holder { held: Arc::new((Mutex::new(Joined::new()), Condvar::new())), host: Arc::new(host) }
    }

    /// The host it is held for.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// Joins `ticket` to the queue → the number it is known by there, and how
    /// many are ahead of it in its lane.
    pub fn join(&self, ticket: Ticket<N>) -> (u64, usize) {
        let lanes = self.host.lanes();
        let joined = {
            let mut joined = self.joined();
            joined.count(Instant::now(), lanes);
            joined.next = joined.next.saturating_add(1);
            let number = joined.next;
            joined.tickets.push(Ticket { number, ran: None, ..ticket });
            (number, ahead(&joined.tickets, number).unwrap_or(0))
        };
        self.held.1.notify_all();
        joined
    }

    /// Waits for the turn of the ticket `number`: until no ticket ahead of it
    /// holds or reserves a place it needs, while grants are not paused, and
    /// its host lets it be granted. Its command runs from then, until it
    /// leaves.
    pub fn turn(&self, number: u64) -> Turn {
        let (_, turned) = &*self.held;
        let mut joined = self.joined();
        loop {
            if joined.closed {
                return Turn::Closed;
            }
            if ahead(&joined.tickets, number).is_none() {
                return Turn::Left;
            }
            if !joined.paused {
                let eligible: Vec<Ticket<N>> = self
                    .with_beside(&joined)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| t.running() || self.host.grantable(t))
                    .collect();
                if ready(&eligible, number, self.host.lanes()) {
                    break;
                }
            }
            joined =
                turned.wait_timeout(joined, CHECK_EVERY).unwrap_or_else(PoisonError::into_inner).0;
        }
        let ran = self.host.now();
        joined.count(Instant::now(), self.host.lanes());
        joined.waited.insert(number, Duration::ZERO);
        joined.counted_at = Instant::now();
        for ticket in joined.tickets.iter_mut().filter(|ticket| ticket.number == number) {
            ticket.ran = Some(ran);
        }
        Turn::Runs(ran)
    }

    /// Waits while the running ticket `number` is in the queue → the bound
    /// it reached, once it held others up longer than the host's bound;
    /// `None` once it left first. It keeps its lane either way, until it
    /// leaves: the next command is not given its turn before what it ran
    /// is ended.
    pub fn overdue(&self, number: u64) -> Option<Bound> {
        let (_, turned) = &*self.held;
        let mut joined = self.joined();
        loop {
            if !joined.tickets.iter().any(|ticket| ticket.number == number) {
                return None;
            }
            let lanes = self.host.lanes();
            joined.count(Instant::now(), lanes);
            let bound = self.host.bound();
            let waiting = joined.waiting_for(number, lanes);
            if waiting > 0
                && joined.waited.get(&number).copied().unwrap_or_default()
                    >= Duration::from_secs(bound)
            {
                return Some(Bound { bound, waiting });
            }
            joined =
                turned.wait_timeout(joined, CHECK_EVERY).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Takes the ticket `number` out of the queue, where its host lets it
    /// leave: the next runs, if it was the one that ran.
    pub fn leave(&self, number: u64) {
        let leaving = self.joined().tickets.iter().find(|t| t.number == number).cloned();
        if let Some(ticket) = leaving
            && !self.host.leaving(&ticket)
        {
            return;
        }
        let lanes = self.host.lanes();
        let mut joined = self.joined();
        joined.count(Instant::now(), lanes);
        joined.tickets.retain(|ticket| ticket.number != number);
        joined.waited.remove(&number);
        drop(joined);
        self.held.1.notify_all();
    }

    /// Runs `work` in the lane of `ticket`, once its turn came, and leaves →
    /// what `work` returned, given when its turn came; `None` where its turn
    /// never came: the holder was closed. What reaches the bound is not
    /// ended here: `overdue` says so, to whoever watches it.
    pub fn alone<T>(&self, ticket: Ticket<N>, work: impl FnOnce(u64) -> T) -> Option<T> {
        let (number, _) = self.join(ticket);
        let done = match self.turn(number) {
            Turn::Runs(ran) => Some(work(ran)),
            Turn::Left | Turn::Closed => None,
        };
        self.leave(number);
        done
    }

    /// Changes what is noted of each ticket `which` picks, by `change`. No
    /// rule of the queue reads a note.
    pub fn note(&self, which: impl Fn(&Ticket<N>) -> bool, change: impl Fn(&mut N)) {
        let mut joined = self.joined();
        for ticket in joined.tickets.iter_mut().filter(|ticket| which(ticket)) {
            change(&mut ticket.note);
        }
    }

    /// Sends waiting commands to the next holder: none is granted here any
    /// more. None is running when a move to another holder calls this;
    /// closing also works when that move fails.
    pub fn close(&self) {
        self.joined().closed = true;
        self.held.1.notify_all();
    }

    /// Pauses new grants atomically with reading those already running →
    /// how many run, beside the queue too. Waiting commands rejoin the next
    /// holder with their original ask time. One that cannot be read counts.
    pub fn pause(&self) -> usize {
        let mut joined = self.joined();
        joined.paused = true;
        self.with_beside(&joined)
            .map_or(1, |tickets| tickets.iter().filter(|ticket| ticket.running()).count())
    }

    /// Lets commands start again, where the move to another holder did not
    /// happen.
    pub fn resume(&self) {
        self.joined().paused = false;
        self.held.1.notify_all();
    }

    /// The tickets, beside the queue too, in the order they run, and whether
    /// new grants are paused — under the same lock. Errors: those beside it
    /// cannot be read.
    pub fn observation(&self) -> Result<(Vec<Ticket<N>>, bool), String> {
        let joined = self.joined();
        Ok((in_order(self.with_beside(&joined)?), joined.paused || joined.closed))
    }

    /// The queue's own tickets and those `Host::beside` it.
    fn with_beside(&self, joined: &Joined<N>) -> Result<Vec<Ticket<N>>, String> {
        let mut tickets = joined.tickets.clone();
        for beside in self.host.beside()? {
            if !tickets.iter().any(|t| t.pid == beside.pid && t.running()) {
                tickets.push(beside);
            }
        }
        Ok(tickets)
    }

    fn joined(&self) -> MutexGuard<'_, Joined<N>> {
        self.held.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclusive::Plain;

    fn ticket(command: &str, asked: u64) -> Ticket {
        Ticket {
            number: 0,
            pid: 1,
            command: command.into(),
            dir: "/w/a".into(),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            gate: None,
            asked,
            ran: None,
            note: Plain::default(),
        }
    }

    const TWO: Lanes = Lanes { worker: 2, fast: 1, slow: 1 };

    #[test]
    fn contention_pauses_without_waiters_and_changing_waiters_does_not_reset_it() {
        let start = Instant::now();
        let mut running = ticket("running", 0);
        running.ran = Some(0);
        let mut joined = Joined { tickets: vec![running], counted_at: start, ..Joined::new() };
        let one = Lanes { worker: 1, fast: 1, slow: 1 };
        joined.count(start + Duration::from_secs(20), one);
        assert_eq!(
            joined.waited.get(&0).copied().unwrap_or_default(),
            Duration::ZERO,
            "time alone does not count"
        );
        joined.tickets.push(ticket("waiting", 20));
        joined.count(start + Duration::from_secs(23), one);
        assert_eq!(joined.waited.get(&0).copied().unwrap_or_default(), Duration::from_secs(3));
        joined.tickets.pop();
        joined.count(start + Duration::from_secs(50), one);
        assert_eq!(
            joined.waited.get(&0).copied().unwrap_or_default(),
            Duration::from_secs(3),
            "nobody waits in the gap"
        );
        joined.tickets.push(ticket("another waiter", 50));
        joined.count(start + Duration::from_secs(54), one);
        assert_eq!(
            joined.waited.get(&0).copied().unwrap_or_default(),
            Duration::from_secs(7),
            "another waiter resumes the count"
        );
    }

    #[test]
    fn a_free_lane_is_no_contention_and_a_repos_second_turn_waits_only_for_its_own() {
        let mut a = ticket("a", 100);
        a.number = 1;
        a.repo = Some("a".into());
        a.ran = Some(100);
        let mut b = ticket("b", 101);
        b.number = 2;
        b.repo = Some("b".into());
        let mut joined = Joined { tickets: vec![a.clone(), b.clone()], ..Joined::new() };
        assert_eq!(joined.waiting_for(1, TWO), 0, "b has a free lane, even at a zero bound");
        b.ran = Some(101);
        let mut another_a = a;
        another_a.number = 3;
        another_a.ran = None;
        joined.tickets = vec![joined.tickets[0].clone(), b, another_a];
        assert_eq!(joined.waiting_for(1, TWO), 1);
        assert_eq!(joined.waiting_for(2, TWO), 0, "b does not hold a's second turn up");
    }
}

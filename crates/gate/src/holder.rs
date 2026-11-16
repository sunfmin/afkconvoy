//! The holder of a host's exclusive queue (docs/adr/0019, docs/adr/0023): the
//! one process of a host that keeps every ticket waiting or running, grants
//! each its lane in the order `exclusive` decides, and ends the hold of a
//! command that held others up past the host's bound (docs/adr/0031). A
//! command starts once the host has room for it (docs/adr/0087).
//!
//! A host process embeds it — afkconvoy's host runner, or another program's
//! daemon — and says through `Host` what only it knows: the time, how busy
//! the host is and the share a start stays under, what each kind of command
//! takes of it, the bound, the tickets it holds outside this queue, and what must
//! be done before a ticket leaves. How an asker reaches the holder, and is
//! told its turn, is the embedder's: each asker's thread `join`s, waits for
//! its `turn`, and `leave`s once its command ended or its line closed.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::exclusive::{
    BUSY_SHARE, ConstraintKind, Halt, Low, Reading, Retained, Room, State, Ticket, ahead, in_order,
    lag, ready, same_repo, scheduling,
};

/// How often a waiting turn looks again, besides being woken by a change.
const CHECK_EVERY: Duration = Duration::from_millis(100);

/// What only the process that embeds the holder knows.
pub trait Host<N>: Send + Sync {
    /// The time, in epoch seconds: when a ticket's command begins to run.
    fn now(&self) -> u64;

    /// How busy the host is now, over the last minute (docs/adr/0087): its
    /// cores by the kernel's CPU ticks, or by the load alone where those
    /// cannot be read, and its memory where it is under its floor. `None`: it
    /// cannot be read, and a command starts only while nothing of the queue
    /// runs. A host may say the same reading again until it takes the next.
    fn reading(&self) -> Option<Reading> {
        None
    }

    /// The busy share of the cores a command starts under, in percent, now:
    /// a changed setting applies to the next grant.
    fn share(&self) -> u64 {
        BUSY_SHARE
    }

    /// The CPU, in percent of the host's, the kind of command `ticket` runs
    /// took in its recent runs: what its start counts as until the reading
    /// includes it. `None`: no run of it is kept (`exclusive::UNMEASURED`).
    fn takes(&self, ticket: &Ticket<N>) -> Option<u64> {
        let _ = ticket;
        None
    }

    /// How long, in seconds, a running command may hold others up before its
    /// asker is told to end it, now: a changed setting applies to the command
    /// already running.
    fn bound(&self) -> u64;

    /// Tickets running outside this queue on the same host
    /// — a lease a command's descendants inherit, say — to be counted
    /// beside its own. One of them that this queue already runs is not
    /// counted twice. Errors: they cannot be read.
    fn beside(&self) -> Result<Vec<Ticket<N>>, String> {
        Ok(vec![])
    }

    /// Whether a waiting `ticket` may be granted at all, beside what the
    /// host's room allows: its repo is free to be worked in, say.
    fn grantable(&self, ticket: &Ticket<N>) -> bool {
        let _ = ticket;
        true
    }

    /// The disk the host builds on, where it is under the floor its builds
    /// need kept free, now: no new command is granted while it is. `None`:
    /// it is not, or the host does not say.
    fn disk(&self) -> Option<Low> {
        None
    }

    /// What is shown of `ticket` of this queue, as an observation reads it:
    /// its note as its host keeps it now. No rule of the queue reads a note.
    fn shown(&self, ticket: &mut Ticket<N>) {
        let _ = ticket;
    }

    /// Done as `ticket` leaves the queue. Errors: what it ran could not be
    /// confirmed ended, so it keeps its lane, `retained` with this error,
    /// and its leaving is tried again every `retry_every` — its asker is
    /// gone and does not leave again.
    fn leaving(&self, ticket: &Ticket<N>) -> Result<(), String> {
        let _ = ticket;
        Ok(())
    }

    /// How long a retained ticket waits before its leaving is tried again.
    fn retry_every(&self) -> Duration {
        Duration::from_secs(60)
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
/// room the host has (docs/adr/0087), held apart by repo. Requests keep
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
    /// When each retained ticket's leaving was last tried.
    tried: BTreeMap<u64, Instant>,
    counted_at: Instant,
    /// The reading the last command started on: the next starts on another.
    started_on: Option<Reading>,
}

impl<N> Joined<N> {
    fn new() -> Self {
        Joined {
            tickets: vec![],
            next: 0,
            paused: false,
            closed: false,
            waited: BTreeMap::new(),
            tried: BTreeMap::new(),
            counted_at: Instant::now(),
            started_on: None,
        }
    }

    /// The waiters the running command `number` actually holds up, as
    /// `room` schedules them: a repo's second turn waits only for its own
    /// first turn, and one held by the host's room is held by every command
    /// of the queue the room counts. Room enough means none need wait.
    fn waiting_for(&self, number: u64, room: Room) -> usize {
        if !self.tickets.iter().any(|ticket| ticket.number == number && ticket.running()) {
            return 0;
        }
        let mut held: usize = 0;
        for decision in scheduling(&self.tickets, room, Halt::default()) {
            if decision.state != State::Waiting {
                continue;
            }
            let by = |kinds: &dyn Fn(&ConstraintKind) -> bool| {
                decision
                    .reasons
                    .iter()
                    .filter(|reason| kinds(&reason.kind))
                    .flat_map(|reason| &reason.requests)
                    .filter(|holder| holder.state != State::Eligible)
                    .map(|holder| holder.number)
                    .collect::<Vec<u64>>()
            };
            let owners = by(&|kind| matches!(kind, ConstraintKind::SameRepo));
            let holders = if owners.is_empty() {
                by(&|kind| !matches!(kind, ConstraintKind::Paused | ConstraintKind::Disk))
            } else {
                owners
            };
            if holders.contains(&number) {
                held = held.saturating_add(1);
            }
        }
        held
    }

    /// Accounts for the interval before any queue change, so short waits,
    /// gaps without waiters and a changing set of waiters count correctly.
    fn count(&mut self, now: Instant, room: Room) {
        let since = now.saturating_duration_since(self.counted_at);
        let mut holding: Vec<u64> = vec![];
        for ticket in &self.tickets {
            if ticket.running() && self.waiting_for(ticket.number, room) > 0 {
                holding.push(ticket.number);
            }
        }
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
        let joined = {
            let mut joined = self.joined();
            let room = self.own_room(&joined);
            joined.count(Instant::now(), room);
            joined.next = joined.next.saturating_add(1);
            let number = joined.next;
            let mut ticket = ticket;
            ticket.number = number;
            ticket.ran = None;
            joined.tickets.push(ticket);
            (number, ahead(&joined.tickets, number).unwrap_or(0))
        };
        self.held.1.notify_all();
        joined
    }

    /// Waits for the turn of the ticket `number`: until the host has room
    /// for it and no ticket ahead of it holds or reserves what it needs,
    /// while grants are not paused, and its host lets it be granted. Its
    /// command runs from then, until it leaves.
    pub fn turn(&self, number: u64) -> Turn {
        let (_, turned) = &*self.held;
        let mut joined = self.joined();
        let started_on = loop {
            if joined.closed {
                return Turn::Closed;
            }
            if ahead(&joined.tickets, number).is_none() {
                return Turn::Left;
            }
            // A retained ticket may leave now: its lane could be this one's.
            if self.due(&joined) {
                drop(joined);
                self.retry();
                joined = self.joined();
            }
            if !joined.paused && self.host.disk().is_none() {
                let eligible: Vec<Ticket<N>> = self
                    .with_beside(&joined)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| t.running() || self.host.grantable(t))
                    .collect();
                let room = self.room(&joined, &eligible);
                if ready(&eligible, number, room) {
                    break room.reading;
                }
            }
            joined =
                turned.wait_timeout(joined, CHECK_EVERY).unwrap_or_else(PoisonError::into_inner).0;
        };
        let ran = self.host.now();
        let room = self.own_room(&joined);
        joined.count(Instant::now(), room);
        joined.started_on = started_on;
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
            let room = self.own_room(&joined);
            joined.count(Instant::now(), room);
            let bound = self.host.bound();
            let waiting = joined.waiting_for(number, room);
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
    /// leave: the next runs, if it was the one that ran. Where its host
    /// cannot confirm it may, it stays `retained` in its lane, and is tried
    /// again (`retry`) without its asker.
    pub fn leave(&self, number: u64) {
        let leaving = self.joined().tickets.iter().find(|t| t.number == number).cloned();
        if let Some(ticket) = leaving
            && let Err(error) = self.host.leaving(&ticket)
        {
            let now = self.host.now();
            let mut joined = self.joined();
            joined.tried.insert(number, Instant::now());
            for ticket in joined.tickets.iter_mut().filter(|ticket| ticket.number == number) {
                let since = ticket.retained.as_ref().map_or(now, |retained| retained.since);
                ticket.retained = Some(Retained { since, error: error.clone() });
            }
            return;
        }
        let mut joined = self.joined();
        let room = self.own_room(&joined);
        joined.count(Instant::now(), room);
        joined.tickets.retain(|ticket| ticket.number != number);
        joined.waited.remove(&number);
        joined.tried.remove(&number);
        drop(joined);
        self.held.1.notify_all();
    }

    /// Tries again the leaving of each retained ticket last tried
    /// `retry_every` ago or longer: one that its host now lets leave frees
    /// its lane. Each is tried by one caller at a time. The embedder calls
    /// this on its own bound; a waiting turn does too.
    pub fn retry(&self) {
        let due: Vec<u64> = {
            let mut joined = self.joined();
            let due = self.due_of(&joined);
            for number in &due {
                joined.tried.insert(*number, Instant::now());
            }
            due
        };
        for number in due {
            self.leave(number);
        }
    }

    /// Whether a retained ticket's leaving is due to be tried again.
    fn due(&self, joined: &Joined<N>) -> bool {
        !self.due_of(joined).is_empty()
    }

    fn due_of(&self, joined: &Joined<N>) -> Vec<u64> {
        let every = self.host.retry_every();
        let mut due = vec![];
        for ticket in &joined.tickets {
            let tried = joined.tried.get(&ticket.number);
            if ticket.retained.is_some() && tried.is_none_or(|tried| tried.elapsed() >= every) {
                due.push(ticket.number);
            }
        }
        due
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

    /// Grants `ticket` its lane at once, where it may be granted now and no
    /// other ticket of its repo, of this queue or beside it, runs or waits →
    /// the number it runs as, until it `leave`s; `None`: it was not joined.
    /// What holds its whole repo (`Ticket::whole_repo`) is asked so, at a
    /// moment nothing of its repo runs, and never waits for one.
    pub fn at_once(&self, ticket: Ticket<N>) -> Option<u64> {
        let mut joined = self.joined();
        if joined.paused || joined.closed || !self.host.grantable(&ticket) {
            return None;
        }
        let standing = self.with_beside(&joined).ok()?;
        if standing.iter().any(|other| same_repo(other, &ticket)) {
            return None;
        }
        let mut ticket = ticket;
        ticket.number = joined.next.saturating_add(1);
        ticket.ran = None;
        let mut asking = standing;
        asking.push(ticket.clone());
        let room = self.room(&joined, &asking);
        if !ready(&asking, ticket.number, room) {
            return None;
        }
        let own = self.own_room(&joined);
        joined.count(Instant::now(), own);
        joined.started_on = room.reading;
        joined.next = ticket.number;
        ticket.ran = Some(self.host.now());
        joined.waited.insert(ticket.number, Duration::ZERO);
        let number = ticket.number;
        joined.tickets.push(ticket);
        drop(joined);
        self.held.1.notify_all();
        Some(number)
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
        let mut tickets = joined.tickets.clone();
        for ticket in &mut tickets {
            self.host.shown(ticket);
        }
        Ok((in_order(self.besides(tickets)?), joined.paused || joined.closed))
    }

    /// What the queue starts commands by now, beside the queue too
    /// (docs/adr/0087): the host's reading, the share, what the reading does
    /// not include yet, and whether a command already started on it. Those
    /// beside it that cannot be read are left out.
    pub fn room_now(&self) -> Room {
        let joined = self.joined();
        let tickets = self.with_beside(&joined).unwrap_or_else(|_| joined.tickets.clone());
        self.room(&joined, &tickets)
    }

    /// The room `tickets`, the queue's as `joined` holds it, leave.
    fn room(&self, joined: &Joined<N>, tickets: &[Ticket<N>]) -> Room {
        let share = self.host.share();
        let Some(reading) = self.host.reading() else { return Room::unread(share) };
        Room {
            reading: Some(reading),
            share,
            counted: lag(tickets, &reading, |ticket| self.host.takes(ticket)),
            spent: joined.started_on == Some(reading),
        }
    }

    /// The room of the queue's own tickets: what the bound counts by.
    fn own_room(&self, joined: &Joined<N>) -> Room {
        self.room(joined, &joined.tickets)
    }

    /// The queue's own tickets and those `Host::beside` it.
    fn with_beside(&self, joined: &Joined<N>) -> Result<Vec<Ticket<N>>, String> {
        self.besides(joined.tickets.clone())
    }

    /// `tickets`, and those `Host::beside` them.
    fn besides(&self, mut tickets: Vec<Ticket<N>>) -> Result<Vec<Ticket<N>>, String> {
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
            whole_repo: false,
            gate: None,
            asked,
            ran: None,
            retained: None,
            note: Plain::default(),
        }
    }

    /// A host that cannot be read, so one command runs at a time, whose
    /// cleanup fails as often as `fails` says, then passes; it counts each
    /// leaving it is asked.
    struct Failing {
        fails: Mutex<u32>,
        asked: Mutex<u32>,
    }

    impl Host<Plain> for Failing {
        fn now(&self) -> u64 {
            1_000
        }
        fn bound(&self) -> u64 {
            3_600
        }
        fn leaving(&self, _: &Ticket) -> Result<(), String> {
            *self.asked.lock().unwrap() += 1;
            let mut fails = self.fails.lock().unwrap();
            if *fails == 0 {
                return Ok(());
            }
            *fails -= 1;
            Err("git commit-tree: No space left on device".into())
        }
        fn retry_every(&self) -> Duration {
            Duration::ZERO
        }
    }

    /// A lane retained for a failed cleanup is a retry, not a life sentence
    /// (sunfmin/control#1122): once its cleanup passes, the lane is freed
    /// and the next waiter granted, though its asker never leaves again.
    #[test]
    fn a_retained_lane_is_freed_once_its_cleanup_passes_without_its_asker() {
        let holder = Holder::new(Failing { fails: Mutex::new(1), asked: Mutex::new(0) });
        let (first, _) = holder.join(ticket("first", 1));
        assert!(matches!(holder.turn(first), Turn::Runs(_)));
        holder.leave(first);
        let (standing, _) = holder.observation().unwrap();
        let retained = standing[0].retained.clone().expect("its failed cleanup retains it");
        assert_eq!(
            (standing[0].number, retained.since, retained.error.as_str()),
            (first, 1_000, "git commit-tree: No space left on device")
        );
        assert!(standing[0].running(), "it keeps its lane");

        let (next, _) = holder.join(ticket("next", 2));
        let waiter = {
            let holder = holder.clone();
            std::thread::spawn(move || holder.turn(next))
        };
        assert!(matches!(waiter.join().unwrap(), Turn::Runs(_)), "the next waiter is granted");
        let (standing, _) = holder.observation().unwrap();
        assert_eq!(standing.iter().map(|t| t.number).collect::<Vec<_>>(), vec![next]);
        assert_eq!(*holder.host().asked.lock().unwrap(), 2, "tried once more, without its asker");
    }

    /// With nobody waiting, the embedder's own `retry` frees it; a cleanup
    /// that fails again keeps the error it last said, from when it first failed.
    #[test]
    fn retry_keeps_a_failing_cleanup_retained_since_it_first_failed_then_frees_it() {
        let holder = Holder::new(Failing { fails: Mutex::new(2), asked: Mutex::new(0) });
        let (first, _) = holder.join(ticket("first", 1));
        assert!(matches!(holder.turn(first), Turn::Runs(_)));
        holder.leave(first);
        holder.retry();
        let (standing, _) = holder.observation().unwrap();
        assert_eq!(standing[0].retained.as_ref().map(|r| r.since), Some(1_000));
        holder.retry();
        assert!(holder.observation().unwrap().0.is_empty(), "the lane is freed");
        assert_eq!(*holder.host().asked.lock().unwrap(), 3);
    }

    /// The host's cores a tenth busy: room for more.
    fn roomy() -> Room {
        let reading = crate::exclusive::Reading {
            busy: 10,
            by: crate::exclusive::By::Ticks,
            since: 0,
            taken: 60,
            memory: None,
        };
        Room { reading: Some(reading), share: BUSY_SHARE, counted: 0, spent: false }
    }

    #[test]
    fn contention_pauses_without_waiters_and_changing_waiters_does_not_reset_it() {
        let start = Instant::now();
        let mut running = ticket("running", 0);
        running.ran = Some(0);
        let mut joined = Joined { tickets: vec![running], counted_at: start, ..Joined::new() };
        let one = Room::unread(BUSY_SHARE);
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
    fn room_is_no_contention_and_a_repos_second_turn_waits_only_for_its_own() {
        let mut a = ticket("a", 100);
        a.number = 1;
        a.repo = Some("a".into());
        a.ran = Some(100);
        let mut b = ticket("b", 101);
        b.number = 2;
        b.repo = Some("b".into());
        let mut joined = Joined { tickets: vec![a.clone(), b.clone()], ..Joined::new() };
        assert_eq!(
            joined.waiting_for(1, roomy()),
            0,
            "the host has room for b, even at a zero bound"
        );
        b.ran = Some(101);
        let mut another_a = a;
        another_a.number = 3;
        another_a.ran = None;
        joined.tickets = vec![joined.tickets[0].clone(), b, another_a];
        assert_eq!(joined.waiting_for(1, roomy()), 1);
        assert_eq!(joined.waiting_for(2, roomy()), 0, "b does not hold a's second turn up");
    }
}

#[cfg(test)]
mod disk_tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::exclusive::Plain;

    /// A host whose disk is under its floor while `low` says so.
    struct Filling {
        low: AtomicBool,
    }

    impl Host<Plain> for Filling {
        fn now(&self) -> u64 {
            100
        }
        fn bound(&self) -> u64 {
            3600
        }
        fn disk(&self) -> Option<Low> {
            self.low.load(Ordering::SeqCst).then_some(Low { free: 1, floor: 2 })
        }
    }

    #[test]
    fn a_disk_under_its_floor_grants_nothing_new_until_space_returns() {
        let holder = Holder::new(Filling { low: AtomicBool::new(true) });
        let ticket = Ticket {
            number: 0,
            pid: 1,
            command: "gate".into(),
            dir: "/w/a".into(),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            whole_repo: false,
            gate: None,
            asked: 100,
            ran: None,
            retained: None,
            note: Plain::default(),
        };
        let (number, ahead) = holder.join(ticket);
        assert_eq!(ahead, 0, "nothing is ahead of it");
        let waiting = holder.clone();
        let turn = std::thread::spawn(move || waiting.turn(number));
        std::thread::sleep(CHECK_EVERY * 3);
        assert!(!turn.is_finished(), "no grant while the disk is under its floor");
        holder.host().low.store(false, Ordering::SeqCst);
        assert_eq!(turn.join().unwrap(), Turn::Runs(100), "granted once space returned");
    }
}

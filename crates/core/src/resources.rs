//! Resource conditions that call for the host's watch, once an episode.

use serde::{Deserialize, Serialize};

use crate::admission::Hold;

/// How long, in seconds, admission holds back for one reason before the host
/// calls for a watch, where the host says no span of its own.
pub const SPAN: u64 = 600;

/// How long, in seconds, admission admits before an episode ends: holding
/// back after that is a new one, and the host calls for a new watch.
pub const ADMITTED_FOR: u64 = 300;

/// What admission held new workers back for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Load,
    Memory,
    Disk,
}

impl Reason {
    /// The reason `hold` holds back for.
    pub fn of(hold: &Hold) -> Reason {
        match hold {
            Hold::Load { .. } => Reason::Load,
            Hold::Memory { .. } => Reason::Memory,
            Hold::Disk { .. } => Reason::Disk,
        }
    }

    /// How the log, the panel and `doctor` say it.
    pub fn word(self) -> &'static str {
        match self {
            Reason::Load => "load",
            Reason::Memory => "memory",
            Reason::Disk => "disk",
        }
    }

    pub fn read(word: &str) -> Option<Reason> {
        [Reason::Load, Reason::Memory, Reason::Disk]
            .into_iter()
            .find(|reason| reason.word() == word)
    }
}

/// What a `run` keeps of admission, tick after tick: since when it has held
/// back for each reason without a break, and the episode those make.
///
/// An episode starts at the first tick that holds back after the last one
/// ended, and ends once admission has admitted for `ADMITTED_FOR`: a tick
/// that admits in between breaks every reason's run, but not the episode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holding {
    /// Each reason held back for at every tick since then.
    held: Vec<(Reason, u64)>,
    /// When the episode started; `None` while there is none.
    episode: Option<u64>,
    /// When admission last started admitting, while the episode stands.
    admitted: Option<u64>,
}

/// A host watch due: for `reason`, held back for since `since`, in the
/// episode that started at `episode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Due {
    pub reason: Reason,
    pub since: u64,
    pub episode: u64,
}

impl Holding {
    /// What a tick at `now` found: admission holding back for `reasons`, or —
    /// none — admitting.
    pub fn ticked(&mut self, now: u64, reasons: &[Reason]) {
        if reasons.is_empty() {
            self.held.clear();
            if self.episode.is_none() {
                return;
            }
            let admitted = *self.admitted.get_or_insert(now);
            if now.saturating_sub(admitted) >= ADMITTED_FOR {
                *self = Holding::default();
            }
            return;
        }
        self.admitted = None;
        if self.episode.is_none() {
            self.episode = Some(now);
        }
        self.held.retain(|(reason, _)| reasons.contains(reason));
        for reason in reasons {
            if !self.held.iter().any(|(held, _)| held == reason) {
                self.held.push((*reason, now));
            }
        }
    }

    /// The host watch due at `now` under a span of `span` seconds: for
    /// the reason held back for longest, once that is `span` or more.
    /// Episode coverage belongs to the host watch.
    pub fn due(&self, now: u64, span: u64) -> Option<Due> {
        let episode = self.episode?;
        let mut due: Option<Due> = None;
        for (reason, since) in &self.held {
            let long = now.saturating_sub(*since) >= span;
            if long && due.is_none_or(|due| *since < due.since) {
                due = Some(Due { reason: *reason, since: *since, episode });
            }
        }
        due
    }

    /// When the episode started; `None` while admission admits.
    pub fn episode(&self) -> Option<u64> {
        self.episode
    }
}

/// The user account holding the most resident memory on the machine, where
/// it is not this one: memory pressure it causes is beyond every watch's
/// bounds, which keep it to this account's fleet-owned processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elsewhere {
    pub account: String,
    pub uid: u32,
    pub memory_bytes: u64,
    pub processes: usize,
}

/// The lowest uid of a user account, where the machine's own accounts end.
pub const FIRST_USER_UID: u32 = 500;
/// Past this, the uids of the machine's nobody accounts.
pub const LAST_USER_UID: u32 = 60_000;

/// Of `accounts` — each account's uid and the resident bytes its processes
/// hold, an account once — the uid of the user account other than `me` that
/// holds the most, where it holds more than `me` does: memory pressure then
/// is that account's. Accounts below `FIRST_USER_UID` or past
/// `LAST_USER_UID` are the machine's own and are never it.
pub fn elsewhere(accounts: &[(u32, u64)], me: u32) -> Option<u32> {
    let mut most: (Option<u32>, u64) = (None, 0);
    for (uid, bytes) in accounts {
        if *uid == me && *bytes > most.1 {
            most = (None, *bytes);
        }
    }
    for (uid, bytes) in accounts {
        if *uid != me && is_user(*uid) && *bytes > most.1 {
            most = (Some(*uid), *bytes);
        }
    }
    most.0
}

// Flux cannot check a range's `contains`.
#[allow(clippy::manual_range_contains)]
fn is_user(uid: u32) -> bool {
    FIRST_USER_UID <= uid && uid <= LAST_USER_UID
}

/// Why a memory-pressure episode starts no watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unwatched {
    /// Another account holds the most memory, and this host's fleets hold no
    /// worker: nothing a watch may touch holds any.
    NoWorker,
    /// A watch already reported pressure from that same account and left it
    /// with the operator, who has not answered it since.
    LeftWithOperator,
}

/// Whether memory pressure whose largest holder is the account `elsewhere`
/// (by uid; `None` where it is this account's, or unknown), while this
/// host's fleets hold `workers` workers, starts no watch — and why: once
/// `left`, the account a reported watch left such pressure with the operator
/// for, it is that account's again.
pub fn unwatched(elsewhere: Option<u32>, workers: usize, left: Option<u32>) -> Option<Unwatched> {
    let uid = elsewhere?;
    if workers == 0 {
        Some(Unwatched::NoWorker)
    } else if left == Some(uid) {
        Some(Unwatched::LeftWithOperator)
    } else {
        None
    }
}

/// Whether what a watch left with the operator for `left` still stands once
/// a look finds the pressure's largest holder `elsewhere`: not once it is
/// another account, or this account's fleet again.
pub fn still_left(left: u32, elsewhere: Option<u32>) -> bool {
    elsewhere == Some(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_reason_must_hold_for_the_whole_span() {
        let mut holding = Holding::default();
        holding.ticked(0, &[Reason::Load]);
        assert!(holding.due(SPAN - 1, SPAN).is_none());
        assert_eq!(
            holding.due(SPAN, SPAN),
            Some(Due { reason: Reason::Load, since: 0, episode: 0 })
        );
        holding.ticked(601, &[Reason::Memory]);
        holding.ticked(900, &[Reason::Disk]);
        assert!(holding.due(1200, SPAN).is_none());
        holding.ticked(1500, &[Reason::Disk]);
        assert_eq!(holding.due(1500, SPAN).unwrap().since, 900);
    }

    #[test]
    fn recovery_cancels_a_waiting_trigger_but_keeps_coverage_for_five_minutes() {
        let mut holding = Holding::default();
        holding.ticked(0, &[Reason::Memory]);
        assert!(holding.due(600, SPAN).is_some());
        holding.ticked(601, &[]);
        assert!(holding.due(601, SPAN).is_none());
        assert_eq!(holding.episode(), Some(0));
        holding.ticked(700, &[Reason::Memory]);
        assert_eq!(holding.episode(), Some(0));
        holding.ticked(800, &[]);
        holding.ticked(1100, &[]);
        assert_eq!(holding.episode(), None);
        holding.ticked(1200, &[Reason::Memory]);
        assert_eq!(holding.due(1800, SPAN).unwrap().episode, 1200);
    }

    #[test]
    fn pressure_and_admission_have_independent_persistent_episodes() {
        let (mut admission, mut pressure) = (Holding::default(), Holding::default());
        admission.ticked(0, &[Reason::Load]);
        pressure.ticked(100, &[Reason::Memory]);
        pressure.ticked(700, &[]);
        pressure.ticked(1000, &[]);
        assert!(pressure.episode().is_none());
        assert_eq!(admission.due(1000, SPAN).unwrap().episode, 0);
        let saved = serde_json::to_string(&admission).unwrap();
        let restored: Holding = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored, admission);
    }

    #[test]
    fn another_user_account_holding_more_than_this_one_is_where_pressure_comes_from() {
        // 20 GiB of another account's VM and simulator against this one's 2.
        let gib = 1u64 << 30;
        let table = [(0, 30 * gib), (88, 3 * gib), (501, 2 * gib), (502, 20 * gib), (503, gib)];
        assert_eq!(elsewhere(&table, 501), Some(502));
        assert_eq!(elsewhere(&table, 502), None, "this account holds the most");
        let fleet_largest = [(501, 9 * gib), (502, 8 * gib)];
        assert_eq!(elsewhere(&fleet_largest, 501), None);
        assert_eq!(elsewhere(&[(0, gib), (4_294_967_294, gib)], 501), None, "machine accounts");
        assert_eq!(elsewhere(&[], 501), None);
    }

    #[test]
    fn pressure_from_another_account_starts_no_watch_with_no_worker_or_once_left() {
        assert_eq!(unwatched(Some(502), 0, None), Some(Unwatched::NoWorker));
        assert_eq!(unwatched(Some(502), 2, None), None, "a worker may be acted on");
        assert_eq!(unwatched(None, 0, None), None, "this account's pressure");
        assert_eq!(unwatched(Some(502), 2, Some(502)), Some(Unwatched::LeftWithOperator));
        assert_eq!(unwatched(Some(503), 2, Some(502)), None, "a different account");
        assert!(still_left(502, Some(502)));
        assert!(!still_left(502, Some(503)));
        assert!(!still_left(502, None), "the fleet holds the most again");
    }
}

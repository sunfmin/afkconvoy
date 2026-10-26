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
}

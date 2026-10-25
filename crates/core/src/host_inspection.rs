//! A host inspection (docs/adr/0026): when admission holds new workers back
//! for one reason — the load, memory or disk — past a span, the stock consult
//! is started on the host, not on one worker, as the operator on its host:
//! to find what holds what admission read, fix it, and check that admission
//! admits again. When one starts and what it is told are decided here;
//! taking down what it is given, starting it and ending it are the effect
//! layer's.
//!
//! It is held to an inspection's bounds (`crate::inspection`): one a host at a
//! time, of either kind, in a checkout of its own, no slot of the
//! concurrency, `TIME_LIMIT` to say what it did, and what any unattended
//! worker may never touch, which the guard holds it to where it can.

use serde::{Deserialize, Serialize};

use crate::admission::Hold;
use crate::inspection::{Kind, Operator, operator_fields, with_operator};
use crate::prompt::fill;

/// How long, in seconds, admission holds back for one reason before the host
/// is inspected, where the host says no span of its own.
pub const SPAN: u64 = 600;

/// How long, in seconds, admission admits before an episode ends: holding
/// back after that is a new one, and the host is inspected again.
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holding {
    /// Each reason held back for at every tick since then.
    held: Vec<(Reason, u64)>,
    /// When the episode started; `None` while there is none.
    episode: Option<u64>,
    /// When admission last started admitting, while the episode stands.
    admitted: Option<u64>,
    /// Whether the host was inspected in this episode.
    inspected: bool,
}

/// A host inspection due: for `reason`, held back for since `since`, in the
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

    /// The host inspection due at `now` under a span of `span` seconds: for
    /// the reason held back for longest, once that is `span` or more, unless
    /// the host was inspected in this episode.
    pub fn due(&self, now: u64, span: u64) -> Option<Due> {
        let episode = self.episode?;
        if self.inspected {
            return None;
        }
        let mut due: Option<Due> = None;
        for (reason, since) in &self.held {
            let long = now.saturating_sub(*since) >= span;
            if long && due.is_none_or(|due| *since < due.since) {
                due = Some(Due { reason: *reason, since: *since, episode });
            }
        }
        due
    }

    /// Records that the host was inspected in this episode — by this `run`,
    /// or by another fleet of the host (`covered`).
    pub fn record(&mut self) {
        self.inspected = true;
    }

    /// When the episode started; `None` while admission admits.
    pub fn episode(&self) -> Option<u64> {
        self.episode
    }
}

/// Whether the host inspection `last` — for its reason, started then, by
/// whichever fleet of the host — already covers `due`: another fleet held back
/// for the same reason inspected the host in the same episode.
pub fn covered(last: Option<(Reason, u64)>, due: &Due) -> bool {
    last.is_some_and(|(reason, started)| reason == due.reason && started >= due.episode)
}

/// What calls for a host inspection. Pressure is observed independently of
/// admission: memory may be available even while the host is under pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Admission,
    Pressure,
}

impl Trigger {
    fn condition(self) -> &'static str {
        match self {
            Trigger::Admission => "admission has held back every new worker for {reason}",
            Trigger::Pressure => "the host has been under memory pressure",
        }
    }

    fn goal(self) -> &'static str {
        match self {
            Trigger::Admission => {
                "admission admits new workers again, and holds back for {reason} no more"
            }
            Trigger::Pressure => "the host is under memory pressure no more",
        }
    }

    fn check(self) -> &'static str {
        match self {
            Trigger::Admission => {
                "check that admission would admit now (`afkconvoy plan`, its `admission`)"
            }
            Trigger::Pressure => {
                "check that the host's memory pressure is gone (`sysctl -n kern.memorystatus_vm_pressure_level`, below 2); admission may already admit and is no measure of pressure"
            }
        }
    }
}

/// The oldest uninterrupted condition due, admission first on a tie. The
/// effect layer keeps admission per checkout and pressure once for the host,
/// so a fleet joining during an episode shares its existing coverage.
pub fn next(
    admission: &Holding,
    pressure: &Holding,
    now: u64,
    span: u64,
) -> Option<(Trigger, Due)> {
    let admission = admission.due(now, span);
    let pressure = pressure.due(now, span);
    match (admission, pressure) {
        (Some(a), Some(p)) if p.since < a.since => Some((Trigger::Pressure, p)),
        (Some(a), _) => Some((Trigger::Admission, a)),
        (_, Some(p)) => Some((Trigger::Pressure, p)),
        _ => None,
    }
}

/// What a host inspection's brief is filled with.
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> {
    pub trigger: Trigger,
    pub reason: Reason,
    /// What admission read, as it says it, and for how long it held back.
    pub reading: &'a str,
    pub minutes: u64,
    /// Where what was taken down is.
    pub evidence: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    /// The stock consult's instruction, where nothing else gives it.
    pub instruction: Option<&'a str>,
    /// Where it works, and how it runs afkconvoy there.
    pub operator: Operator<'a>,
}

const BRIEF: &str = "\
You are an afkconvoy **host inspection**, started in your role (`{role}`, skill `{skill}`) on this
**host**: {condition} for {minutes} minutes in a row.
Nobody reads this terminal: never wait for an answer.
{instruction}
## Your goal

The fleet's work goes on: {goal}.
Find what holds the host's {reason}, and why — anywhere on this host, not only in the
fleet's worktrees — fix it, {check}, and say what you did.

## What you are given

- **What the host read:** {reading}
- **What was taken down as it started:** `{evidence}` — admission's reading and shares, {taken}
- **Your checkout:** `{checkout}`, the base branch as it stood, on a branch of its own: `{branch}`
{operator}";

/// What the evidence of a host inspection for `reason` holds, beside
/// admission's reading, as its brief says it.
fn taken(reason: Reason) -> &'static str {
    match reason {
        Reason::Disk => {
            "the largest directories of the whole volume under the home folder: each of this \
             host's worktrees of every fleet, with its issue, its stage and whether a worker holds \
             it, the landing train's, the fleet's caches, and the largest outside them"
        }
        Reason::Load | Reason::Memory => {
            "the process trees of each worker and of the landing train, and the hottest processes \
             outside them"
        }
    }
}

/// The brief a host inspection is given.
pub fn brief(fields: &Fields) -> Result<String, String> {
    let template = BRIEF
        .replace("{condition}", fields.trigger.condition())
        .replace("{goal}", fields.trigger.goal())
        .replace("{check}", fields.trigger.check());
    let template = with_operator(&template, fields.instruction);
    let minutes = fields.minutes.to_string();
    let operator = operator_fields(Kind::Host, &fields.operator);
    let mut filled: Vec<(&str, &str)> = vec![
        ("role", fields.role),
        ("skill", fields.skill),
        ("reason", fields.reason.word()),
        ("minutes", &minutes),
        ("reading", fields.reading),
        ("evidence", fields.evidence),
        ("taken", taken(fields.reason)),
    ];
    filled.extend(operator.iter().map(|(name, value)| (*name, value.as_str())));
    fill(&template, &filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_is_inspected_once_and_recovers_independently_of_admission() {
        let (mut admission, mut pressure) = (Holding::default(), Holding::default());
        pressure.ticked(0, &[Reason::Memory]);
        assert_eq!(next(&admission, &pressure, SPAN - 1, SPAN), None);
        pressure.ticked(SPAN, &[Reason::Memory]);
        let due = Due { reason: Reason::Memory, since: 0, episode: 0 };
        assert_eq!(next(&admission, &pressure, SPAN, SPAN), Some((Trigger::Pressure, due)));
        assert!(covered(Some((Reason::Memory, 20)), &due));
        pressure.record();
        pressure.ticked(900, &[Reason::Memory]);
        assert_eq!(next(&admission, &pressure, 900, SPAN), None);
        pressure.ticked(1000, &[]);
        pressure.ticked(1200, &[Reason::Memory]);
        pressure.ticked(1800, &[Reason::Memory]);
        assert_eq!(next(&admission, &pressure, 1800, SPAN), None, "brief recovery, same episode");
        // Pressure recovers even though load holds admission.
        admission.ticked(2000, &[Reason::Load]);
        pressure.ticked(2000, &[]);
        pressure.ticked(2300, &[]);
        admission.ticked(2400, &[]);
        pressure.ticked(2400, &[Reason::Memory]);
        pressure.ticked(3000, &[Reason::Memory]);
        assert_eq!(
            next(&admission, &pressure, 3000, SPAN),
            Some((Trigger::Pressure, Due { reason: Reason::Memory, since: 2400, episode: 2400 }))
        );
    }

    #[test]
    fn an_inspection_waiting_on_a_cooled_host_episode_starts_nothing() {
        let mut pressure = Holding::default();
        pressure.ticked(0, &[Reason::Memory]);
        let due = pressure.due(SPAN, SPAN).unwrap();
        pressure.ticked(SPAN, &[]);
        assert_ne!(pressure.due(SPAN, SPAN), Some(due));
        pressure.ticked(SPAN + ADMITTED_FOR, &[]);
        assert_eq!(pressure.due(SPAN + ADMITTED_FOR, SPAN), None);
        pressure.ticked(1000, &[Reason::Memory]);
        assert_ne!(pressure.due(1600, SPAN), Some(due), "a different episode");
    }

    #[test]
    fn holding_back_for_one_reason_past_the_span_makes_an_inspection_due() {
        let mut holding = Holding::default();
        holding.ticked(1000, &[Reason::Disk]);
        holding.ticked(1300, &[Reason::Disk]);
        assert_eq!(holding.due(1599, SPAN), None, "less long than the span");
        holding.ticked(1600, &[Reason::Disk]);
        let due = Due { reason: Reason::Disk, since: 1000, episode: 1000 };
        assert_eq!(holding.due(1600, SPAN), Some(due));
        // The host says its own span.
        assert_eq!(holding.due(1600, 900), None);
    }

    #[test]
    fn holding_back_for_changing_reasons_makes_none_due() {
        let mut holding = Holding::default();
        holding.ticked(0, &[Reason::Disk]);
        holding.ticked(300, &[Reason::Load]);
        holding.ticked(600, &[Reason::Memory]);
        holding.ticked(900, &[Reason::Disk]);
        assert_eq!(holding.due(900, SPAN), None);
        // One reason held throughout, beside others that came and went, is due.
        holding.ticked(1200, &[Reason::Disk, Reason::Load]);
        holding.ticked(1500, &[Reason::Disk]);
        assert_eq!(holding.due(1500, SPAN).map(|due| due.reason), Some(Reason::Disk));
        assert_eq!(holding.due(1500, SPAN).map(|due| due.since), Some(900));
    }

    #[test]
    fn a_tick_that_admits_breaks_the_run_of_a_reason() {
        let mut holding = Holding::default();
        holding.ticked(0, &[Reason::Disk]);
        holding.ticked(300, &[]);
        holding.ticked(600, &[Reason::Disk]);
        assert_eq!(holding.due(600, SPAN), None);
        assert_eq!(holding.episode(), Some(0), "but the episode stands");
    }

    #[test]
    fn the_host_is_inspected_once_an_episode_and_again_after_five_minutes_admitted() {
        let mut holding = Holding::default();
        holding.ticked(0, &[Reason::Disk]);
        holding.ticked(600, &[Reason::Disk]);
        assert!(holding.due(600, SPAN).is_some());
        holding.record();
        holding.ticked(1200, &[Reason::Disk]);
        assert_eq!(holding.due(1200, SPAN), None, "once an episode");
        // Admitted for less than five minutes: the same episode.
        holding.ticked(1500, &[]);
        holding.ticked(1700, &[]);
        holding.ticked(1800, &[Reason::Disk]);
        holding.ticked(2400, &[Reason::Disk]);
        assert_eq!(holding.due(2400, SPAN), None, "still the same episode");
        // Admitted for five minutes: the episode ends, and the next is new.
        holding.ticked(2500, &[]);
        holding.ticked(2800, &[]);
        assert_eq!(holding.episode(), None);
        holding.ticked(2900, &[Reason::Disk]);
        holding.ticked(3500, &[Reason::Disk]);
        let due = Due { reason: Reason::Disk, since: 2900, episode: 2900 };
        assert_eq!(holding.due(3500, SPAN), Some(due));
    }

    #[test]
    fn the_reason_held_back_for_longest_is_inspected() {
        let mut holding = Holding::default();
        holding.ticked(0, &[Reason::Load]);
        holding.ticked(100, &[Reason::Load, Reason::Disk]);
        holding.ticked(800, &[Reason::Load, Reason::Disk]);
        assert_eq!(holding.due(800, SPAN).map(|due| due.reason), Some(Reason::Load));
    }

    #[test]
    fn another_fleet_s_inspection_of_the_host_in_the_episode_covers_it() {
        let due = Due { reason: Reason::Disk, since: 100, episode: 50 };
        assert!(covered(Some((Reason::Disk, 60)), &due));
        assert!(!covered(Some((Reason::Disk, 40)), &due), "an episode before");
        assert!(!covered(Some((Reason::Load, 60)), &due), "another reason");
        assert!(!covered(None, &due));
    }

    #[test]
    fn the_brief_gives_the_goal_what_admission_read_the_authority_and_the_bounds() {
        let fields = Fields {
            trigger: Trigger::Admission,
            reason: Reason::Disk,
            reading: "22.9 GiB of the checkout's disk is free, under 46.3 GiB, 5% of it",
            minutes: 10,
            evidence: "/c/host-inspection/evidence.md",
            role: "stock-consult",
            skill: "consult",
            instruction: None,
            operator: Operator {
                checkout: "/w/inspection",
                branch: "tester/afkconvoy-inspection",
                base_branch: "main",
                afkconvoy: "cd '/w/inspection' && afkconvoy",
            },
        };
        let said = brief(&fields).unwrap();
        assert!(said.contains("every new worker for disk for 10 minutes in a row"), "{said}");
        assert!(said.contains("admission admits new workers again"), "{said}");
        assert!(said.contains("whether a worker holds it"), "{said}");
        assert!(said.contains("You are the operator on this host"), "{said}");
        assert!(said.contains("afkconvoy inspected --text-file <file> [--did"), "{said}");
        assert!(said.contains("`~/.cache/huggingface`"), "{said}");
        assert!(said.contains("beside admission's reading"), "{said}");
        assert!(said.contains("`cd '/w/inspection' && afkconvoy land --inspection`"), "{said}");
        for word in ["`reclaimable`", "`fleet`", "`not-its`", "--reclaim", "--name", "--left"] {
            assert!(!said.contains(word), "{word}: {said}");
        }
        let load = brief(&Fields { reason: Reason::Load, ..fields }).unwrap();
        assert!(load.contains("the process trees of each worker"), "{load}");
        let pressure =
            brief(&Fields { trigger: Trigger::Pressure, reason: Reason::Memory, ..fields })
                .unwrap();
        assert!(
            pressure.contains("the host has been under memory pressure for 10 minutes"),
            "{pressure}"
        );
        assert!(pressure.contains("admission may already admit"), "{pressure}");
        assert!(!pressure.contains("admission has held back"), "{pressure}");
        for reason in [Reason::Load, Reason::Memory, Reason::Disk] {
            assert_eq!(Reason::read(reason.word()), Some(reason));
        }
    }
}

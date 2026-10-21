//! A host inspection (docs/adr/0019): when admission holds new workers back
//! for one reason — the load, memory or disk — past a span, the stock consult
//! is started on the host, not on one worker, to find out what holds what
//! admission read, and why. When one starts, what it is told and what the
//! fleet does of what it found are decided here; taking down what it is
//! given, starting it and ending it are the effect layer's.
//!
//! It is held to an inspection's bounds (`crate::inspection`): one a host at a
//! time, of either kind, in a checkout of its own, no slot of the
//! `concurrency`, and `TIME_LIMIT` to say what it found.

use serde::{Deserialize, Serialize};

use crate::admission::Hold;
use crate::inspection::{Found, TIME_LIMIT};
use crate::prompt::fill;

/// How long, in seconds, admission holds back for one reason before the host
/// is inspected, where the host says no span of its own.
pub const SPAN: u64 = 600;

/// How long, in seconds, admission admits before an episode ends: holding
/// back after that is a new one, and the host is inspected again.
pub const ADMITTED_FOR: u64 = 300;

/// The label of an issue a host inspection had the fleet open, for a fleet
/// whose own shape outgrows its host: how GitHub is asked for the one open.
pub const LABEL: &str = "fleet-outgrows-host";

/// The title of that issue.
pub const TITLE: &str = "The fleet outgrows its host: admission keeps holding back new workers";

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
        [Reason::Load, Reason::Memory, Reason::Disk].into_iter().find(|reason| reason.word() == word)
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

/// What the fleet does of what a host inspection found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// It records it, and deletes nothing on its word.
    Record,
    /// It opens the issue that says the fleet outgrows its host — none while
    /// one is open.
    Open,
}

/// What the fleet does of `found`, with `open` the issue already open that
/// says the fleet outgrows its host.
pub fn act(found: Found, open: Option<u64>) -> Act {
    match (found, open) {
        (Found::Fleet, None) => Act::Open,
        _ => Act::Record,
    }
}

/// The body of the issue a host inspection that found the fleet outgrowing
/// its host has the fleet open: what admission read, and the arithmetic it
/// said.
pub fn report(reason: Reason, reading: &str, said: &str, minutes: u64) -> String {
    format!(
        "Admission held back new workers for {} for {minutes} minutes in a row: {reading}. \
         A host inspection found that the fleet's own shape outgrows this host:\n\n{}\n\n\
         Until the shape fits — fewer workers at once (`concurrency`), less that each holds, \
         or a host with more — admission keeps holding new workers back. The fleet opens no \
         other issue like this while this one is open.\n",
        reason.word(),
        quoted(said)
    )
}

fn quoted(said: &str) -> String {
    said.trim().lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n")
}

/// What a host inspection's brief is filled with.
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> {
    pub reason: Reason,
    /// What admission read, as it says it, and for how long it held back.
    pub reading: &'a str,
    pub minutes: u64,
    /// Where what was taken down is.
    pub evidence: &'a str,
    /// The inspection's own checkout of the base branch.
    pub checkout: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    /// The stock consult's instruction, where nothing else gives it.
    pub instruction: Option<&'a str>,
    /// The line it declares what it found with.
    pub inspected_command: &'a str,
}

const BRIEF: &str = "\
You are an afkconvoy **host inspection**, started in your role (`{role}`, skill `{skill}`) on this
**host**: admission has held back every new worker for {reason} for {minutes} minutes in a row.
Nobody reads this terminal: never wait for an answer.
{instruction}
- **What admission read:** {reading}
- **What was taken down as it started:** `{evidence}` — admission's reading and shares, {taken}
- **Your checkout:** `{checkout}`, the base branch as it stood

## What to find

What holds the {reason} admission read, and why. Look at the evidence and at the host as it is now
(`du`, `df`, `ps`, `top -l 1`). It is one of three things:

- `reclaimable`: the fleet holds what it no longer needs — a worktree whose issue is closed, the
  build output of a worktree whose next stage will rebuild it anyway. Name each, with how much.
- `fleet`: the fleet's own shape outgrows the host — its `concurrency` times one worktree's fresh
  build past the free disk, say. Say the arithmetic.
- `not-its`: what holds it is outside this fleet — another fleet, a daemon, a user's files. Name it.

You may read and run anything that changes nothing. Commit nothing, push nothing, and change
nothing on this host: stop no process and delete no file. The fleet deletes nothing on your word
either: what you name is for a human to free.

## Say what you found

You have {minutes_left} minutes. Write what you found, where and why, to a file, then run this once,
with `reclaimable`, `fleet` or `not-its`, and stop:
```bash
{inspected_command} --found <found> --text-file <file>
```
The fleet records it beside admission's reading, and for `fleet` opens one issue saying so.
";

/// What the evidence of a host inspection for `reason` holds, beside
/// admission's reading, as its brief says it.
fn taken(reason: Reason) -> &'static str {
    match reason {
        Reason::Disk => {
            "the largest directories under the checkout's disk: each of this host's worktrees of \
             every fleet, with its issue, its stage and whether a worker holds it, the landing \
             train's, the fleet's caches, and the largest outside them"
        }
        Reason::Load | Reason::Memory => {
            "the process trees of each worker and of the landing train, and the hottest processes \
             outside them"
        }
    }
}

/// The brief a host inspection is given.
pub fn brief(fields: &Fields) -> Result<String, String> {
    let instruction = fields.instruction.map(|text| format!("\n{text}")).unwrap_or_default();
    let template = BRIEF.replace("{instruction}", &instruction);
    fill(
        &template,
        &[
            ("role", fields.role),
            ("skill", fields.skill),
            ("reason", fields.reason.word()),
            ("minutes", &fields.minutes.to_string()),
            ("reading", fields.reading),
            ("evidence", fields.evidence),
            ("taken", taken(fields.reason)),
            ("checkout", fields.checkout),
            ("minutes_left", &(TIME_LIMIT / 60).to_string()),
            ("inspected_command", fields.inspected_command),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn only_fleet_opens_an_issue_and_none_while_one_is_open() {
        assert_eq!(act(Found::Fleet, None), Act::Open);
        assert_eq!(act(Found::Fleet, Some(12)), Act::Record);
        assert_eq!(act(Found::Reclaimable, None), Act::Record);
        assert_eq!(act(Found::NotIts, None), Act::Record);
    }

    #[test]
    fn the_brief_says_what_admission_read_and_what_was_taken_down() {
        let fields = Fields {
            reason: Reason::Disk,
            reading: "22.9 GiB of the checkout's disk is free, under 46.3 GiB, 5% of it",
            minutes: 10,
            evidence: "/c/host-inspection/evidence.md",
            checkout: "/w/inspection",
            role: "stock-consult",
            skill: "consult",
            instruction: None,
            inspected_command: "cd '/w/inspection' && afkconvoy inspected",
        };
        let said = brief(&fields).unwrap();
        assert!(said.contains("every new worker for disk for 10 minutes in a row"), "{said}");
        assert!(said.contains("whether a worker holds it"), "{said}");
        assert!(said.contains("afkconvoy inspected --found <found> --text-file <file>"), "{said}");
        let load = brief(&Fields { reason: Reason::Load, ..fields }).unwrap();
        assert!(load.contains("the process trees of each worker"), "{load}");
        for reason in [Reason::Load, Reason::Memory, Reason::Disk] {
            assert_eq!(Reason::read(reason.word()), Some(reason));
        }
    }

    #[test]
    fn the_issue_says_the_reading_and_the_arithmetic() {
        let body = report(Reason::Disk, "22.9 GiB free", "4 × 15 GiB > 22.9 GiB", 10);
        assert!(body.contains("for disk for 10 minutes in a row: 22.9 GiB free"), "{body}");
        assert!(body.contains("> 4 × 15 GiB > 22.9 GiB"), "{body}");
    }
}

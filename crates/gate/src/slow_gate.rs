//! A slow gate: what the gate is when its recent runs for the base branch, on
//! one host, all exceeded its fixed duration budget, or most of its limit.
//!
//! The limit ends a run that became many times slower. It does nothing about
//! a gate that slows a little with every change, until the change that happens
//! to reach it is stopped by what it did not cause. Whether the gate is slow
//! is judged here, from the recorded gate runs of the landings and the limit;
//! what is done about it is the program's that asks.

use flux_rs::attrs::*;
use serde::Serialize;

use crate::gate::GreenRun;

/// How many landings in a row, on one host, make the gate slow.
pub const LANDINGS: usize = 5;

/// A run took most of the gate limit when it took more than this many
/// hundredths of it.
pub const PERCENT: u64 = 80;

/// A commit on the base branch whose tree has a green run of the gate command
/// on record.
#[derive(Debug)]
pub struct Landing {
    pub commit: String,
    pub run: GreenRun,
}

/// One of the runs that say the gate is slow: the commit that landed, and how
/// long the gate ran on its tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Slowed {
    pub commit: String,
    pub seconds: u64,
}

/// The gate is slow, and the evidence: the limit, the host, and that host's
/// last `LANDINGS` runs, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Slow {
    /// The gate limit the runs are measured against.
    pub gate_seconds: u64,
    /// The declared warning budget, separate from the termination limit.
    /// Absent: `PERCENT` percent of `gate_seconds`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_budget_seconds: Option<u64>,
    pub host: String,
    pub runs: Vec<Slowed>,
}

/// The longest a run under the gate limit of `limit` takes without taking
/// most of it: `PERCENT` hundredths of the limit, rounded down.
#[spec(fn(limit: u64) -> u64{most: most * 100 <= limit * PERCENT && limit * PERCENT < (most + 1) * 100})]
fn most(limit: u64) -> u64 {
    // In two parts: `limit * PERCENT` may not fit.
    (limit / 100) * PERCENT + (limit % 100) * PERCENT / 100
}

/// Whether a run of `seconds` took most of the gate limit of `limit`: more
/// than `PERCENT` hundredths of it.
#[spec(fn(seconds: u64, limit: u64) -> bool[seconds * 100 > limit * PERCENT])]
pub fn took_most(seconds: u64, limit: u64) -> bool {
    seconds > most(limit)
}

/// Whether the gate is slow under the fixed warning `budget`, or `PERCENT`
/// percent of `limit` where absent, by the base branch's `landings`, newest first.
///
/// It is when one host's last `LANDINGS` landings that count all exceeded
/// the warning threshold: one host whose runs say so is enough, the one that landed last
/// of several. A landing counts when its run says how long it took and on
/// which host, and it was made after `since`, where one is given: it is above
/// every landing whose run had finished by then. What landed before is what
/// was already seen to — a report of a slow gate that has closed, say.
pub fn slow(
    landings: &[Landing],
    limit: u64,
    budget: Option<u64>,
    since: Option<u64>,
) -> Option<Slow> {
    let mut counted = Vec::new();
    for landing in landings {
        if since.is_some_and(|since| landing.run.at() <= since) {
            break;
        }
        if let Some(took) = landing.run.took() {
            counted.push((landing.commit.as_str(), took));
        }
    }
    for (_, newest) in &counted {
        let host = newest.host();
        let mut runs = Vec::new();
        let mut all_took_most = true;
        for (commit, took) in &counted {
            if took.host() == host && runs.len() < LANDINGS {
                all_took_most = all_took_most
                    && match budget {
                        Some(budget) => took.seconds() > budget,
                        None => took_most(took.seconds(), limit),
                    };
                runs.push(Slowed { commit: commit.to_string(), seconds: took.seconds() });
            }
        }
        if runs.len() == LANDINGS && all_took_most {
            let host = host.to_string();
            return Some(Slow { gate_seconds: limit, gate_budget_seconds: budget, host, runs });
        }
    }
    None
}

/// The warning threshold as the issue, plan and doctor explain it.
pub fn threshold(slow: &Slow) -> String {
    match slow.gate_budget_seconds {
        Some(budget) => format!(
            "the fixed duration budget of {budget}s (`gate_budget_seconds`); the termination gate limit is {}s (`gate_seconds`)",
            slow.gate_seconds
        ),
        None => format!("{PERCENT}% of the gate limit of {}s (`gate_seconds`)", slow.gate_seconds),
    }
}

/// What `plan` and `doctor` say of the slow gate `slow`: the evidence in one
/// line.
pub fn said(slow: &Slow) -> String {
    let mut runs: Vec<String> = Vec::new();
    for run in &slow.runs {
        runs.push(format!("{} {}s", run.commit, run.seconds));
    }
    format!(
        "on the host {}, the last {LANDINGS} landings with a recorded gate run all took more \
         than {}: {}",
        slow.host,
        threshold(slow),
        runs.join(", ")
    )
}

/// What callers may rely on: each is checked by Flux, and stops passing when
/// the spec it reads is weakened.
#[cfg(flux)]
#[allow(dead_code)]
mod relied_on {
    use super::*;

    /// A run that reached the gate limit took most of it, whatever the limit.
    #[spec(fn(limit: u64{limit > 0}))]
    fn a_run_at_the_limit_took_most_of_it(limit: u64) {
        flux_rs::assert(took_most(limit, limit));
    }

    /// A run of four fifths of the limit did not: most of it is more.
    #[spec(fn(fifth: u64{fifth <= 1000000}))]
    fn a_run_of_four_fifths_did_not(fifth: u64) {
        flux_rs::assert(!took_most(4 * fifth, 5 * fifth));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{GateCommand, Ran, Recording, Tree, after_run};

    /// The landing `commit`, whose run took `seconds` on `host` and finished
    /// at `at`; no host: a run with no time.
    fn landing(commit: &str, host: &str, seconds: u64, at: u64) -> Landing {
        let ran =
            Ran { exit_code: 0, timed_out: false, clean_before: true, clean_after: true, seconds };
        match after_run(&Tree::named(commit), &GateCommand::new("make"), ran, host, at) {
            Recording::Write(run) => Landing { commit: commit.to_string(), run },
            other => panic!("{other:?}"),
        }
    }

    /// Landings newest first, each (host, seconds), finished at 1000, 999, ….
    fn landings(runs: &[(&str, u64)]) -> Vec<Landing> {
        let each = runs.iter().enumerate();
        each.map(|(at, (host, seconds))| {
            landing(&format!("c{at}"), host, *seconds, 1000 - at as u64)
        })
        .collect()
    }

    #[test]
    fn most_of_the_limit_is_more_than_eighty_hundredths_of_it() {
        assert!(!took_most(480, 600), "exactly 80%");
        assert!(took_most(481, 600));
        assert!(!took_most(0, 1) && took_most(1, 1));
        assert!(!took_most(7, 9) && took_most(8, 9), "80% of 9 is 7.2");
        assert!(!took_most(u64::MAX / 5 * 4, u64::MAX), "a limit whose hundredfold does not fit");
        assert!(took_most(u64::MAX, u64::MAX));
    }

    #[test]
    fn five_landings_on_one_host_all_over_most_of_the_limit_are_a_slow_gate() {
        let runs = [("mini", 590), ("mini", 481), ("mini", 500), ("mini", 599), ("mini", 550)];
        let slow = slow(&landings(&runs), 600, None, None).expect("slow");
        assert_eq!((slow.gate_seconds, slow.host.as_str()), (600, "mini"));
        let evidence: Vec<(&str, u64)> =
            slow.runs.iter().map(|run| (run.commit.as_str(), run.seconds)).collect();
        assert_eq!(evidence, [("c0", 590), ("c1", 481), ("c2", 500), ("c3", 599), ("c4", 550)]);
        // an older landing that was fast is not one of the five
        let older = [&runs[..], &[("mini", 10)]].concat();
        assert!(super::slow(&landings(&older), 600, None, None).is_some());
    }

    #[test]
    fn one_run_at_or_under_most_of_the_limit_or_fewer_than_five_is_no_slow_gate() {
        let one_fast = [("mini", 590), ("mini", 480), ("mini", 500), ("mini", 599), ("mini", 550)];
        assert_eq!(slow(&landings(&one_fast), 600, None, None), None);
        let four = [("mini", 590); 4];
        assert_eq!(slow(&landings(&four), 600, None, None), None);
        // the newest five decide: five slow ones under a fast one are not the last five
        let recovered = [&[("mini", 100)][..], &[("mini", 590); 5]].concat();
        assert_eq!(slow(&landings(&recovered), 600, None, None), None);
        assert_eq!(slow(&[], 600, None, None), None);
    }

    #[test]
    fn the_five_are_one_hosts() {
        let spread = [("mini", 590), ("mini", 590), ("mini", 590), ("air", 590), ("air", 590)];
        assert_eq!(slow(&landings(&spread), 600, None, None), None, "no host has five");
        // one host whose runs say so is enough, whatever the other's say
        let mut mixed = vec![("air", 10)];
        mixed.extend([("mini", 590); 5]);
        mixed.push(("air", 20));
        let found = slow(&landings(&mixed), 600, None, None).expect("mini is slow");
        assert_eq!(found.host, "mini");
        let commits: Vec<&str> = found.runs.iter().map(|run| run.commit.as_str()).collect();
        assert_eq!(commits, ["c1", "c2", "c3", "c4", "c5"]);
    }

    #[test]
    fn a_landing_whose_run_has_no_time_is_passed_over() {
        let mut runs = vec![("mini", 590), ("", 5)];
        runs.extend([("mini", 590); 4]);
        let found = slow(&landings(&runs), 600, None, None).expect("the five with a time");
        let commits: Vec<&str> = found.runs.iter().map(|run| run.commit.as_str()).collect();
        assert_eq!(commits, ["c0", "c2", "c3", "c4", "c5"]);
    }

    #[test]
    fn only_the_landings_since_count() {
        // finished at 1000, 999, … 995
        let runs = landings(&[("mini", 590); 6]);
        let closed = Some;
        assert!(slow(&runs, 600, None, closed(994)).is_some());
        assert!(slow(&runs, 600, None, closed(995)).is_some(), "five finished since");
        assert_eq!(slow(&runs, 600, None, closed(996)), None, "four since");
        assert_eq!(slow(&runs, 600, None, closed(2000)), None);
        // a landing made before it closed ends the count, with or without a time
        let mut cut = landings(&[("mini", 590); 3]);
        cut.push(landing("old", "", 5, 10));
        cut.extend(landings(&[("mini", 590); 3]));
        assert!(slow(&cut, 600, None, None).is_some());
        assert_eq!(slow(&cut, 600, None, closed(10)), None, "three since, whatever is under them");
    }

    #[test]
    fn what_is_said_of_a_slow_gate_gives_the_evidence() {
        let found = slow(&landings(&[("mini", 590); 5]), 600, None, None).unwrap();
        assert!(said(&found).contains("on the host mini") && said(&found).contains("c4 590s"));
    }
}

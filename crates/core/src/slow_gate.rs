//! A slow gate, as the fleet reports it: the gate crate judges whether the
//! gate is slow (`afkconvoy_gate::slow_gate`); the fleet opens one issue
//! that says so, for the fleet to work, and no other while it is open.

use serde::Serialize;

pub use afkconvoy_gate::slow_gate::{
    LANDINGS, Landing, PERCENT, Slow, Slowed, said, threshold, took_most,
};

/// The label of an issue the fleet opened for a slow gate: how GitHub is
/// asked for them, open and closed.
pub const LABEL: &str = "slow-gate";

/// The title of that issue.
pub const TITLE: &str = "The gate is slow: its runs keep exceeding the duration budget";

/// The key of the claim one host of several opens that issue under.
pub const CLAIM_KEY: &str = "slow-gate";

/// The issues the fleet opened for a slow gate, as GitHub has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reported {
    /// The one that is open: the lowest, of several.
    pub open: Option<u64>,
    /// When the last one to close did.
    pub closed: Option<u64>,
}

impl Reported {
    /// The fleet opened none.
    pub const NONE: Reported = Reported { open: None, closed: None };

    /// What GitHub says of each issue that carries `LABEL`: its number, and
    /// when it closed — `None`: it is open, `Some(None)`: GitHub gave no
    /// time.
    pub fn among(issues: &[(u64, Option<Option<u64>>)]) -> Reported {
        let mut reported = Reported::NONE;
        for (number, closed) in issues {
            // Spelled out, not one `match` with guards: Flux fails on that one.
            if let Some(closed) = closed {
                if let Some(at) = closed
                    && reported.closed.is_none_or(|last| *at > last)
                {
                    reported.closed = Some(*at);
                }
            } else if reported.open.is_none_or(|open| *number < open) {
                reported.open = Some(*number);
            }
        }
        reported
    }
}

/// The gate is slow, the evidence, and the open issue that says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlowGate {
    #[serde(flatten)]
    pub slow: Slow,
    /// The open issue that says so, when there is one: no other is opened.
    pub issue: Option<u64>,
}

impl std::ops::Deref for SlowGate {
    type Target = Slow;

    fn deref(&self) -> &Slow {
        &self.slow
    }
}

/// Whether the gate is slow under the fixed warning `budget`, or `PERCENT`
/// percent of `limit` where absent, by the base branch's `landings`, newest
/// first, counting only those made since the last issue for a slow gate
/// closed (`afkconvoy_gate::slow_gate::slow`), and the open issue that says so.
///
/// So a gate that is slow since an issue closed is slow by its landings
/// alone, whatever closed: asking with `Reported::NONE` first tells whether
/// GitHub has to be asked at all.
pub fn slow(
    landings: &[Landing],
    limit: u64,
    budget: Option<u64>,
    reported: &Reported,
) -> Option<SlowGate> {
    afkconvoy_gate::slow_gate::slow(landings, limit, budget, reported.closed)
        .map(|slow| SlowGate { slow, issue: reported.open })
}

/// The body of the issue the fleet opens for the slow gate `slow` of
/// `command` on `base_branch`: the evidence, and what is not a fix.
pub fn report(slow: &SlowGate, command: &str, base_branch: &str) -> String {
    let mut runs = String::new();
    for run in &slow.runs {
        runs.push_str(&format!("- `{}` took {}s\n", run.commit, run.seconds));
    }
    format!(
        "**afkconvoy** — the gate is slow. On the host `{host}`, the last {LANDINGS} landings on \
         `{base_branch}` with a recorded gate run all took more than {threshold}, \
         newest first:\n\n{runs}\nNothing is stopped by this warning: a run \
         that reaches the limit is ended and is red, and the PR it stops will not be the one \
         that made the gate slow. Find what `{command}` spends its time on, and make that \
         faster.\n\nTwo things are not a fix: deleting or skipping tests, and raising \
         `gate_seconds` or `gate_budget_seconds`. The gate's output names step wall times, \
         Flux functions and slow tests with their full names and seconds where the gate prints \
         them; compare those logs to the repo's committed baseline. Queue waiting is outside \
         the recorded duration.\n\nThe fleet opens no other issue for a slow gate while this one is \
         open. Once it is closed, only the landings made since count.",
        host = slow.host,
        threshold = threshold(slow),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{GateCommand, Ran, Recording, Tree, after_run};

    fn landings(runs: &[(&str, u64)]) -> Vec<Landing> {
        let each = runs.iter().enumerate();
        each.map(|(at, (host, seconds))| {
            let ran = Ran {
                exit_code: 0,
                timed_out: false,
                clean_before: true,
                clean_after: true,
                seconds: *seconds,
            };
            let commit = format!("c{at}");
            let at = 1000 - at as u64;
            match after_run(&Tree::named(&commit), &GateCommand::new("make"), ran, host, at) {
                Recording::Write(run) => Landing { commit, run },
                other => panic!("{other:?}"),
            }
        })
        .collect()
    }

    #[test]
    fn the_open_issue_is_named_with_the_evidence_and_a_closed_one_ends_the_count() {
        let runs = landings(&[("mini", 590); 5]);
        let reported = Reported { open: Some(41), closed: Some(10) };
        assert_eq!(slow(&runs, 600, None, &reported).unwrap().issue, Some(41));
        assert_eq!(slow(&runs, 600, None, &Reported::NONE).unwrap().issue, None);
        let closed = Reported { open: None, closed: Some(996) };
        assert_eq!(slow(&runs, 600, None, &closed), None, "four since");
    }

    #[test]
    fn what_github_has_is_the_lowest_open_issue_and_the_last_close() {
        let issues =
            [(9, None), (4, Some(Some(300))), (7, None), (5, Some(Some(900))), (6, Some(None))];
        assert_eq!(Reported::among(&issues), Reported { open: Some(7), closed: Some(900) });
        assert_eq!(Reported::among(&[]), Reported::NONE);
    }

    #[test]
    fn the_report_gives_the_evidence_and_what_is_not_a_fix() {
        let found = slow(&landings(&[("mini", 590); 5]), 600, None, &Reported::NONE).unwrap();
        let body = report(&found, "./gate.sh", "main");
        assert!(body.contains("On the host `mini`") && body.contains("gate limit of 600s"));
        for at in 0..5 {
            assert!(body.contains(&format!("- `c{at}` took 590s\n")), "{body}");
        }
        assert!(body.contains("deleting or skipping tests, and raising `gate_seconds`"), "{body}");
        assert!(said(&found).contains("on the host mini") && said(&found).contains("c4 590s"));
    }
}

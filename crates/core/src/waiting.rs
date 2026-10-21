//! What waits for a human: every open issue the fleet has left to one, each
//! with why, and with the command that answers it. `plan` and `doctor` give
//! the list and the panel shows it; the comment of an escalation and the
//! status board end with the same lines, so no two of them can disagree on
//! what a human is to run.

use serde::Serialize;

use crate::line::Line;
use crate::position::{Declared, Position};
use crate::role::{Due, due};
use crate::stage_record::{StageRecord, history};
use crate::team::{Team, Workflow};

/// Every command a human runs to answer an escalated issue, as the
/// `humans` help names them, `<n>` standing for the issue's number: a
/// consult, who stands where that human stands (docs/adr/0018), is given
/// the same list. A command added here is both's.
pub const ANSWERING: [&str; 7] = [
    "afkconvoy rule --issue <n> --text-file <file>",
    "afkconvoy send --issue <n> --stage <stage> --text-file <file>",
    "gh issue close <n> --comment <why>",
    "gh issue create --title <title> --body-file <file> --label <ready label>",
    "gh issue edit <n> --body-file <file>",
    "gh api repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by -F issue_id=<id>",
    "gh api -X DELETE repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by/<id>",
];

/// How long an issue waits for a human before `doctor` counts it a finding.
pub const WAITED_LONG: u64 = 3600;

/// How an escalated issue goes back to the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Back {
    /// By a ruling: the stage it stopped at runs again under it.
    Rule { stage: String },
    /// By being sent to a stage: it has left the stages of its workflow, so
    /// none runs again under a ruling.
    Send,
    /// By one flow label: it follows no one workflow of its team, and nothing
    /// says which it left. `escalate` is the label that comes off.
    Flow { escalate: String },
    /// An unbound repo's: by the ready label, put back in place of the
    /// escalate label.
    Ready { ready: String, escalate: String },
}

/// What an open issue waits for a human with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waits {
    /// It was escalated, or its records call for it: nothing moves it until
    /// a human hands it back.
    Escalated(Back),
    /// It is at a stage that is a human's, which its workflow leads on from
    /// by `verdicts`: `pass`, and `reject` where the stage has that edge.
    Verdict { stage: String, verdicts: Vec<&'static str> },
    /// Its workflow has ended and it is still open: nothing closes it.
    Ended,
    /// It reports a failed run of this patrol, which is not run again while
    /// it is open.
    PatrolFailed { patrol: String },
}

/// One way out of what an issue waits for: what it does, and the line to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WayOut {
    pub what: String,
    pub line: String,
}

fn close(issue: u64, what: &str) -> WayOut {
    let line = Line::new().fixed("gh issue close ").number(issue).own();
    WayOut { what: what.to_string(), line }
}

/// The line that swaps the label `off` of `issue` for the label `on`.
fn swap(issue: u64, on: &str, off: &str) -> String {
    let line = Line::new().fixed("gh issue edit ").number(issue);
    line.fixed(" --add-label ").program(on).fixed(" --remove-label ").program(off).own()
}

/// A command of this tool's about `issue`, with `rest` after its number.
fn own(command: &'static str, issue: u64, rest: &'static str) -> String {
    let line = Line::new().fixed("afkconvoy ").fixed(command).fixed(" --issue ").number(issue);
    line.fixed(rest).own()
}

impl Waits {
    /// Why the issue waits, in a few words.
    pub fn why(&self) -> String {
        match self {
            Waits::Escalated(Back::Rule { stage }) => format!("escalated at {stage}"),
            Waits::Escalated(_) => "escalated".to_string(),
            Waits::Verdict { stage, .. } => format!("stage {stage} is by a human"),
            Waits::Ended => "its workflow has ended, and it is still open".to_string(),
            Waits::PatrolFailed { patrol } => {
                format!("reports a failed run of the patrol `{patrol}`")
            }
        }
    }

    /// Why `plan` leaves the issue off the frontier, where what it waits for
    /// says more than its labels do: an escalated issue carries no flow label
    /// and no ready label, as one nobody ever handed to the fleet.
    pub fn excluded(&self) -> Option<String> {
        let waits = match self {
            Waits::Escalated(Back::Rule { .. }) => "a ruling",
            Waits::Escalated(Back::Send) => "being sent to a stage",
            Waits::Escalated(Back::Flow { .. }) => "its flow label",
            Waits::Escalated(Back::Ready { .. }) => "the ready label",
            Waits::PatrolFailed { patrol } => {
                return Some(format!(
                    "reports a failed run of the patrol `{patrol}`: waits to be closed"
                ));
            }
            Waits::Verdict { .. } | Waits::Ended => return None,
        };
        Some(format!("escalated: waits for {waits}"))
    }

    /// Every way out for `issue`, the one that answers it first: each a line
    /// to run as it is, but for what stands between `<` and `>`.
    pub fn ways_out(&self, issue: u64) -> Vec<WayOut> {
        let answered = match self {
            Waits::Escalated(Back::Rule { stage }) => WayOut {
                what: format!("rule on it: the stage `{stage}` runs again under your ruling"),
                line: own("rule", issue, " --text-file <file>"),
            },
            Waits::Escalated(Back::Send) => WayOut {
                what: "send it to a stage of its workflow: it has left them, so none runs again \
                       under a ruling"
                    .to_string(),
                line: own("send", issue, " --stage <stage> --text-file <file>"),
            },
            Waits::Escalated(Back::Flow { escalate }) => WayOut {
                what: "put its one flow label back: nothing says which workflow it left"
                    .to_string(),
                line: swap(issue, "<flow label>", escalate),
            },
            Waits::Escalated(Back::Ready { ready, escalate }) => WayOut {
                what: "fix what the comment names, and put the ready label back".to_string(),
                line: swap(issue, ready, escalate),
            },
            Waits::Verdict { stage, verdicts } => {
                let mut line = Line::new().fixed("afkconvoy verdict ");
                for (nth, verdict) in verdicts.iter().enumerate() {
                    line = if nth == 0 { line } else { line.fixed("|") }.fixed(verdict);
                }
                let line = line.fixed(" --issue ").number(issue).fixed(" --stage ").program(stage);
                return vec![WayOut {
                    what: format!("declare the verdict of the stage `{stage}`"),
                    line: line.fixed(" --text-file <file>").own(),
                }];
            }
            Waits::Ended => return vec![close(issue, "close it: nothing is left to do on it")],
            Waits::PatrolFailed { .. } => {
                return vec![close(issue, "close it once you have looked: the patrol runs again")];
            }
        };
        vec![answered, close(issue, "close it: the fleet leaves a closed issue alone")]
    }

    /// The line that answers `issue`: the first of its ways out.
    pub fn answer(&self, issue: u64) -> String {
        self.ways_out(issue).into_iter().next().map(|way| way.line).unwrap_or_default()
    }

    /// What the comment of an escalation and the status board end with: each
    /// way out of `issue`, with its line.
    pub fn answer_lines(&self, issue: u64) -> String {
        let mut lines = vec!["**To answer it**, in a checkout of this repo:".to_string()];
        for way in self.ways_out(issue) {
            lines.push(format!("- {} — `{}`", way.what, way.line));
        }
        lines.join("\n")
    }
}

/// How an escalated issue that followed `workflow` — or none, where nothing
/// says which — goes back to a bound repo's fleet, given its stage records.
pub fn back(workflow: Option<&Workflow>, records: &[StageRecord], escalate: &str) -> Back {
    let Some(workflow) = workflow else {
        return Back::Flow { escalate: escalate.to_string() };
    };
    match history(workflow, records).place.position.stage() {
        Some(stage) => Back::Rule { stage: stage.name.clone() },
        None => Back::Send,
    }
}

/// What a bound repo's open issue nobody holds waits for a human with, while
/// it carries a flow label: `labels` are its labels, `records` its stage
/// records, oldest first, and `pr` its open PR. `None` where a worker is due
/// on it. Who it is with is `role::due`'s to say: this only says what answers
/// it.
pub fn waits(
    team: &Team,
    labels: &[String],
    records: &[StageRecord],
    pr: Option<u64>,
) -> Option<Waits> {
    if team.flows(labels).is_empty() {
        return None;
    }
    let workflow = crate::role::workflow_of(team, labels).ok();
    match due(team, labels, records, pr) {
        Due::Worker(_) | Due::Consult(_) | Due::Lander(_) => None,
        Due::Human(_) => Some(Waits::Escalated(back(workflow, records, &team.escalate_label))),
        Due::Nothing(_) => {
            let read = history(workflow?, records);
            let mut verdicts = vec![];
            // The two a workflow leads on from: what else a stage
            // may declare, `afkconvoy verdict --help` says.
            for declared in [Declared::Pass, Declared::Reject] {
                if read.place.takes(declared) {
                    verdicts.push(declared.word());
                }
            }
            let position = read.place.position;
            match position.stage() {
                None => matches!(position, Position::Ended).then_some(Waits::Ended),
                Some(stage) => Some(Waits::Verdict { stage: stage.name.clone(), verdicts }),
            }
        }
    }
}

/// One open issue that waits for a human, as `plan` and `doctor` answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Waiting {
    pub issue: u64,
    pub title: String,
    /// Why it waits, in a few words.
    pub why: String,
    /// Since when, in epoch seconds, where GitHub said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// The line that answers it.
    pub answer: String,
    #[serde(skip)]
    pub waits: Waits,
}

impl Waiting {
    pub fn new(issue: u64, title: &str, waits: Waits, since: Option<u64>) -> Waiting {
        Waiting {
            issue,
            title: title.to_string(),
            why: waits.why(),
            since,
            answer: waits.answer(issue),
            waits,
        }
    }

    /// How long it has waited at `now`, once that is long enough to count
    /// against it (`WAITED_LONG`).
    pub fn waited_long(&self, now: u64) -> Option<u64> {
        let waited = now.checked_sub(self.since?)?;
        if waited > WAITED_LONG { Some(waited) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> Waits {
        Waits::Escalated(Back::Rule { stage: "workup".into() })
    }

    #[test]
    fn an_escalated_issue_names_the_command_that_answers_it_with_its_number() {
        let waiting = Waiting::new(23, "prn doses", rule(), Some(1_791_862_506));
        assert_eq!(waiting.why, "escalated at workup");
        assert_eq!(waiting.answer, "afkconvoy rule --issue 23 --text-file <file>");
        assert_eq!(rule().excluded().as_deref(), Some("escalated: waits for a ruling"));
        assert_eq!(
            rule().answer_lines(23),
            "**To answer it**, in a checkout of this repo:\n\
             - rule on it: the stage `workup` runs again under your ruling — `afkconvoy rule \
             --issue 23 --text-file <file>`\n\
             - close it: the fleet leaves a closed issue alone — `gh issue close 23`"
        );
    }

    #[test]
    fn every_way_of_waiting_has_a_line_to_run() {
        let answer = |waits: Waits| waits.answer(12);
        assert_eq!(
            answer(Waits::Verdict { stage: "sign-off".into(), verdicts: vec!["pass", "reject"] }),
            "afkconvoy verdict pass|reject --issue 12 --stage sign-off --text-file <file>"
        );
        assert_eq!(
            answer(Waits::Escalated(Back::Send)),
            "afkconvoy send --issue 12 --stage <stage> --text-file <file>"
        );
        assert_eq!(
            answer(Waits::Escalated(Back::Flow { escalate: "needs a human".into() })),
            "gh issue edit 12 --add-label '<flow label>' --remove-label 'needs a human'"
        );
        let ready = Back::Ready { ready: "ready-for-agent".into(), escalate: "stuck".into() };
        assert_eq!(
            answer(Waits::Escalated(ready)),
            "gh issue edit 12 --add-label ready-for-agent --remove-label stuck"
        );
        assert_eq!(answer(Waits::Ended), "gh issue close 12");
        let failed = Waits::PatrolFailed { patrol: "triage".into() };
        assert_eq!(answer(failed.clone()), "gh issue close 12");
        assert_eq!(failed.why(), "reports a failed run of the patrol `triage`");
        // What its labels already say is left to them.
        assert_eq!(Waits::Ended.excluded(), None);
    }

    #[test]
    fn an_issue_counts_as_waiting_long_only_past_the_hour() {
        let waiting = |since| Waiting::new(7, "t", rule(), since);
        assert_eq!(waiting(Some(1000)).waited_long(1000 + WAITED_LONG), None);
        assert_eq!(waiting(Some(1000)).waited_long(1001 + WAITED_LONG), Some(WAITED_LONG + 1));
        assert_eq!(waiting(None).waited_long(u64::MAX), None, "nothing says since when");
        assert_eq!(waiting(Some(2000)).waited_long(1000), None, "a clock behind GitHub's");
    }
}

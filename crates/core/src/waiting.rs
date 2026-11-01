//! What waits for a human: every open issue the fleet has left to one, each
//! with why, and with the command that answers it. `plan` and `doctor` give
//! the list and the panel shows it; the comment of an escalation and the
//! status board end with the same lines, so no two of them can disagree on
//! what a human is to run. The lines here name no checkout: the command that
//! prints them names it, in the form that command takes, so they run from
//! any directory.

use serde::Serialize;

use crate::line::Line;
use crate::position::{Declared, Position};
use crate::prompt::in_tracker;
use crate::role::{Due, due};
use crate::stage_record::{StageRecord, Stated, history};
use crate::team::{Team, Workflow};
use crate::tracker::Tracker;

/// Every command a human or the watch runs to answer an escalated issue, `<n>` standing
/// for the issue's number, with the marks of the fleet's tracker in each of
/// its issue commands (`answering`).
const ANSWERING: [&str; 7] = [
    "afkconvoy rule --issue <n> --text-file <file>",
    "afkconvoy send --issue <n> --stage <stage> --text-file <file>",
    "gh issue close <n><tracker> --comment <why>",
    "gh issue create<tracker> --title <title> --body-file <file> --label <ready label><labelled>",
    "gh issue edit <n><tracker> --body-file <file>",
    "gh api repos/<issues>/issues/<n>/dependencies/blocked_by -F issue_id=<id>",
    "gh api -X DELETE repos/<issues>/issues/<n>/dependencies/blocked_by/<id>",
];

/// Every command a human runs to answer an escalated issue of `tracker`, as
/// the `humans` help names them of the checkout's own repo, `<n>` standing
/// for the issue's number: a watch, or a named consult standing where that human stands
/// (docs/adr/0018), is given the same list, each issue command naming the
/// tracker. A command added here is given to all three.
pub fn answering(tracker: &Tracker) -> Vec<String> {
    ANSWERING.iter().map(|line| in_tracker(line, tracker)).collect()
}

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
    /// says which it left. `labels` are the team's available flow labels;
    /// its only one is filled in, otherwise a human chooses among them.
    /// `escalate` is the label that comes off.
    Flow { labels: Vec<String>, escalate: String },
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
    /// The consult waits for evidence and an explicit send.
    Parked { stage: String },
    /// It reports a failed run of this patrol, which is not run again while
    /// it is open.
    PatrolFailed { patrol: String },
    /// It says a cut failed (docs/adr/0028): no cut starts while it is open.
    CutFailed,
}

/// One way out of what an issue waits for: what it does, and the line to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WayOut {
    pub what: String,
    pub line: String,
}

fn close(issue: u64, tracker: &Tracker, what: &str) -> WayOut {
    WayOut { what: what.to_string(), line: tracker.gh_issue("close", issue).own() }
}

/// The line that swaps the label `off` of `issue` of `tracker` for the label `on`.
fn swap(issue: u64, tracker: &Tracker, on: &str, off: &str) -> String {
    let line = tracker.gh_issue("edit", issue);
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
            Waits::Escalated(Back::Flow { labels, .. }) => match labels.as_slice() {
                [_] => "escalated with no position".to_string(),
                [] => "escalated with no position; `afkconvoy flows` lists flow labels".to_string(),
                _ => format!(
                    "escalated with no position; choose <flow label> from: {}",
                    labels.join(", ")
                ),
            },
            Waits::Escalated(_) => "escalated".to_string(),
            Waits::Verdict { stage, .. } => format!("stage {stage} is by a human"),
            Waits::Parked { stage } => format!("parked for evidence at {stage}"),
            Waits::Ended => "its workflow has ended, and it is still open".to_string(),
            Waits::PatrolFailed { patrol } => {
                format!("reports a failed run of the patrol `{patrol}`")
            }
            Waits::CutFailed => "says a cut failed".to_string(),
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
            Waits::CutFailed => return Some("says a cut failed: waits to be closed".to_string()),
            Waits::Parked { .. } => return Some(self.why()),
            Waits::Verdict { .. } | Waits::Ended => return None,
        };
        Some(format!("escalated: waits for {waits}"))
    }

    /// Every way out for `issue` of `tracker`, the one that answers it first:
    /// each a line to run as it is, in a checkout of the repo, but for what
    /// stands between `<` and `>`.
    pub fn ways_out(&self, issue: u64, tracker: &Tracker) -> Vec<WayOut> {
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
            Waits::Escalated(Back::Flow { labels, escalate }) => WayOut {
                what: format!("put a flow label on it: {}", self.why()),
                line: match labels.as_slice() {
                    [label] => swap(issue, tracker, label, escalate),
                    _ => swap(issue, tracker, "<flow label>", escalate),
                },
            },
            Waits::Escalated(Back::Ready { ready, escalate }) => WayOut {
                what: "fix what the comment names, and put the ready label back".to_string(),
                line: swap(issue, tracker, ready, escalate),
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
            Waits::Parked { stage } => WayOut {
                what: format!("bring it back with new evidence at {stage}"),
                line: own("send", issue, " --stage <stage> --text-file <evidence-file>"),
            },
            Waits::Ended => {
                return vec![close(issue, tracker, "close it: nothing is left to do on it")];
            }
            Waits::PatrolFailed { .. } => {
                let what = "close it once you have looked: the patrol runs again";
                return vec![close(issue, tracker, what)];
            }
            Waits::CutFailed => {
                return vec![close(
                    issue,
                    tracker,
                    "go on from the step that failed with the line its body gives, which closes \
                     it — or close it once you have looked: the fleet cuts again from the start",
                )];
            }
        };
        vec![answered, close(issue, tracker, "close it: the fleet leaves a closed issue alone")]
    }

    /// The line that answers `issue` of `tracker`: the first of its ways out.
    pub fn answer(&self, issue: u64, tracker: &Tracker) -> String {
        let first = self.ways_out(issue, tracker).into_iter().next();
        first.map(|way| way.line).unwrap_or_default()
    }

    /// What the comment of an escalation and the status board end with: each
    /// way out of `issue` of `tracker`, with its line.
    pub fn answer_lines(&self, issue: u64, tracker: &Tracker) -> String {
        let mut lines = vec!["**To answer it**, in a checkout of this repo:".to_string()];
        for way in self.ways_out(issue, tracker) {
            lines.push(format!("- {} — `{}`", way.what, way.line));
        }
        lines.join("\n")
    }
}

/// How an escalated issue that followed `workflow` — or none, where nothing
/// says which — goes back to a bound repo's fleet, given its stage records
/// and the team whose flow labels a human may choose from.
pub fn back(team: &Team, workflow: Option<&Workflow>, records: &[StageRecord]) -> Back {
    let Some(workflow) = workflow else {
        let labels =
            team.workflows.iter().map(|workflow| workflow.flow_label().to_string()).collect();
        return Back::Flow { labels, escalate: team.escalate_label.clone() };
    };
    let position = history(workflow, records.to_vec()).place.position;
    if position.landing() {
        return Back::Rule { stage: crate::team::LAND.to_string() };
    }
    match position.stage() {
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
        Due::Human(_) => Some(Waits::Escalated(back(team, workflow, records))),
        Due::Nothing(_) => {
            let read = history(workflow?, records.to_vec());
            let mut verdicts = vec![];
            // The two a workflow leads on from: what else a stage
            // may declare, `afkconvoy verdict --help` says.
            for declared in [Declared::Pass, Declared::Reject] {
                if read.place.takes(declared) {
                    verdicts.push(declared.word());
                }
            }
            let position = read.place.position;
            if let Position::ParkedForEvidence(stage) = position {
                return Some(Waits::Parked { stage: stage.name.clone() });
            }
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
    /// What kind of wait, then the first sentence of its source's reason.
    pub why: String,
    /// The comment that gives the reason, or the report itself for an older
    /// failed patrol or cut that left no comment. None without a source.
    pub url: Option<String>,
    /// Since when, in epoch seconds, where GitHub said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// The line that answers it.
    pub answer: String,
    #[serde(skip)]
    pub waits: Waits,
}

impl Waiting {
    /// `issue` of `tracker`, which waits for a human with `waits`.
    pub fn new(
        issue: u64,
        tracker: &Tracker,
        title: &str,
        waits: Waits,
        since: Option<u64>,
    ) -> Waiting {
        Waiting {
            issue,
            title: title.to_string(),
            why: waits.why(),
            url: None,
            since,
            answer: waits.answer(issue, tracker),
            waits,
        }
    }

    /// Adds the reason from the comment that explains this wait, ignoring
    /// later status chatter. `sources` are oldest first; an older failure
    /// report's body may stand first, before its comments.
    pub fn explain(&mut self, sources: &[Said<'_>]) {
        let source = sources
            .iter()
            .rev()
            .find_map(|source| self.waits.reason(source.body).map(|reason| (source, reason)));
        if let Some((source, reason)) = source {
            let reason = first_sentence(&reason);
            if !reason.is_empty() {
                self.why = format!("{}: {reason}", self.waits.why());
            }
            self.url = Some(source.url.to_string());
            self.since = source.at;
        }
    }

    /// How long it has waited at `now`, once that is long enough to count
    /// against it (`WAITED_LONG`).
    pub fn waited_long(&self, now: u64) -> Option<u64> {
        let waited = now.checked_sub(self.since?)?;
        if waited > WAITED_LONG { Some(waited) } else { None }
    }
}

/// A source already read from GitHub; no extra request is needed to explain
/// a wait. The URL and time belong to the same text that gives its reason.
pub struct Said<'a> {
    pub body: &'a str,
    pub url: &'a str,
    pub at: Option<u64>,
}

impl Waits {
    /// The reason this source gives for this kind of wait, without the
    /// fleet's marker or heading. Unrelated comments give none.
    fn reason(&self, body: &str) -> Option<String> {
        let after_marker = || body.split_once('\n').map(|(_, text)| text.trim());
        match self {
            Waits::Escalated(_) => {
                // A position may call for a human before the tick writes
                // its escalation. A newer stage record then supersedes an
                // old escalation from before the issue was handed back.
                if crate::transition::escalated_at([body]).is_none() {
                    return StageRecord::read(body).map(|record| record.text);
                }
                let text = after_marker()?;
                Some(
                    text.strip_prefix("**afkconvoy: escalated to a human.**")
                        .unwrap_or(text)
                        .trim()
                        .into(),
                )
            }
            Waits::Parked { stage } => {
                let record = StageRecord::read(body)?;
                (record.stage == *stage && record.stated == Stated::Declared(Declared::Park))
                    .then_some(record.text)
            }
            Waits::PatrolFailed { patrol } => {
                let (_, fields) = crate::record::latest_marker(crate::patrol::FAILED, [body])?;
                (fields.get("patrol") == Some(patrol))
                    .then(|| report_reason(after_marker().unwrap_or("")))
            }
            Waits::CutFailed => {
                crate::record::latest_marker(crate::cut::FAILED, [body])?;
                Some(report_reason(after_marker()?))
            }
            Waits::Verdict { .. } | Waits::Ended => {
                StageRecord::read(body).map(|record| record.text)
            }
        }
    }
}

/// The reason's first sentence on one line. A dot inside a version or URL
/// is no sentence boundary; Chinese sentence endings need no following space.
fn first_sentence(text: &str) -> String {
    let paragraph = text.trim().split("\n\n").next().unwrap_or("");
    let text = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    for (at, ch) in text.char_indices() {
        let end = at.saturating_add(ch.len_utf8());
        let rest = text.get(end..).unwrap_or("");
        let ended = matches!(ch, '。' | '！' | '？')
            || (matches!(ch, '.' | '!' | '?')
                && (rest.is_empty() || rest.starts_with(char::is_whitespace)));
        if ended {
            return text.get(..end).unwrap_or(&text).to_string();
        }
    }
    text
}

fn report_reason(text: &str) -> String {
    text.strip_prefix("**afkconvoy** — ").unwrap_or(text).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> Waits {
        Waits::Escalated(Back::Rule { stage: "workup".into() })
    }

    #[test]
    fn reasons_use_the_first_sentence_without_cutting_versions_or_urls() {
        assert_eq!(
            first_sentence("Needs v0.5.18\nfrom https://example.com/a. Then retry."),
            "Needs v0.5.18 from https://example.com/a."
        );
        assert_eq!(first_sentence("还缺原始日志。收到后重试。"), "还缺原始日志。");
        assert_eq!(first_sentence("Need raw logs\n\n**To answer it**"), "Need raw logs");
    }

    #[test]
    fn an_escalated_issue_names_the_command_that_answers_it_with_its_number() {
        let waiting = Waiting::new(23, &Tracker::Own, "prn doses", rule(), Some(1_791_862_506));
        assert_eq!(waiting.why, "escalated at workup");
        assert_eq!(waiting.answer, "afkconvoy rule --issue 23 --text-file <file>");
        assert_eq!(rule().excluded().as_deref(), Some("escalated: waits for a ruling"));
        assert_eq!(
            rule().answer_lines(23, &Tracker::Own),
            "**To answer it**, in a checkout of this repo:\n\
             - rule on it: the stage `workup` runs again under your ruling — `afkconvoy rule \
             --issue 23 --text-file <file>`\n\
             - close it: the fleet leaves a closed issue alone — `gh issue close 23`"
        );
    }

    #[test]
    fn every_way_of_waiting_has_a_line_to_run() {
        let answer = |waits: Waits| waits.answer(12, &Tracker::Own);
        assert_eq!(
            answer(Waits::Verdict { stage: "sign-off".into(), verdicts: vec!["pass", "reject"] }),
            "afkconvoy verdict pass|reject --issue 12 --stage sign-off --text-file <file>"
        );
        assert_eq!(
            answer(Waits::Escalated(Back::Send)),
            "afkconvoy send --issue 12 --stage <stage> --text-file <file>"
        );
        assert_eq!(
            answer(Waits::Escalated(Back::Flow {
                labels: vec![],
                escalate: "needs a human".into()
            })),
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
    fn a_line_that_answers_an_issue_of_another_tracker_names_it() {
        let tracker = Tracker::Other {
            tracker: crate::config::RepoName::read("acme/tracker").unwrap(),
            repo: crate::config::RepoName::read("acme/dev").unwrap(),
        };
        let answer = |waits: Waits| waits.answer(12, &tracker);
        assert_eq!(answer(Waits::Ended), "gh issue close 12 --repo acme/tracker");
        assert_eq!(
            answer(Waits::Escalated(Back::Flow { labels: vec![], escalate: "stuck".into() })),
            "gh issue edit 12 --repo acme/tracker --add-label '<flow label>' --remove-label stuck"
        );
        // A command of the fleet's own works the checkout's tracker itself.
        assert_eq!(answer(rule()), "afkconvoy rule --issue 12 --text-file <file>");
        assert!(
            rule().answer_lines(12, &tracker).ends_with("`gh issue close 12 --repo acme/tracker`")
        );
    }

    #[test]
    fn an_issue_counts_as_waiting_long_only_past_the_hour() {
        let waiting = |since| Waiting::new(7, &Tracker::Own, "t", rule(), since);
        assert_eq!(waiting(Some(1000)).waited_long(1000 + WAITED_LONG), None);
        assert_eq!(waiting(Some(1000)).waited_long(1001 + WAITED_LONG), Some(WAITED_LONG + 1));
        assert_eq!(waiting(None).waited_long(u64::MAX), None, "nothing says since when");
        assert_eq!(waiting(Some(2000)).waited_long(1000), None, "a clock behind GitHub's");
    }
}

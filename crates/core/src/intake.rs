//! Intake by a worker (docs/adr/0011): the record it leaves on the issue it
//! took in, how an issue is read to have come in, and the limit on such
//! issues still open. The comment's format has this one home: only `afkconvoy
//! intake` writes it. Nothing is counted but these records: a flow label with
//! none was a human's, and is outside the limit.

use crate::record;
use crate::role::flows;

/// The kind of record an intake record's comment opens with.
pub const INTAKE_RECORD: &str = "convoy:intake";

/// What a worker's intake left on the issue it took in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeRecord {
    /// The workflow the issue was taken in to: what follows `flow:` in the
    /// flow label put on it.
    pub workflow: String,
    /// The role of the worker that took it in.
    pub role: String,
    /// The issue that worker was on.
    pub from: u64,
}

impl IntakeRecord {
    /// The comment that carries the record: its marker, and a line for a human.
    pub fn comment(&self) -> String {
        let from = self.from.to_string();
        let marker = record::marker(
            INTAKE_RECORD,
            &[("workflow", &self.workflow), ("role", &self.role), ("from", &from)],
        );
        format!(
            "{marker}\n**afkconvoy** — a worker in the role `{}`, on #{}, took this issue in to \
             the workflow `{}`.",
            self.role, self.from, self.workflow
        )
    }

    /// The record a comment carries; `None` when it is no intake record: it
    /// does not open with the marker, or the marker leaves out what every
    /// record states.
    pub fn read(body: &str) -> Option<IntakeRecord> {
        let (marker, _) = body.strip_prefix("<!--")?.split_once("-->")?;
        let fields = record::read(INTAKE_RECORD, marker)?;
        Some(IntakeRecord {
            workflow: fields.get("workflow")?.to_string(),
            role: fields.get("role")?.to_string(),
            from: fields.number("from")?,
        })
    }
}

/// The intake by which an issue carrying `labels` follows a workflow now,
/// when a worker's: the latest intake record among its comments, oldest
/// first, that names a workflow one of its flow labels names → (the place of
/// its comment, the record). `None`: a human labelled it, or it carries no
/// flow label. A record for a workflow whose label the issue no longer
/// carries says nothing of how it carries the one it has.
pub fn taken_in<'a>(
    labels: &[String],
    bodies: impl IntoIterator<Item = &'a str>,
) -> Option<(usize, IntakeRecord)> {
    let carried = flows(labels);
    let mut found = None;
    for (at, body) in bodies.into_iter().enumerate() {
        if let Some(record) = IntakeRecord::read(body)
            && carried.contains(&record.workflow.as_str())
        {
            found = Some((at, record));
        }
    }
    found
}

/// Whether a worker may take one more issue in, with `open` open issues taken
/// in by workers and the repo's `intake_limit` at `limit`.
pub fn has_room(open: usize, limit: u64) -> bool {
    (open as u64) < limit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| label.to_string()).collect()
    }

    fn record(workflow: &str, role: &str, from: u64) -> IntakeRecord {
        IntakeRecord { workflow: workflow.to_string(), role: role.to_string(), from }
    }

    #[test]
    fn a_record_reads_back_from_its_comment() {
        let written = record("small fix", "dev", 7);
        let comment = written.comment();
        assert!(comment.starts_with("<!--convoy:intake workflow=small%20fix role=dev from=7-->\n"));
        assert_eq!(IntakeRecord::read(&comment), Some(written));
    }

    #[test]
    fn a_comment_that_does_not_open_with_the_whole_marker_is_no_record() {
        let comment = record("feature", "dev", 7).comment();
        assert_eq!(IntakeRecord::read(&format!("quoted:\n{comment}")), None);
        assert_eq!(IntakeRecord::read("<!--convoy:intake workflow=feature role=dev-->"), None);
        assert_eq!(IntakeRecord::read("<!--convoy:intake role=dev from=7-->"), None);
        assert_eq!(IntakeRecord::read("<!--convoy:stage workflow=a role=dev from=7-->"), None);
    }

    #[test]
    fn an_issue_was_taken_in_by_the_latest_record_for_the_flow_label_it_carries() {
        let (first, second) = (record("feature", "dev", 7).comment(), record("feature", "lead", 9));
        let bodies = [first.as_str(), "chat", &second.comment(), "more chat"];
        assert_eq!(taken_in(&labels(&["flow:feature", "bug"]), bodies), Some((2, second)));
    }

    #[test]
    fn a_flow_label_with_no_record_for_it_was_a_human_s() {
        let other = record("feature", "dev", 7).comment();
        assert_eq!(taken_in(&labels(&["flow:feature"]), ["chat"]), None);
        // A human moved it to another workflow: the worker's record is not for that one.
        assert_eq!(taken_in(&labels(&["flow:fix"]), [other.as_str()]), None);
        // …and an issue that carries no flow label is in no workflow at all.
        assert_eq!(taken_in(&labels(&["bug"]), [other.as_str()]), None);
    }

    #[test]
    fn there_is_room_below_the_limit_only() {
        assert!(has_room(0, 1));
        assert!(has_room(49, 50));
        assert!(!has_room(50, 50));
        assert!(!has_room(51, 50));
        assert!(!has_room(0, 0));
    }
}

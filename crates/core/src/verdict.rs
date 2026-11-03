//! A verdict: what a worker that opens no PR declares, as a marker comment on
//! its issue.

use crate::blocked::BlockedOn;
use crate::record;

/// The kind of record a verdict comment carries.
pub const VERDICT: &str = "convoy:verdict";

/// What a worker may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The issue is already implemented on the base branch.
    AlreadySatisfied,
    /// The issue waits on other issues, which the verdict names, or on the
    /// next cut of its repo.
    Blocked,
    /// A human has to step in: the worker is stuck, or the issue cannot be done as written.
    Escalate,
}

impl Phase {
    pub const ALL: [Phase; 3] = [Phase::AlreadySatisfied, Phase::Blocked, Phase::Escalate];

    pub fn word(self) -> &'static str {
        match self {
            Phase::AlreadySatisfied => "already-satisfied",
            Phase::Blocked => "blocked",
            Phase::Escalate => "escalate",
        }
    }

    /// The phase a marker's word names. `needs-human` is what `escalate` was
    /// called: comments written under that name are still on issues.
    pub fn read(word: &str) -> Option<Phase> {
        if word == "needs-human" {
            return Some(Phase::Escalate);
        }
        Phase::ALL.into_iter().find(|phase| phase.word() == word)
    }
}

/// What a worker declared on its issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// `None` when the marker names no phase the fleet knows: the worker
    /// writes it by hand, and a marker is a verdict whatever it says.
    pub phase: Option<Phase>,
    /// What a `blocked` verdict says it waits on.
    pub blocked_by: BlockedOn,
    /// Which of the comments read carries it.
    pub comment: usize,
}

/// The worker's verdict on an issue, from the bodies of its comments, oldest
/// first: the latest marker wins.
pub fn latest_verdict<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Option<Verdict> {
    let (comment, fields) = record::latest_marker(VERDICT, bodies)?;
    let phase = match fields.get("phase") {
        Some(word) => Phase::read(word),
        None => None,
    };
    Some(Verdict { phase, blocked_by: BlockedOn::read(fields.get("blocked_by")), comment })
}

/// The marker as it is shown to a worker that must write one by hand.
pub fn marker_format() -> String {
    let words: Vec<&str> = Phase::ALL.iter().map(|phase| phase.word()).collect();
    format!("<!--{VERDICT} phase=<{}> [blocked_by=<n,…|cut>]-->", words.join("|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_reads_back_from_its_word() {
        for phase in Phase::ALL {
            assert_eq!(Phase::read(phase.word()), Some(phase));
        }
        assert_eq!(Phase::read("needs-human"), Some(Phase::Escalate));
        assert_eq!(Phase::read("giving-up"), None);
        assert_eq!(
            marker_format(),
            "<!--convoy:verdict phase=<already-satisfied|blocked|escalate> [blocked_by=<n,…|cut>]-->"
        );
    }

    #[test]
    fn the_latest_verdict_wins_and_an_unknown_phase_is_still_a_verdict() {
        let bodies = [
            "<!--convoy:verdict phase=blocked blocked_by=41, #42-->\nwaiting on the schema",
            "just a comment",
            "<!--convoy:verdict phase=escalate-->\nI cannot make the gate pass",
        ];
        let verdict = latest_verdict(bodies).unwrap();
        assert_eq!((verdict.phase, verdict.comment), (Some(Phase::Escalate), 2));
        let first = latest_verdict(bodies[..2].iter().copied()).unwrap();
        assert_eq!(
            (first.phase, first.blocked_by, first.comment),
            (Some(Phase::Blocked), BlockedOn::issues(vec![41, 42]), 0)
        );
        let on_cut = latest_verdict(["<!--convoy:verdict phase=blocked blocked_by=cut-->"]);
        assert_eq!(on_cut.unwrap().blocked_by, BlockedOn { issues: vec![], cut: true });
        assert_eq!(latest_verdict(["<!--convoy:verdict phase=giving-up-->"]).unwrap().phase, None);
        assert_eq!(latest_verdict(["<!--convoy:verdict-->"]).unwrap().phase, None);
        assert_eq!(latest_verdict(["nothing declared"]), None);
    }
}

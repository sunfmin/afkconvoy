//! A verdict: what a worker that opens no PR declares, as a marker comment on
//! its issue.

/// The kind of record a verdict comment carries.
pub const VERDICT: &str = "convoy:verdict";

/// What a worker may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The issue is already implemented on the base branch.
    AlreadySatisfied,
    /// The issue waits on other issues, which the verdict names.
    Blocked,
    /// A human has to step in: the worker is stuck, or the issue cannot be done as written.
    NeedsHuman,
}

impl Phase {
    pub const ALL: [Phase; 3] = [Phase::AlreadySatisfied, Phase::Blocked, Phase::NeedsHuman];

    pub fn word(self) -> &'static str {
        match self {
            Phase::AlreadySatisfied => "already-satisfied",
            Phase::Blocked => "blocked",
            Phase::NeedsHuman => "needs-human",
        }
    }

    pub fn read(word: &str) -> Option<Phase> {
        Phase::ALL.into_iter().find(|phase| phase.word() == word)
    }
}

/// The marker as it is shown to a worker that must write one by hand.
pub fn marker_format() -> String {
    let words: Vec<&str> = Phase::ALL.iter().map(|phase| phase.word()).collect();
    format!("<!--{VERDICT} phase=<{}> [blocked_by=<n,…>]-->", words.join("|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_reads_back_from_its_word() {
        for phase in Phase::ALL {
            assert_eq!(Phase::read(phase.word()), Some(phase));
        }
        assert_eq!(Phase::read("giving-up"), None);
        assert_eq!(
            marker_format(),
            "<!--convoy:verdict phase=<already-satisfied|blocked|needs-human> [blocked_by=<n,…>]-->"
        );
    }
}

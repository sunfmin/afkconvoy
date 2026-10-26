//! The help's table of [`Status`]: where one of a fleet's claims stands, as
//! `plan` answers it under `mine`.
//!
//! `Status` lives in the core, which Flux checks and which so takes no
//! `agent-help`: its table is here. The two matches below are exhaustive
//! both ways, so a status added to, renamed in or removed from the core does
//! not compile until this table follows; the test holds the names equal.

use afkconvoy_core::plan::Status;

/// Where one of a fleet's claims stands; each name is how `plan` prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, agent_help::Help)]
pub enum ClaimStatus {
    /// No open PR closes the issue: its worker is still at it, or stopped.
    NoPr,
    /// An open PR closes it, and that PR is not on the base branch yet. In a bound repo, only under a landing worker's claim.
    Landing,
    /// The landing train put its work on the base branch, and the landing was cut short before the issue was closed: a tick closes it, then settles the claim.
    Landed,
    /// The issue is closed: a tick settles the claim and releases it.
    Closed,
    /// GitHub says the issue was deleted, or transferred to another repo: nothing is left to settle, and the claim is let go.
    Gone,
    /// In a bound repo, a claim on an open issue that is not a landing worker's, whatever PR is open: it was dispatched to a stage, and what that stage declares ends it.
    AtStage,
}

impl From<Status> for ClaimStatus {
    fn from(status: Status) -> Self {
        match status {
            Status::NoPr => Self::NoPr,
            Status::Landing => Self::Landing,
            Status::Landed => Self::Landed,
            Status::Closed => Self::Closed,
            Status::Gone => Self::Gone,
            Status::AtStage => Self::AtStage,
        }
    }
}

impl From<ClaimStatus> for Status {
    fn from(status: ClaimStatus) -> Self {
        match status {
            ClaimStatus::NoPr => Self::NoPr,
            ClaimStatus::Landing => Self::Landing,
            ClaimStatus::Landed => Self::Landed,
            ClaimStatus::Closed => Self::Closed,
            ClaimStatus::Gone => Self::Gone,
            ClaimStatus::AtStage => Self::AtStage,
        }
    }
}

#[cfg(test)]
mod tests {
    use agent_help::Table;

    use super::*;

    /// Each entry is named as `plan` prints its status, and the table holds
    /// every status once: the core's statuses, each through the table and
    /// back, are as many as the entries and come back themselves.
    #[test]
    fn each_entry_is_named_as_plan_prints_its_status() {
        let statuses = [
            Status::NoPr,
            Status::Landing,
            Status::Landed,
            Status::Closed,
            Status::Gone,
            Status::AtStage,
        ];
        assert_eq!(statuses.len(), ClaimStatus::ENTRIES.len());
        for status in statuses {
            let entry = ClaimStatus::from(status);
            assert_eq!(Status::from(entry), status);
            assert_eq!(serde_json::to_value(status).unwrap(), entry.entry().name);
        }
    }
}

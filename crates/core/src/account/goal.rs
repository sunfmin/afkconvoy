//! How a worker's goal went, as its work account records it
//! (sunfmin/control#548): what its session record and its harness's goal
//! store say, read while the worker's worktree still holds them. Only a
//! worker whose harness keeps a goal — Codex — carries one.

use serde::{Deserialize, Serialize};

use crate::harness::{GoalRow, GoalSession};

/// What a goal's status is recorded as when its session's goal was set and
/// its store holds no row for it: the fleet ran `/goal clear` once the
/// worker's verdict stood, which removes the row.
pub const CLEARED: &str = "cleared";

/// A worker's goal, as its account records it. A fact its records do not
/// say is null.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerGoal {
    /// Whether its session was set a goal.
    pub set: bool,
    /// The objective it was set: the brief pointer `/goal` was typed with.
    pub objective: Option<String>,
    /// The turns with `turn_trigger: goal`, the one its setting began
    /// among them.
    pub goal_turns: Option<u64>,
    /// Of those, the turns begun after the worker's verdict stood: a worker
    /// that marked its goal complete at its verdict takes none.
    pub goal_turns_after_verdict: Option<u64>,
    /// The store's final status of the goal, or `cleared`.
    pub status: Option<String>,
    /// The tokens the goal used against its budget, as the store counts
    /// them: uncached input and output.
    pub tokens_used: Option<u64>,
    pub token_budget: Option<u64>,
    /// The session record these were read from.
    pub session: Option<String>,
}

impl WorkerGoal {
    /// The goal `said` and its store's `row` record, read from the session
    /// record at `session`. `row` is `None` where the store could not be
    /// read, `Some(None)` where it holds no row. Goal turns after the
    /// verdict are counted from `verdict_at`, where it is known.
    pub fn of(
        said: &GoalSession,
        session: &str,
        row: Option<Option<GoalRow>>,
        verdict_at: Option<u64>,
    ) -> WorkerGoal {
        let set = said.objective.is_some();
        let status = match row {
            Some(Some(row)) => Some(row.status.key().to_string()),
            Some(None) if set => Some(CLEARED.to_string()),
            Some(None) | None => None,
        };
        let row = row.flatten();
        WorkerGoal {
            set,
            objective: said.objective.clone(),
            goal_turns: Some(said.turns.len() as u64),
            goal_turns_after_verdict: verdict_at
                .map(|verdict| said.turns.iter().filter(|at| **at > verdict).count() as u64),
            status,
            tokens_used: row.map(|row| row.tokens_used),
            token_budget: row.and_then(|row| row.token_budget),
            session: Some(session.to_string()),
        }
    }

    /// How this goal misbehaved, as evidence a person can check: it ended at
    /// its budget, or it kept taking goal turns after the worker's verdict.
    /// `None`: it did neither, or its records do not say.
    pub fn misbehaved(&self) -> Option<String> {
        let mut said = vec![];
        if self.status.as_deref() == Some(crate::harness::Goal::BudgetLimited.key()) {
            let used = self.tokens_used.map_or("unknown".to_string(), |n| n.to_string());
            let budget = self.token_budget.map_or("unknown".to_string(), |n| n.to_string());
            said.push(format!("its goal ended `budget_limited`, {used} of {budget} tokens used"));
        }
        if let Some(after @ 1..) = self.goal_turns_after_verdict {
            said.push(format!("it took {after} goal turns after its verdict"));
        }
        if said.is_empty() {
            return None;
        }
        let session = self.session.as_deref().unwrap_or("unknown");
        Some(format!("{}; session record `{session}`", said.join(" and ")))
    }

    /// The account's human words for this goal.
    pub(super) fn words(&self) -> String {
        if !self.set {
            return "no goal set".into();
        }
        let status = self.status.as_deref().unwrap_or("unknown");
        let turns = self.goal_turns.map_or("unknown".to_string(), |n| n.to_string());
        let mut words = format!("goal `{status}`, {turns} goal turns");
        if let Some(after) = self.goal_turns_after_verdict {
            words.push_str(&format!(" ({after} after its verdict)"));
        }
        match (self.tokens_used, self.token_budget) {
            (Some(used), Some(budget)) => words.push_str(&format!(", {used} of {budget} tokens")),
            (Some(used), None) => words.push_str(&format!(", {used} tokens, no budget")),
            _ => words.push_str(", goal tokens unknown"),
        }
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Goal;

    const SESSION: &str = "/kept/codex/sessions/2026/11/03/rollout-t.jsonl";

    fn said(turns: &[u64]) -> GoalSession {
        GoalSession {
            thread: Some("t".into()),
            objective: Some("Read /b.md and carry it out".into()),
            turns: turns.to_vec(),
        }
    }

    fn row(status: Goal, used: u64) -> Option<Option<GoalRow>> {
        Some(Some(GoalRow { status, tokens_used: used, token_budget: Some(1_000_000) }))
    }

    #[test]
    fn a_goal_that_completes_at_its_verdict_is_recorded_and_did_not_misbehave() {
        let goal =
            WorkerGoal::of(&said(&[100, 200]), SESSION, row(Goal::Complete, 24_959), Some(300));
        assert_eq!(
            goal,
            WorkerGoal {
                set: true,
                objective: Some("Read /b.md and carry it out".into()),
                goal_turns: Some(2),
                goal_turns_after_verdict: Some(0),
                status: Some("complete".into()),
                tokens_used: Some(24_959),
                token_budget: Some(1_000_000),
                session: Some(SESSION.into()),
            }
        );
        assert_eq!(goal.misbehaved(), None);
        assert_eq!(
            goal.words(),
            "goal `complete`, 2 goal turns (0 after its verdict), 24959 of 1000000 tokens"
        );
    }

    #[test]
    fn a_goal_at_its_budget_misbehaved_and_says_its_evidence() {
        let goal = WorkerGoal::of(
            &said(&[100, 200, 400]),
            SESSION,
            row(Goal::BudgetLimited, 1_000_412),
            None,
        );
        assert_eq!(goal.status.as_deref(), Some("budget_limited"));
        assert_eq!(goal.goal_turns_after_verdict, None, "no verdict, none counted");
        assert_eq!(
            goal.misbehaved().unwrap(),
            format!(
                "its goal ended `budget_limited`, 1000412 of 1000000 tokens used; session record `{SESSION}`"
            )
        );
    }

    #[test]
    fn a_goal_the_fleet_cleared_is_cleared_and_its_turns_after_the_verdict_are_evidence() {
        let goal = WorkerGoal::of(&said(&[100, 350, 420]), SESSION, Some(None), Some(300));
        assert_eq!(goal.status.as_deref(), Some(CLEARED));
        assert_eq!(goal.tokens_used, None, "a cleared goal's row is gone");
        assert_eq!(goal.goal_turns_after_verdict, Some(2));
        assert_eq!(
            goal.misbehaved().unwrap(),
            format!("it took 2 goal turns after its verdict; session record `{SESSION}`")
        );
        let unread = WorkerGoal::of(&said(&[100]), SESSION, None, Some(300));
        assert_eq!(unread.status, None, "an unreadable store says no status");
    }

    #[test]
    fn a_session_set_no_goal_says_so_and_no_row_is_no_clearing() {
        let none = GoalSession { thread: Some("t".into()), objective: None, turns: vec![] };
        let goal = WorkerGoal::of(&none, SESSION, Some(None), Some(300));
        assert!(!goal.set);
        assert_eq!(goal.status, None);
        assert_eq!(goal.words(), "no goal set");
        assert_eq!(goal.misbehaved(), None);
    }
}

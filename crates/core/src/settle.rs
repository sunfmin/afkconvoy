//! Settling a claim whose work has landed: what only the tick can do for it,
//! and in which order.

use flux_rs::attrs::*;

use crate::plan::PullRequest;
use crate::train::Joined;

/// One step of settling a landed claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleStep {
    /// Close the issue a landing put on the base branch and was cut short of
    /// closing.
    CloseIssue,
    /// Close the PR the train landed that GitHub still shows open.
    ClosePr,
    /// Delete the issue's own branches on the remote that are on the base
    /// branch: the landing deletes them only once GitHub shows the PR merged.
    DropBranches,
    /// Remove the worker's worktree.
    RemoveWorktree,
    /// Fast-forward the fleet's own checkout to what landed.
    FollowBase,
    /// Write the status board: merged, or closed with nothing landed.
    Board,
    /// Delete the claim.
    Release,
}

/// The steps of one settling as they are decided.
#[refined_by(closed: bool, dropped: bool, removed: bool, boarded: bool)]
struct SettleOrder {
    #[field(bool[closed])]
    closed: bool,
    #[field(bool[dropped])]
    dropped: bool,
    #[field(bool[removed])]
    removed: bool,
    #[field(bool[boarded])]
    boarded: bool,
    steps: Vec<SettleStep>,
}

impl SettleOrder {
    #[spec(fn(closed: bool) -> SettleOrder[closed, false, false, false])]
    fn new(closed: bool) -> SettleOrder {
        SettleOrder { closed, dropped: false, removed: false, boarded: false, steps: Vec::new() }
    }

    #[spec(fn(self: &mut Self[@o]) ensures self: Self[{ closed: true, ..o }])]
    fn close_issue(&mut self) {
        self.steps.push(SettleStep::CloseIssue);
        self.closed = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.closed)]
    fn close_pr(&mut self) {
        self.steps.push(SettleStep::ClosePr);
    }

    /// Before the worktree goes: its branch is one of the issue's own only
    /// while orca still links the two.
    #[spec(fn(self: &mut Self[@o]) requires o.closed && !o.removed ensures self: Self[{ dropped: true, ..o }])]
    fn drop_branches(&mut self) {
        self.steps.push(SettleStep::DropBranches);
        self.dropped = true;
    }

    /// An open issue's worktree is never touched: it may hold work.
    #[spec(fn(self: &mut Self[@o]) requires o.closed && o.dropped ensures self: Self[{ removed: true, ..o }])]
    fn remove_worktree(&mut self) {
        self.steps.push(SettleStep::RemoveWorktree);
        self.removed = true;
    }

    #[spec(fn(self: &mut Self[@o]))]
    fn follow_base(&mut self) {
        self.steps.push(SettleStep::FollowBase);
    }

    /// The board's last word, once the issue is closed: it never says merged
    /// over an issue still open.
    #[spec(fn(self: &mut Self[@o]) requires o.closed ensures self: Self[{ boarded: true, ..o }])]
    fn board(&mut self) {
        self.steps.push(SettleStep::Board);
        self.boarded = true;
    }

    /// The claim goes last: a settling that failed on the way still holds it,
    /// and the next tick settles it again.
    #[spec(fn(self: &mut Self[@o]) requires o.closed && o.removed && o.boarded)]
    fn release(&mut self) {
        self.steps.push(SettleStep::Release);
    }
}

/// The steps that settle a claim whose issue's work is on the base branch.
/// `closed`: the issue is closed already — it is not when the landing was cut
/// short after its push.
pub fn settle_steps(closed: bool) -> Vec<SettleStep> {
    let mut order = SettleOrder::new(closed);
    if !order.closed {
        order.close_issue();
    }
    order.close_pr();
    order.drop_branches();
    order.remove_worktree();
    order.follow_base();
    order.board();
    order.release();
    order.steps
}

/// Was the landing made at `landed_at` made under the claim taken at
/// `claimed_at`? A PR landed long ago closes the issue too — one since
/// reopened and claimed again: only a landing no earlier than the claim is
/// this claim's. Both times are the fleet's own clock.
#[spec(fn(landed_at: u64, claimed_at: u64) -> bool[landed_at >= claimed_at])]
pub fn landed_under(landed_at: u64, claimed_at: u64) -> bool {
    landed_at >= claimed_at
}

/// Is `join` — the newest joining on the base branch that closes a claimed,
/// still open issue — a landing cut short after its push, to be finished from
/// the base branch itself? Not when it was made before the claim, and not when
/// the issue's open PR is another PR or has another head than the one that
/// landed: its worker pushed more since, and joins again with it.
pub fn cut_landing(
    join: &Joined,
    landed_at: u64,
    claimed_at: Option<u64>,
    open_pr: Option<&PullRequest>,
) -> bool {
    let under = match claimed_at {
        Some(claimed_at) => landed_under(landed_at, claimed_at),
        None => false,
    };
    let same = match open_pr {
        Some(pr) => pr.number == join.pr && pr.head_sha == join.head,
        None => true,
    };
    under && same
}

/// The comment a landed PR is closed with when GitHub did not mark it merged.
pub fn landed_comment(commit: &str, base_branch: &str) -> String {
    format!("Landed on `{base_branch}` by the landing train as {commit}.")
}

/// How the fleet's own checkout follows the base branch to `tip`. Only ever a
/// fast-forward, which both moves refuse to be anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// The local branch is at the tip.
    UpToDate,
    /// The branch is checked out: merge the tip, fast-forward only.
    Merge,
    /// It is not: move its ref by a fetch.
    Fetch,
}

pub fn follow(local: &str, tip: &str, checked_out: bool) -> Follow {
    if local == tip {
        Follow::UpToDate
    } else if checked_out {
        Follow::Merge
    } else {
        Follow::Fetch
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A claim deleted with its worktree still there: a crash right after
    /// leaves a worktree no claim leads back to.
    #[should_fail]
    fn a_claim_released_before_its_worktree_is_removed() {
        let mut order = SettleOrder::new(true);
        order.release();
    }

    /// A claim deleted while its status board still says it is on the train.
    #[should_fail]
    fn a_claim_released_before_the_board_says_merged() {
        let mut order = SettleOrder::new(true);
        order.drop_branches();
        order.remove_worktree();
        order.release();
    }

    /// The worktree removed with the issue's branches still on the remote:
    /// the branch it was on is then no longer known for the issue's own.
    #[should_fail]
    fn a_worktree_removed_before_its_branch_is_dropped() {
        let mut order = SettleOrder::new(true);
        order.remove_worktree();
    }

    /// The branch of an open issue deleted: it may hold work.
    #[should_fail]
    fn an_open_issues_branches_dropped() {
        let mut order = SettleOrder::new(false);
        order.drop_branches();
    }

    /// The board saying merged over an issue still open.
    #[should_fail]
    fn a_board_that_says_merged_over_an_open_issue() {
        let mut order = SettleOrder::new(false);
        order.board();
    }

    /// The worktree of an open issue removed: it may hold work.
    #[should_fail]
    fn an_open_issues_worktree_removed() {
        let mut order = SettleOrder::new(false);
        order.remove_worktree();
    }

    /// A landing older than the claim taken for the claim's own.
    #[should_fail]
    #[spec(fn(landed_at: u64, claimed_at: u64{claimed_at > landed_at}) -> bool[true])]
    fn a_landing_older_than_the_claim(landed_at: u64, claimed_at: u64) -> bool {
        landed_under(landed_at, claimed_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_claim_is_deleted_last() {
        use SettleStep::*;
        assert_eq!(
            settle_steps(true),
            [ClosePr, DropBranches, RemoveWorktree, FollowBase, Board, Release]
        );
        assert_eq!(
            settle_steps(false),
            [CloseIssue, ClosePr, DropBranches, RemoveWorktree, FollowBase, Board, Release]
        );
    }

    #[test]
    fn a_cut_landing_is_this_claims_and_of_the_head_that_landed() {
        let join = Joined { issue: 7, pr: 70, commit: "m".into(), head: "h".into() };
        let pr = |number: u64, head: &str| PullRequest {
            number,
            title: String::new(),
            head_branch: "b".into(),
            head_sha: head.into(),
            closes: vec![7],
        };
        assert!(cut_landing(&join, 500, Some(500), None), "GitHub shows the PR merged");
        assert!(cut_landing(&join, 500, Some(100), Some(&pr(70, "h"))));
        assert!(!cut_landing(&join, 99, Some(100), None), "landed before the claim was taken");
        assert!(!cut_landing(&join, 500, None, None), "a claim that names nobody");
        assert!(!cut_landing(&join, 500, Some(100), Some(&pr(70, "newer"))));
        assert!(!cut_landing(&join, 500, Some(100), Some(&pr(71, "h"))));
        assert!(landed_comment("abc", "main").contains("`main`"));
    }

    #[test]
    fn the_checkout_follows_by_fast_forward_only() {
        assert_eq!(follow("a", "a", true), Follow::UpToDate);
        assert_eq!(follow("a", "b", true), Follow::Merge);
        assert_eq!(follow("a", "b", false), Follow::Fetch);
    }
}

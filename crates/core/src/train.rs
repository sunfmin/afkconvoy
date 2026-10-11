//! The landing train: an append-only line of commits ahead of the base branch.
//! Each PR on it is one merge commit whose message names the PR and the issue
//! it closes. Everything asked about a train — what is on it, what it landed —
//! is read off that line and the base branch, and nothing off a marker.

use flux_rs::attrs::*;

use crate::claims::NAMESPACE;
use crate::plan::PullRequest;
use crate::record::fleet_number;

/// The train's two refs on the remote, for the base branch it lands on.
pub struct TrainRefs {
    /// The train itself: its tip.
    pub line: String,
    /// The commit of the line the train worker last gated red.
    pub red: String,
}

pub fn train_refs(base_branch: &str) -> TrainRefs {
    TrainRefs {
        line: format!("{NAMESPACE}/train/line/{base_branch}"),
        red: format!("{NAMESPACE}/train/red/{base_branch}"),
    }
}

/// The train's line as the remote has it.
pub struct Line<'a> {
    pub tip: &'a str,
    /// The base branch already holds the tip: the train landed.
    pub on_base: bool,
}

/// What the next PR joins onto. A line the base branch already holds — the
/// train landed, or nothing ever joined — is no train: the next PR joins onto
/// the base branch's tip, and pushing that commit fast-forwards the line all
/// the same.
pub fn joins_onto<'a>(line: Option<Line<'a>>, base: &'a str) -> &'a str {
    match line {
        Some(line) if !line.on_base => line.tip,
        _ => base,
    }
}

/// One PR's joining of the train.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub issue: u64,
    pub pr: u64,
    /// The merge commit on the train.
    pub commit: String,
    /// The PR's head it merged: the commit's second parent.
    pub head: String,
}

/// The message of the merge commit a PR joins the train with — which is how
/// what is on a train is read back (`read_joins`).
pub fn join_message(title: &str, pr: u64, issue: u64) -> String {
    let title = title.trim();
    let title = if title.is_empty() { format!("PR {pr}") } else { title.to_string() };
    format!("{title} (#{pr})\n\nCloses #{issue}\n")
}

/// One commit as `git log` gives it: its sha, its parents, its whole message.
pub struct Logged<'a> {
    pub sha: &'a str,
    pub parents: &'a str,
    pub message: &'a str,
}

/// The joinings among `commits`, in the order given. A joining is a merge
/// commit whose subject ends ` (#<pr>)` and whose message has a line
/// `Closes #<issue>`. A commit with one parent is never one, whatever its
/// message says — a fix titled `… (#12)` is a fix.
pub fn read_joins(commits: &[Logged]) -> Vec<Joined> {
    let mut joins = vec![];
    for commit in commits {
        let parents: Vec<&str> = commit.parents.split_whitespace().collect();
        let [_, head] = parents.as_slice() else { continue };
        let subject = commit.message.lines().next().unwrap_or("");
        let pr = subject
            .strip_suffix(')')
            .and_then(|rest| rest.rsplit_once(" (#"))
            .and_then(|(_, number)| fleet_number(number));
        let issue = commit
            .message
            .lines()
            .find_map(|line| line.strip_prefix("Closes #").and_then(fleet_number));
        if let (Some(pr), Some(issue)) = (pr, issue) {
            joins.push(Joined {
                issue,
                pr,
                commit: commit.sha.to_string(),
                head: head.to_string(),
            });
        }
    }
    joins
}

/// The joinings of the open `prs` whose head is on the train: of the joinings
/// on the train and not yet on the base branch (`joins`, oldest first), a PR's
/// newest one, when it merged the head the PR has now. A PR whose worker
/// pushed again since it joined is not on the train: it joins again.
pub fn on_train<'a>(joins: &'a [Joined], prs: &[PullRequest]) -> Vec<&'a Joined> {
    let mut newest: Vec<&Joined> = vec![];
    for join in joins.iter().rev() {
        if !newest.iter().any(|newer| newer.pr == join.pr) {
            newest.push(join);
        }
    }
    newest.retain(|join| prs.iter().any(|pr| pr.number == join.pr && pr.head_sha == join.head));
    newest.reverse();
    newest
}

/// One step of a PR joining the train.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinStep {
    /// Push what the worker committed to the PR's branch.
    PushBranch,
    /// Merge the PR's head onto the train's tip, as one commit.
    Merge,
    /// Push that commit as the train's new tip: the train's one lock.
    PushLine,
}

/// The steps of one join as they are decided. `pushed`: the PR's branch on the
/// remote is at the head being joined.
#[refined_by(pushed: bool, merged: bool)]
struct JoinOrder {
    #[field(bool[pushed])]
    pushed: bool,
    #[field(bool[merged])]
    merged: bool,
    steps: Vec<JoinStep>,
}

impl JoinOrder {
    #[spec(fn(pushed: bool) -> JoinOrder[pushed, false])]
    fn new(pushed: bool) -> JoinOrder {
        JoinOrder { pushed, merged: false, steps: Vec::new() }
    }

    #[spec(fn(self: &mut Self[@o]) ensures self: Self[true, o.merged])]
    fn push_branch(&mut self) {
        self.steps.push(JoinStep::PushBranch);
        self.pushed = true;
    }

    /// What joins the train is the head GitHub shows for the PR: a head only
    /// this worktree has would land work the PR never showed.
    #[spec(fn(self: &mut Self[@o]) requires o.pushed ensures self: Self[o.pushed, true])]
    fn merge(&mut self) {
        self.steps.push(JoinStep::Merge);
        self.merged = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.merged)]
    fn push_line(&mut self) {
        self.steps.push(JoinStep::PushLine);
    }
}

/// The steps that put a PR's head on the train. `pushed`: the PR's branch on
/// the remote is already at that head.
pub fn join_steps(pushed: bool) -> Vec<JoinStep> {
    let mut order = JoinOrder::new(pushed);
    if !order.pushed {
        order.push_branch();
    }
    order.merge();
    order.push_line();
    order.steps
}

/// How often a join is made again on a train that moved under it before the
/// command gives up and is run again.
pub const PUSH_TRIES: u32 = 8;

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A head joined to the train that the PR's branch does not have.
    #[should_fail]
    fn a_head_joined_before_it_is_pushed() {
        let mut order = JoinOrder::new(false);
        order.merge();
    }

    /// A tip pushed with nothing merged onto it.
    #[should_fail]
    fn a_line_pushed_without_a_join_commit() {
        let mut order = JoinOrder::new(true);
        order.push_line();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, head: &str) -> PullRequest {
        PullRequest {
            number,
            title: String::new(),
            head_branch: format!("b{number}"),
            head_sha: head.to_string(),
            closes: vec![],
        }
    }

    #[test]
    fn a_join_message_reads_back() {
        let message = join_message("  Add plan (really) ", 31, 7);
        assert_eq!(message, "Add plan (really) (#31)\n\nCloses #7\n");
        let commits = [
            Logged { sha: "m1", parents: "t0 h31", message: &message },
            Logged { sha: "f1", parents: "m1", message: "fix the train (#31)\n\nCloses #7\n" },
            Logged { sha: "m2", parents: "f1 h40", message: "Merge branch 'main'" },
            Logged { sha: "m3", parents: "m2 h32", message: &join_message("", 32, 8) },
        ];
        let joins = read_joins(&commits);
        assert_eq!(joins.len(), 2, "a one-parent commit and an ordinary merge are no joining");
        assert_eq!(joins[0], Joined { issue: 7, pr: 31, commit: "m1".into(), head: "h31".into() });
        assert_eq!((joins[1].pr, joins[1].issue, joins[1].head.as_str()), (32, 8, "h32"));
    }

    #[test]
    fn a_pr_is_on_the_train_only_at_its_current_head() {
        let join = |pr: u64, commit: &str, head: &str| Joined {
            issue: pr - 20,
            pr,
            commit: commit.into(),
            head: head.into(),
        };
        let joins = [
            join(31, "m1", "old"),
            join(32, "m2", "h32"),
            join(31, "m3", "new"),
            join(33, "m4", "h33"),
        ];
        let on = on_train(&joins, &[pr(31, "new"), pr(32, "moved"), pr(34, "h34")]);
        assert_eq!(on.iter().map(|j| j.commit.as_str()).collect::<Vec<_>>(), ["m3"]);
        let stale = on_train(&joins[..2], &[pr(31, "new")]);
        assert!(stale.is_empty(), "its worker pushed again: it joins again");
    }

    #[test]
    fn the_branch_is_pushed_before_its_head_joins() {
        use JoinStep::*;
        assert_eq!(join_steps(false), [PushBranch, Merge, PushLine]);
        assert_eq!(join_steps(true), [Merge, PushLine]);
    }

    #[test]
    fn a_line_the_base_branch_holds_is_no_train() {
        assert_eq!(joins_onto(None, "base"), "base");
        assert_eq!(joins_onto(Some(Line { tip: "t", on_base: true }), "base"), "base");
        assert_eq!(joins_onto(Some(Line { tip: "t", on_base: false }), "base"), "t");
    }

    #[test]
    fn a_trains_refs_are_named_for_its_base_branch() {
        let refs = train_refs("release/1.x");
        assert_eq!(refs.line, "refs/convoy/train/line/release/1.x");
        assert_eq!(refs.red, "refs/convoy/train/red/release/1.x");
    }
}

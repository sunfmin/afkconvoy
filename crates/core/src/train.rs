//! The landing train: an append-only line of commits ahead of the base branch.
//! Each PR on it is one merge commit whose message names the PR and the issue
//! it closes. Everything asked about a train — what is on it, what it landed —
//! is read off that line and the base branch, and nothing off a marker.

use flux_rs::attrs::*;

use crate::claims::NAMESPACE;
use crate::plan::PullRequest;
use crate::record::fleet_number;
use crate::worker::WorkerState;

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

/// How a run of the train worker's `land --train` ended. Only `Landed` put
/// anything on the base branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainOutcome {
    /// The train is on the base branch, up to the commit gated.
    Landed,
    /// Nothing is on the train.
    Idle,
    /// The gate is red on the train's tip.
    GateRed,
    /// The train's tip, or the base branch, conflicts with what the train's
    /// worktree holds.
    Conflict,
    /// The base branch moved while the gate ran: the push was refused.
    TargetMoved,
}

impl TrainOutcome {
    pub fn word(self) -> &'static str {
        match self {
            TrainOutcome::Landed => "landed",
            TrainOutcome::Idle => "idle",
            TrainOutcome::GateRed => "gate_red",
            TrainOutcome::Conflict => "conflict",
            TrainOutcome::TargetMoved => "target_moved",
        }
    }
}

/// Does the train's worktree keep the commits it has — the train's tip merged
/// into them — or start over on that tip? It keeps them only when they are
/// work on THIS train: a fix made here while another PR joined. `ahead`: the
/// train holds something the base branch lacks. `own`: the worktree has
/// commits the train's tip lacks. `on_this_train`: those commits were made on
/// top of this train, not of one that has landed or was abandoned.
#[spec(fn(ahead: bool, own: bool, on_this_train: bool) -> bool[ahead && own && on_this_train])]
pub fn keeps_its_commits(ahead: bool, own: bool, on_this_train: bool) -> bool {
    ahead && own && on_this_train
}

/// Is the train being repaired? — the commit its worker last gated red is on
/// the line and not yet on the base branch. `red`: (the base branch holds
/// that commit, the line holds it), when one is on record.
#[spec(fn(ahead: bool, red: Option<(bool, bool)>) -> bool{v: v => ahead})]
pub fn under_repair(ahead: bool, red: Option<(bool, bool)>) -> bool {
    match red {
        Some((on_base, on_line)) => ahead && !on_base && on_line,
        None => false,
    }
}

/// What a worker's branch takes in before its own gate run while the train is
/// under repair: the newest commit of the line below the red one with a green
/// run on record — `below`, newest first, each with whether it has one — else
/// the base branch. A worker's gate must not be red for a reason that is not
/// its own.
pub fn last_green<'a>(below: &[(&'a str, bool)], base: &'a str) -> &'a str {
    for (commit, recorded) in below {
        if *recorded {
            return commit;
        }
    }
    base
}

/// The joinings a landing finished, of those its commit put on the base
/// branch: all but a PR still open on another head than the one that joined —
/// its worker pushed more since, and joins again with it.
pub fn finished<'a>(joins: &'a [Joined], open: &[PullRequest]) -> Vec<&'a Joined> {
    let mut done = vec![];
    for join in joins {
        let moved = open.iter().any(|pr| pr.number == join.pr && pr.head_sha != join.head);
        if !moved {
            done.push(join);
        }
    }
    done
}

/// The name the train's worktree is created under — one per repo on a
/// machine, kept across runs with what its gate runs built — and so, with a
/// `-<k>` when the name was taken, the name of its directory.
pub const TRAIN_WORKTREE: &str = "convoy-train";

/// Is a worktree directory of this name the train's? The train's worktree is
/// linked to no issue, so it is known by the name it was created under.
pub fn is_train_worktree(directory: &str) -> bool {
    match directory.strip_prefix(TRAIN_WORKTREE) {
        Some("") => true,
        Some(rest) => match rest.strip_prefix('-') {
            Some(k) => !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()),
            None => false,
        },
        None => false,
    }
}

/// What the tick does about the train worker while a PR of its fleet is on
/// the train.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tend {
    /// The worker is at work — or it stopped on the very tip it was last told
    /// about: its silence is not this decision's.
    Leave,
    /// No worker is there: start one in the train's worktree.
    Start,
    /// The worker stopped, and the train holds something it was not told
    /// about: tell it.
    Tell,
}

/// `told`: the train's tip the worker in the worktree was last told about;
/// `None` when nobody was told anything there — whatever terminal is in it is
/// no train worker.
pub fn tend(worker: WorkerState, told: Option<&str>, tip: &str) -> Tend {
    match (worker, told) {
        (WorkerState::Busy, _) => Tend::Leave,
        (WorkerState::Gone, _) | (_, None) => Tend::Start,
        (WorkerState::Idle { .. }, Some(told)) => {
            if told == tip {
                Tend::Leave
            } else {
                Tend::Tell
            }
        }
    }
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

    /// A worktree's commits kept on a train they were never made on.
    #[should_fail]
    #[spec(fn(own: bool) -> bool[true])]
    fn commits_of_a_landed_train_kept(own: bool) -> bool {
        keeps_its_commits(true, own, false)
    }

    /// A repair assumed of a train that holds nothing.
    #[should_fail]
    #[spec(fn(red: Option<(bool, bool)>) -> bool[true])]
    fn a_repair_of_no_train(red: Option<(bool, bool)>) -> bool {
        under_repair(false, red)
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
    fn a_worker_under_a_red_train_gates_on_its_last_green_commit() {
        assert!(under_repair(true, Some((false, true))));
        assert!(!under_repair(true, Some((true, true))), "the red commit landed: it was fixed");
        assert!(!under_repair(true, Some((false, false))), "red on a train that is gone");
        assert!(!under_repair(true, None));
        assert_eq!(last_green(&[("c3", false), ("c2", true), ("c1", true)], "base"), "c2");
        assert_eq!(last_green(&[("c3", false)], "base"), "base");
        assert_eq!(last_green(&[], "base"), "base");
    }

    #[test]
    fn a_pr_that_moved_on_is_not_finished_by_the_landing() {
        let join = |pr: u64, head: &str| Joined {
            issue: pr - 20,
            pr,
            commit: format!("m{pr}"),
            head: head.into(),
        };
        let joins = [join(31, "h31"), join(32, "h32"), join(33, "h33")];
        // #31 is still open at the head that joined, #32 was pushed to since, #33 is merged
        let done = finished(&joins, &[pr(31, "h31"), pr(32, "newer")]);
        assert_eq!(done.iter().map(|j| j.pr).collect::<Vec<_>>(), [31, 33]);
    }

    #[test]
    fn the_trains_worktree_keeps_only_work_on_this_train() {
        assert!(keeps_its_commits(true, true, true));
        assert!(!keeps_its_commits(false, true, true));
        assert!(!keeps_its_commits(true, false, true));
        assert!(!keeps_its_commits(true, true, false));
        assert_eq!(TrainOutcome::TargetMoved.word(), "target_moved");
    }

    #[test]
    fn the_train_worker_is_started_told_or_left() {
        let idle = WorkerState::Idle { quiet: Some(9) };
        assert_eq!(tend(WorkerState::Gone, Some("t1"), "t1"), Tend::Start);
        assert_eq!(tend(idle, None, "t1"), Tend::Start, "a bare shell is no train worker");
        assert_eq!(tend(WorkerState::Busy, Some("t0"), "t1"), Tend::Leave);
        assert_eq!(tend(idle, Some("t0"), "t1"), Tend::Tell);
        assert_eq!(tend(idle, Some("t1"), "t1"), Tend::Leave);
    }

    #[test]
    fn the_trains_worktree_is_known_by_its_name() {
        for name in ["convoy-train", "convoy-train-2", "convoy-train-13"] {
            assert!(is_train_worktree(name), "{name}");
        }
        for name in ["convoy-trainer", "convoy-train-", "convoy-train-x", "issue-7-convoy-train"] {
            assert!(!is_train_worktree(name), "{name}");
        }
    }

    #[test]
    fn a_trains_refs_are_named_for_its_base_branch() {
        let refs = train_refs("release/1.x");
        assert_eq!(refs.line, "refs/convoy/train/line/release/1.x");
        assert_eq!(refs.red, "refs/convoy/train/red/release/1.x");
    }
}

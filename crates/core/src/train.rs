//! The landing train: an append-only line of commits ahead of the base branch.
//! Each PR on it is one merge commit whose message names the PR and the issue
//! it closes — or, of an inspection's change of the repo, the inspection that
//! made it. Everything asked about a train — what is on it, what it landed —
//! is read off that line and the base branch, and nothing off a marker.

use flux_rs::attrs::*;

use crate::layout::Layout;
use crate::plan::PullRequest;
use crate::record::fleet_number;
use crate::worker::{WorkerState, is_worktree_named};

/// The train's two refs on the remote, for the base branch it lands on.
pub struct TrainRefs {
    /// The train itself: its tip.
    pub line: String,
    /// The commit of the line the train worker last gated red.
    pub red: String,
}

pub fn train_refs(layout: Layout, base_branch: &str) -> TrainRefs {
    TrainRefs {
        line: layout.under(&format!("train/line/{base_branch}")),
        red: layout.under(&format!("train/red/{base_branch}")),
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

/// Whose a PR on the train is, as its join commit names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Of {
    /// The issue it closes: a worker's PR.
    Issue(u64),
    /// The inspection on the host so named, which made a change of the repo
    /// itself, on no issue (docs/adr/0026).
    Inspection(String),
    /// The cut on the host so named, whose PR sets the version it makes, on
    /// no issue (docs/adr/0027).
    Cut(String),
}

/// The line of a join commit that names an inspection's PR, the host after it.
const BY_INSPECTION: &str = "By an inspection on ";
/// The line of a join commit that names a cut's PR, the host after it.
const BY_CUT: &str = "By a cut on ";

/// One PR's joining of the train.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub of: Of,
    pub pr: u64,
    /// The merge commit on the train.
    pub commit: String,
    /// The PR's head it merged: the commit's second parent.
    pub head: String,
}

impl Joined {
    /// The issue the PR closes: none of an inspection's or a cut's.
    pub fn issue(&self) -> Option<u64> {
        match self.of {
            Of::Issue(issue) => Some(issue),
            Of::Inspection(_) | Of::Cut(_) => None,
        }
    }
}

/// The message of the merge commit a PR joins the train with — which is how
/// what is on a train is read back (`read_joins`).
pub fn join_message(title: &str, pr: u64, of: &Of) -> String {
    let title = title.trim();
    let title = if title.is_empty() { format!("PR {pr}") } else { title.to_string() };
    match of {
        Of::Issue(issue) => format!("{title} (#{pr})\n\nCloses #{issue}\n"),
        Of::Inspection(host) => format!("{title} (#{pr})\n\n{BY_INSPECTION}{host}\n"),
        Of::Cut(host) => format!("{title} (#{pr})\n\n{BY_CUT}{host}\n"),
    }
}

/// One commit as `git log` gives it: its sha, its parents, its whole message.
pub struct Logged<'a> {
    pub sha: &'a str,
    pub parents: &'a str,
    pub message: &'a str,
}

/// The joinings among `commits`, in the order given. A joining is a merge
/// commit whose subject ends ` (#<pr>)` and whose message has a line
/// `Closes #<issue>` — or, of an inspection's PR, `By an inspection on
/// <host>`, and of a cut's, `By a cut on <host>`. A commit with one parent is
/// never one, whatever its message says — a fix titled `… (#12)` is a fix.
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
        let by = |by: &str| {
            let host = commit.message.lines().find_map(|line| line.strip_prefix(by));
            host.map(str::trim).filter(|host| !host.is_empty()).map(str::to_string)
        };
        let of = issue
            .map(Of::Issue)
            .or_else(|| by(BY_INSPECTION).map(Of::Inspection))
            .or_else(|| by(BY_CUT).map(Of::Cut));
        if let (Some(pr), Some(of)) = (pr, of) {
            joins.push(Joined { of, pr, commit: commit.sha.to_string(), head: head.to_string() });
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

    #[spec(fn(self: &mut Self[@o]) ensures self: Self[{ pushed: true, ..o }])]
    fn push_branch(&mut self) {
        self.steps.push(JoinStep::PushBranch);
        self.pushed = true;
    }

    /// What joins the train is the head GitHub shows for the PR: a head only
    /// this worktree has would land work the PR never showed.
    #[spec(fn(self: &mut Self[@o]) requires o.pushed ensures self: Self[{ merged: true, ..o }])]
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
/// machine, kept across runs with what its gate runs built until it has
/// `outgrown` its build — and so, with a
/// `-<k>` when the name was taken, the name of its directory.
pub const TRAIN_WORKTREE: &str = "convoy-train";

/// The train's worktree has outgrown its build when what it built is more
/// than this many times its fresh build…
pub const OUTGROWN_TIMES: u64 = 3;

/// …and more than this many bytes, 1 GiB: a small repo's worktree is not cut
/// again and again.
pub const OUTGROWN_FLOOR: u64 = 1_073_741_824;

/// What the last landing in the train's worktree measured of it, in bytes:
/// everything git ignores there, and its fresh build when one is on record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Built {
    pub built: u64,
    pub fresh: Option<u64>,
}

impl Built {
    /// Has the worktree outgrown its build (`outgrown`)?
    pub fn outgrown(self) -> bool {
        outgrown(self.built, self.fresh)
    }
}

/// Is `built` more than `OUTGROWN_TIMES` times `fresh`? Asked of `built - 1`
/// divided, for the product may not fit.
#[spec(fn(built: u64{built > 0}, fresh: u64) -> bool[built > fresh * OUTGROWN_TIMES])]
fn past_fresh(built: u64, fresh: u64) -> bool {
    fresh <= (built - 1) / OUTGROWN_TIMES
}

/// Has the train's worktree outgrown its build — `built` more than
/// `OUTGROWN_TIMES` its `fresh` build and more than `OUTGROWN_FLOOR`? With no
/// fresh build on record (a worktree built in before measuring began), more
/// than the floor is enough: the worktree cut again records one.
#[spec(fn(built: u64, fresh: Option<u64>) -> bool{v: v => built > OUTGROWN_FLOOR})]
pub fn outgrown(built: u64, fresh: Option<u64>) -> bool {
    if built <= OUTGROWN_FLOOR {
        return false;
    }
    match fresh {
        Some(fresh) => past_fresh(built, fresh),
        None => true,
    }
}

/// Is a worktree directory of this name the train's? The train's worktree is
/// linked to no issue, so it is known by the name it was created under.
pub fn is_train_worktree(directory: &str) -> bool {
    is_worktree_named(directory, TRAIN_WORKTREE)
}

/// What the tick does about the train worker while a PR of its fleet is on
/// the train.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tend {
    /// The worker is at work.
    Leave,
    /// The worker stopped on the very tip it was last told about, with the
    /// train not landed: its silence decides (`abandon::train_route`).
    Stopped,
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
                Tend::Stopped
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

    /// A tip pushed with nothing merged onto it.
    #[should_fail]
    fn a_line_pushed_without_a_join_commit() {
        let mut order = JoinOrder::new(true);
        order.push_line();
    }
}

/// What callers may rely on: each is checked by Flux, and stops passing when
/// the spec it reads is weakened.
#[cfg(flux)]
#[allow(dead_code)]
mod relied_on {
    use super::*;

    /// A worktree's commits made on another train are not kept.
    #[spec(fn(ahead: bool, own: bool) -> bool[false])]
    fn commits_of_another_train_are_not_kept(ahead: bool, own: bool) -> bool {
        keeps_its_commits(ahead, own, false)
    }

    /// A worktree within the floor is never cut again, whatever its fresh build.
    #[spec(fn(built: u64{built <= OUTGROWN_FLOOR}, fresh: Option<u64>) -> bool[false])]
    fn a_worktree_within_the_floor_has_not_outgrown(built: u64, fresh: Option<u64>) -> bool {
        outgrown(built, fresh)
    }

    /// A worktree past the floor, within three times its fresh build, has not.
    #[spec(fn(built: u64{built > OUTGROWN_FLOOR}, fresh: u64{built <= fresh * OUTGROWN_TIMES})
        -> bool[false])]
    fn a_worktree_within_three_times_its_fresh_build_has_not_outgrown(
        built: u64,
        fresh: u64,
    ) -> bool {
        past_fresh(built, fresh)
    }

    /// A train that holds nothing is not under repair.
    #[spec(fn(red: Option<(bool, bool)>) -> bool[false])]
    fn a_train_that_holds_nothing_is_not_under_repair(red: Option<(bool, bool)>) -> bool {
        under_repair(false, red)
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
        let message = join_message("  Add plan (really) ", 31, &Of::Issue(7));
        assert_eq!(message, "Add plan (really) (#31)\n\nCloses #7\n");
        let commits = [
            Logged { sha: "m1", parents: "t0 h31", message: &message },
            Logged { sha: "f1", parents: "m1", message: "fix the train (#31)\n\nCloses #7\n" },
            Logged { sha: "m2", parents: "f1 h40", message: "Merge branch 'main'" },
            Logged { sha: "m3", parents: "m2 h32", message: &join_message("", 32, &Of::Issue(8)) },
        ];
        let joins = read_joins(&commits);
        assert_eq!(joins.len(), 2, "a one-parent commit and an ordinary merge are no joining");
        let of = Of::Issue(7);
        assert_eq!(joins[0], Joined { of, pr: 31, commit: "m1".into(), head: "h31".into() });
        assert_eq!((joins[1].pr, joins[1].issue(), joins[1].head.as_str()), (32, Some(8), "h32"));
    }

    #[test]
    fn an_inspection_s_pr_joins_on_no_issue_and_reads_back_by_its_host() {
        let host = Of::Inspection("felixmbp14".into());
        let message = join_message("Queue the gates", 45, &host);
        assert_eq!(message, "Queue the gates (#45)\n\nBy an inspection on felixmbp14\n");
        let commits = [
            Logged { sha: "m1", parents: "t0 h45", message: &message },
            Logged { sha: "m2", parents: "m1 h46", message: "Odd (#46)\n\nBy an inspection on \n" },
        ];
        let joins = read_joins(&commits);
        assert_eq!(joins.len(), 1, "a line that names no host names no inspection");
        assert_eq!(joins[0], Joined { of: host, pr: 45, commit: "m1".into(), head: "h45".into() });
        assert_eq!(joins[0].issue(), None);
    }

    #[test]
    fn a_cut_s_pr_joins_on_no_issue_and_reads_back_by_its_host() {
        let cut = Of::Cut("felixmbp14".into());
        let message = join_message("afkconvoy 0.5.17", 470, &cut);
        assert_eq!(message, "afkconvoy 0.5.17 (#470)\n\nBy a cut on felixmbp14\n");
        let commits = [Logged { sha: "m1", parents: "t0 h470", message: &message }];
        let joins = read_joins(&commits);
        assert_eq!(joins, [Joined { of: cut, pr: 470, commit: "m1".into(), head: "h470".into() }]);
        assert_eq!(joins[0].issue(), None);
    }

    #[test]
    fn a_pr_is_on_the_train_only_at_its_current_head() {
        let join = |pr: u64, commit: &str, head: &str| Joined {
            of: Of::Issue(pr - 20),
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
            of: Of::Issue(pr - 20),
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
        assert_eq!(tend(idle, Some("t1"), "t1"), Tend::Stopped);
    }

    #[test]
    fn a_worktree_has_outgrown_its_build_past_three_times_it_and_the_floor() {
        const GIB: u64 = OUTGROWN_FLOOR;
        assert!(outgrown(4 * GIB, Some(GIB)));
        assert!(!outgrown(3 * GIB, Some(GIB)), "three times is within");
        assert!(outgrown(3 * GIB + 1, Some(GIB)));
        assert!(!outgrown(GIB, Some(1)), "the floor keeps a small repo's");
        assert!(outgrown(GIB + 1, Some(1)));
        assert!(!outgrown(GIB, None));
        assert!(outgrown(GIB + 1, None), "with no fresh build, the floor alone");
        assert!(!outgrown(u64::MAX, Some(u64::MAX)));
        assert!(outgrown(u64::MAX, Some(u64::MAX / 3 - 1)));
        assert!(Built { built: 2 * GIB, fresh: None }.outgrown());
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
        let refs = train_refs(Layout::Hidden, "release/1.x");
        assert_eq!(refs.line, "refs/convoy/train/line/release/1.x");
        assert_eq!(refs.red, "refs/convoy/train/red/release/1.x");
        let refs = train_refs(Layout::Branches, "main");
        assert_eq!(refs.line, "refs/heads/convoy-train/line/main");
    }
}

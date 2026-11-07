//! Workers: the order a worker is started in, and what the fleet records of
//! the branch it cut for one.

use crate::lease::stamp_age;
use flux_rs::attrs::*;

use crate::harness::Harness;
use crate::record;
use crate::turn::Turn;

/// A task refusal belongs to the issue at every stage, never to the account.
/// Its comment is fleet state, so another host reads the same exclusion.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Refusal {
    pub harness: Harness,
    pub evidence: String,
}

impl Refusal {
    pub fn comment(&self) -> String {
        let marker = record::marker(
            "convoy:refusal",
            &[("harness", self.harness.key()), ("evidence", &self.evidence)],
        );
        format!(
            "{marker}\n**afkconvoy** — `{}` refused this task. Later starts of this issue skip this harness at every stage; other issues may still use it.\n\n> {}",
            self.harness.key(),
            self.evidence
        )
    }
}

/// The issue's refused harnesses, once each, with the first refusal's words.
pub fn refusals<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Vec<Refusal> {
    let mut found: Vec<Refusal> = vec![];
    for body in bodies {
        let Some(fields) = record::read_marker("convoy:refusal", body) else { continue };
        let Some(harness) = fields.get("harness").and_then(Harness::read) else { continue };
        let Some(evidence) = fields.get("evidence").filter(|e| !e.is_empty()) else { continue };
        if !found.iter().any(|r| r.harness == harness) {
            found.push(Refusal { harness, evidence: evidence.into() });
        }
    }
    found
}

/// The kind of record that says a branch is the fleet's own for an issue.
pub const BRANCH: &str = "convoy:branch";

/// The comment that records on an issue a branch the fleet cut for it.
pub fn branch_comment(branch: &str) -> String {
    format!(
        "{}\nafkconvoy works this issue on the branch `{branch}`.",
        record::marker(BRANCH, &[("name", branch)])
    )
}

/// The branches the fleet recorded on an issue, from its comments' bodies,
/// oldest first.
pub fn recorded_branches<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut branches: Vec<String> = vec![];
    for body in bodies {
        let name =
            record::read_marker(BRANCH, body).and_then(|f| f.get("name").map(str::to_string));
        if let Some(name) = name
            && !branches.contains(&name)
        {
            branches.push(name);
        }
    }
    branches
}

/// `refs/heads/x/y` → `x/y`; orca reports a worktree's branch either way.
pub fn short_branch(name: &str) -> &str {
    name.strip_prefix("refs/heads/").unwrap_or(name)
}

/// The name hint handed to orca for an issue's worktree — orca derives the real
/// branch from it. The slug is the title lowercased to `[a-z0-9-]`, at most 40
/// characters; a title with nothing usable slugs to `work`.
pub fn worktree_name(number: u64, title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if slug.len() >= 40 {
            break;
        }
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    format!("issue-{number}-{}", if slug.is_empty() { "work" } else { slug })
}

/// One step of starting a worker on an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStep {
    /// Take the claim. Losing it ends the dispatch: nothing after it runs.
    Claim,
    /// Read what survived an earlier worker, which picks the tier the worker
    /// is started from (`recovery::select_recovery`).
    Survey,
    /// Throw the previous attempt away: close its PRs, delete the issue's own
    /// branches on the remote, remove its worktree.
    Discard,
    /// Put the worker's worktree in place: the one still on this host, or one
    /// orca cuts — at the tip of the branch a continuation recreates, else at
    /// the remote base branch's tip.
    Place,
    /// Record on the issue the branch the worker is on.
    RecordBranch,
    /// Write the worker prompt where the worker will read it.
    Brief,
    /// Start the worker with the worker launch command.
    Start,
    /// Wait until the agent is ready for a prompt.
    AwaitReady,
    /// Hand the worker its brief.
    Deliver,
}

/// The steps of one dispatch as they are decided, and how far they have got.
/// `by_hand`: a fresh start somebody asked for — the one dispatch that may
/// discard. `chosen`: where the worker starts from is settled.
#[refined_by(by_hand: bool, claimed: bool, chosen: bool, placed: bool, briefed: bool,
    started: bool, ready: bool)]
struct DispatchOrder {
    // What `survey` and `discard` are stated on: nothing reads it back.
    #[allow(dead_code)]
    #[field(bool[by_hand])]
    by_hand: bool,
    #[field(bool[claimed])]
    claimed: bool,
    #[field(bool[chosen])]
    chosen: bool,
    #[field(bool[placed])]
    placed: bool,
    #[field(bool[briefed])]
    briefed: bool,
    #[field(bool[started])]
    started: bool,
    #[field(bool[ready])]
    ready: bool,
    steps: Vec<DispatchStep>,
}

impl DispatchOrder {
    #[spec(fn(held: bool, by_hand: bool)
        -> DispatchOrder[by_hand, held, false, false, false, false, false])]
    fn new(held: bool, by_hand: bool) -> DispatchOrder {
        DispatchOrder {
            by_hand,
            claimed: held,
            chosen: false,
            placed: false,
            briefed: false,
            started: false,
            ready: false,
            steps: Vec::new(),
        }
    }

    #[spec(fn(self: &mut Self[@o]) ensures self: Self[{ claimed: true, ..o }])]
    fn claim(&mut self) {
        self.steps.push(DispatchStep::Claim);
        self.claimed = true;
    }

    /// What survived is read under the claim: a claim lost costs no read, and
    /// nobody else is changing what is found.
    #[spec(fn(self: &mut Self[@o]) requires o.claimed && !o.by_hand
        ensures self: Self[{ chosen: true, ..o }])]
    fn survey(&mut self) {
        self.steps.push(DispatchStep::Survey);
        self.chosen = true;
    }

    /// An attempt is discarded only by hand, only under the claim, and before
    /// anything is put in place for the new one: a continuation tears nothing
    /// down.
    #[spec(fn(self: &mut Self[@o]) requires o.by_hand && o.claimed && !o.placed
        ensures self: Self[{ chosen: true, ..o }])]
    fn discard(&mut self) {
        self.steps.push(DispatchStep::Discard);
        self.chosen = true;
    }

    /// No worktree for an issue the fleet does not hold, or before it is
    /// known what the worker starts from.
    #[spec(fn(self: &mut Self[@o]) requires o.claimed && o.chosen
        ensures self: Self[{ placed: true, ..o }])]
    fn place(&mut self) {
        self.steps.push(DispatchStep::Place);
        self.steps.push(DispatchStep::RecordBranch);
        self.placed = true;
    }

    /// The brief names the worktree: it is written once there is one.
    #[spec(fn(self: &mut Self[@o]) requires o.placed ensures self: Self[{ briefed: true, ..o }])]
    fn brief(&mut self) {
        self.steps.push(DispatchStep::Brief);
        self.briefed = true;
    }

    /// A worker is started only on a claim held, in a worktree, with its brief waiting.
    #[spec(fn(self: &mut Self[@o]) requires o.claimed && o.placed && o.briefed
        ensures self: Self[{ started: true, ..o }])]
    fn start(&mut self) {
        self.steps.push(DispatchStep::Start);
        self.started = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.started ensures self: Self[{ ready: true, ..o }])]
    fn await_ready(&mut self) {
        self.steps.push(DispatchStep::AwaitReady);
        self.ready = true;
    }

    /// A prompt typed at an agent that is not ready is lost.
    #[spec(fn(self: &mut Self[@o]) requires o.ready)]
    fn deliver(&mut self) {
        self.steps.push(DispatchStep::Deliver);
    }
}

/// The steps that start a worker on an issue, in order. `held`: the claim is
/// already this instance's, so it is not pushed again. `by_hand`: a fresh
/// start somebody asked for, which discards the previous attempt instead of
/// continuing from what survived.
pub fn dispatch_steps(held: bool, by_hand: bool) -> Vec<DispatchStep> {
    let mut order = DispatchOrder::new(held, by_hand);
    if !order.claimed {
        order.claim();
    }
    if by_hand {
        order.discard();
    } else {
        order.survey();
    }
    order.place();
    order.brief();
    order.start();
    order.await_ready();
    order.deliver();
    order.steps
}

/// Is a worktree directory of this name the one created under `name`? orca
/// adds a `-<k>` when the name was taken. A worktree linked to no issue is
/// known by the name it was created under.
pub fn is_worktree_named(directory: &str, name: &str) -> bool {
    match directory.strip_prefix(name) {
        Some("") => true,
        Some(rest) => match rest.strip_prefix('-') {
            Some(k) => !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()),
            None => false,
        },
        None => false,
    }
}

/// How long a worker may be quiet before its silence means anything.
pub const GRACE: u64 = 300;

/// What the worker in a worktree is doing, from its harness's session and
/// whether a terminal is still live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// No live terminal in the worktree: the worker is gone. An open session
    /// left behind by a kill is not a worker still at work.
    Gone,
    /// Its harness has a turn open.
    Busy,
    /// Its turn has ended, or no turn is recorded, with how long it has been
    /// quiet — unknown when no turn was recorded.
    Idle { quiet: Option<u64> },
}

/// The worker's state from how many terminals are live and the turn its
/// session records. A quiet screen does not end an open turn, and a turn
/// the session says ended is idle from that end however recently the
/// terminal printed.
pub fn worker_state(terminals: u64, turn: Option<Turn>, now: u64) -> WorkerState {
    if terminals == 0 {
        return WorkerState::Gone;
    }
    match turn {
        Some(Turn::Open { .. }) => WorkerState::Busy,
        Some(Turn::Closed { since, .. }) => WorkerState::Idle { quiet: stamp_age(since, now) },
        None => WorkerState::Idle { quiet: None },
    }
}

/// Whether what began at `since` has lasted the grace by `now`: from then
/// on, a worker's quiet, or a wait on it, means something.
pub fn past_grace(since: u64, now: u64) -> bool {
    now.saturating_sub(since) >= GRACE
}

/// Whether the worker's session says it is at work, `state`, while orca has
/// said its agent is done, since `done`: they disagree, and neither is the
/// worker's state alone.
pub fn disputed(state: Option<WorkerState>, done: Option<u64>) -> bool {
    state == Some(WorkerState::Busy) && done.is_some()
}

/// Whether the worker is `disputed`, and orca has said its agent is done for
/// the grace period by `now`: long enough to be acted on.
pub fn disputed_past_grace(state: Option<WorkerState>, done: Option<u64>, now: u64) -> bool {
    disputed(state, done) && done.is_some_and(|since| past_grace(since, now))
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A worker started on an issue nobody claimed.
    #[should_fail]
    fn what_survived_read_without_a_claim() {
        let mut order = DispatchOrder::new(false, false);
        order.survey();
    }

    /// A worktree put in place before it is known what the worker starts from.
    #[should_fail]
    fn a_worktree_placed_before_the_start_is_chosen() {
        let mut order = DispatchOrder::new(true, false);
        order.place();
    }

    /// A prompt handed to an agent that was never waited for.
    #[should_fail]
    fn a_prompt_delivered_before_the_agent_is_ready() {
        let mut order = DispatchOrder::new(true, false);
        order.survey();
        order.place();
        order.brief();
        order.start();
        order.deliver();
    }

    /// A continuation that tears the attempt down.
    #[should_fail]
    fn an_attempt_discarded_by_a_continuation() {
        let mut order = DispatchOrder::new(true, false);
        order.discard();
    }

    /// An attempt discarded for an issue the fleet does not hold.
    #[should_fail]
    fn an_attempt_discarded_without_the_claim() {
        let mut order = DispatchOrder::new(false, true);
        order.discard();
    }

    /// The worktree just placed for the new attempt, discarded with the old one.
    #[should_fail]
    fn a_discard_after_the_new_worktree_is_placed() {
        let mut order = DispatchOrder::new(true, true);
        order.discard();
        order.place();
        order.discard();
    }

    /// A start by hand that goes on to continue from what it was told to discard.
    #[should_fail]
    fn a_start_by_hand_that_continues() {
        let mut order = DispatchOrder::new(true, true);
        order.survey();
    }

    /// A worker started with no brief to read.
    #[should_fail]
    fn a_worker_started_before_its_brief() {
        let mut order = DispatchOrder::new(true, false);
        order.survey();
        order.place();
        order.start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_comments_keep_evidence_and_ignore_unknown_or_duplicate_harnesses() {
        let first = Refusal {
            harness: Harness::Codex,
            evidence: "a wrapped\nrefusal with <markup> & punctuation".into(),
        };
        let comments = [
            first.comment(),
            first.comment(),
            "<!--convoy:refusal harness=unknown evidence=no-->".into(),
            "<!--convoy:refusal harness=pi-->".into(),
        ];
        let read = refusals(comments.iter().map(String::as_str));
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].harness, Harness::Codex);
        assert_eq!(read[0].evidence, first.evidence);
    }

    #[test]
    fn a_dispatch_runs_in_one_order() {
        use DispatchStep::*;
        let rest = [Place, RecordBranch, Brief, Start, AwaitReady, Deliver];
        assert_eq!(dispatch_steps(true, false)[0], Survey);
        assert_eq!(dispatch_steps(true, false)[1..], rest);
        assert_eq!(dispatch_steps(false, false)[..2], [Claim, Survey]);
        // continuation tears nothing down
        assert!(!dispatch_steps(true, false).contains(&Discard));
        assert!(!dispatch_steps(false, false).contains(&Discard));
        // by hand: the attempt is discarded, and nothing of it is read to continue
        assert_eq!(dispatch_steps(true, true)[0], Discard);
        assert_eq!(dispatch_steps(true, true)[1..], rest);
        assert_eq!(dispatch_steps(false, true)[..2], [Claim, Discard]);
    }

    #[test]
    fn a_worktree_name_is_slugged_from_the_title() {
        assert_eq!(worktree_name(7, "Add `plan`: Rebuild!"), "issue-7-add-plan-rebuild");
        assert_eq!(worktree_name(7, "修复"), "issue-7-work");
        let long = worktree_name(7, &"word ".repeat(20));
        assert!(long.len() <= "issue-7-".len() + 40 && !long.ends_with('-'), "{long}");
    }

    #[test]
    fn a_recorded_branch_reads_back_once() {
        let bodies = [
            branch_comment("u/issue-7-a"),
            "chat".into(),
            branch_comment("u/issue-7-a"),
            branch_comment("u/b"),
        ];
        assert_eq!(recorded_branches(bodies.iter().map(String::as_str)), ["u/issue-7-a", "u/b"]);
        assert_eq!(short_branch("refs/heads/u/b"), "u/b");
        assert_eq!(short_branch("u/b"), "u/b");
    }

    #[test]
    fn a_session_at_work_that_orca_says_is_done_is_disputed_and_acted_on_past_the_grace() {
        assert!(!past_grace(1000, 1000 + GRACE - 1));
        assert!(past_grace(1000, 1000 + GRACE));
        assert!(!past_grace(1000 + GRACE, 1000), "a stamp ahead has lasted nothing");
        let busy = Some(WorkerState::Busy);
        assert!(disputed(busy, Some(1000)), "said at once, however recently");
        assert!(!disputed(busy, None), "orca says nothing of it done");
        let idle = Some(WorkerState::Idle { quiet: None });
        for state in [None, Some(WorkerState::Gone), idle] {
            assert!(!disputed(state, Some(1000)), "{state:?} is not its session at work");
            assert!(!disputed_past_grace(state, Some(1000), 1000 + GRACE));
        }
        assert!(!disputed_past_grace(busy, Some(1000), 1000 + GRACE - 1));
        assert!(disputed_past_grace(busy, Some(1000), 1000 + GRACE));
        assert!(!disputed_past_grace(busy, None, 1000 + GRACE));
    }

    #[test]
    fn a_workers_state_is_its_session_turn() {
        let open = Turn::Open { since: 900 };
        let closed = Turn::Closed { since: 940, started: Some(900) };
        assert_eq!(
            worker_state(0, Some(open), 1000),
            WorkerState::Gone,
            "no live terminal means the worker is gone, even when a killed session never wrote an end"
        );
        assert_eq!(worker_state(1, Some(open), 1000), WorkerState::Busy);
        assert_eq!(
            worker_state(1, Some(open), 1000 + GRACE),
            WorkerState::Busy,
            "an open turn stays busy however quiet the screen is"
        );
        assert_eq!(
            worker_state(1, Some(closed), 1000),
            WorkerState::Idle { quiet: Some(60) },
            "quiet is measured from the end"
        );
        assert_eq!(worker_state(1, None, 1000), WorkerState::Idle { quiet: None });
        assert_eq!(
            worker_state(1, Some(Turn::Closed { since: 1000 + 10_000, started: Some(1000) }), 1000),
            WorkerState::Idle { quiet: None },
            "a stamp far ahead is not a quiet time"
        );
    }
}

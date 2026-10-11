//! Workers: the order a worker is started in, and what the fleet records of
//! the branch it cut for one.

use crate::lease::stamp_age;
use flux_rs::attrs::*;

use crate::record;

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
    /// Have orca cut the worktree and branch at the remote base branch's tip.
    Cut,
    /// Record on the issue the branch orca cut.
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
#[refined_by(claimed: bool, cut: bool, briefed: bool, started: bool, ready: bool)]
struct DispatchOrder {
    #[field(bool[claimed])]
    claimed: bool,
    #[field(bool[cut])]
    cut: bool,
    #[field(bool[briefed])]
    briefed: bool,
    #[field(bool[started])]
    started: bool,
    #[field(bool[ready])]
    ready: bool,
    steps: Vec<DispatchStep>,
}

impl DispatchOrder {
    #[spec(fn(held: bool) -> DispatchOrder[held, false, false, false, false])]
    fn new(held: bool) -> DispatchOrder {
        DispatchOrder {
            claimed: held,
            cut: false,
            briefed: false,
            started: false,
            ready: false,
            steps: Vec::new(),
        }
    }

    #[spec(fn(self: &mut Self[@o]) ensures self: Self[true, o.cut, o.briefed, o.started, o.ready])]
    fn claim(&mut self) {
        self.steps.push(DispatchStep::Claim);
        self.claimed = true;
    }

    /// Nothing is cut for an issue the fleet does not hold.
    #[spec(fn(self: &mut Self[@o]) requires o.claimed
        ensures self: Self[o.claimed, true, o.briefed, o.started, o.ready])]
    fn cut(&mut self) {
        self.steps.push(DispatchStep::Cut);
        self.steps.push(DispatchStep::RecordBranch);
        self.cut = true;
    }

    /// The brief names the worktree: it is written once there is one.
    #[spec(fn(self: &mut Self[@o]) requires o.cut
        ensures self: Self[o.claimed, o.cut, true, o.started, o.ready])]
    fn brief(&mut self) {
        self.steps.push(DispatchStep::Brief);
        self.briefed = true;
    }

    /// A worker is started only on a claim held, in a worktree, with its brief waiting.
    #[spec(fn(self: &mut Self[@o]) requires o.claimed && o.cut && o.briefed
        ensures self: Self[o.claimed, o.cut, o.briefed, true, o.ready])]
    fn start(&mut self) {
        self.steps.push(DispatchStep::Start);
        self.started = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.started
        ensures self: Self[o.claimed, o.cut, o.briefed, o.started, true])]
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

/// The steps that start a fresh worker on an issue, in order. `held`: the
/// claim is already this instance's, so it is not pushed again.
pub fn dispatch_steps(held: bool) -> Vec<DispatchStep> {
    let mut order = DispatchOrder::new(held);
    if !order.claimed {
        order.claim();
    }
    order.cut();
    order.brief();
    order.start();
    order.await_ready();
    order.deliver();
    order.steps
}

/// How long a worker may be quiet before its silence means anything.
pub const GRACE: u64 = 300;

/// What a worker's runtime last reported to orca: `working`, or that it
/// stopped (waiting, blocked, done), and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentReport {
    pub working: bool,
    /// When it entered that state, in epoch seconds.
    pub since: Option<u64>,
}

/// What the worker in a worktree is doing, as orca has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// No live terminal in the worktree: the worker is gone.
    Gone,
    /// It reports `working`, and its terminal produced output within grace.
    Busy,
    /// Anything else, with how long it has been quiet — unknown for a runtime
    /// that reports no state: its idle screen redraws on a timer, so its
    /// output says nothing.
    Idle { quiet: Option<u64> },
}

/// The report that speaks for a worktree: of its top-level agents, the one
/// that changed state last. An older pane's report does not.
pub fn lead(agents: &[AgentReport]) -> Option<AgentReport> {
    let mut lead: Option<AgentReport> = None;
    for agent in agents {
        let newer = match lead {
            Some(found) => agent.since.unwrap_or(0) >= found.since.unwrap_or(0),
            None => true,
        };
        if newer {
            lead = Some(*agent);
        }
    }
    lead
}

/// Reads a worker's state off its worktree's row of `orca worktree ps`:
/// `terminals` live there, the terminal's `last_output` (epoch seconds), and
/// the `lead` report.
///
/// Busy takes both the report and recent output: a stop report the runtime
/// lost would otherwise read `working` forever.
pub fn worker_state(
    terminals: u64,
    last_output: Option<u64>,
    lead: Option<AgentReport>,
    now: u64,
) -> WorkerState {
    if terminals == 0 {
        return WorkerState::Gone;
    }
    let output_quiet = match last_output {
        Some(at) => stamp_age(at, now),
        None => None,
    };
    match lead {
        Some(agent) if agent.working => match output_quiet {
            Some(quiet) if quiet < GRACE => WorkerState::Busy,
            _ => WorkerState::Idle { quiet: output_quiet },
        },
        Some(agent) => {
            let stopped = match agent.since {
                Some(at) => stamp_age(at, now),
                None => None,
            };
            WorkerState::Idle { quiet: if stopped.is_some() { stopped } else { output_quiet } }
        }
        None => WorkerState::Idle { quiet: None },
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A worker started on an issue nobody claimed.
    #[should_fail]
    fn a_worker_started_without_a_claim() {
        let mut order = DispatchOrder::new(false);
        order.cut();
    }

    /// A prompt handed to an agent that was never waited for.
    #[should_fail]
    fn a_prompt_delivered_before_the_agent_is_ready() {
        let mut order = DispatchOrder::new(true);
        order.cut();
        order.brief();
        order.start();
        order.deliver();
    }

    /// A worker started with no brief to read.
    #[should_fail]
    fn a_worker_started_before_its_brief() {
        let mut order = DispatchOrder::new(true);
        order.cut();
        order.start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dispatch_runs_in_one_order() {
        use DispatchStep::*;
        let rest = [Cut, RecordBranch, Brief, Start, AwaitReady, Deliver];
        assert_eq!(dispatch_steps(true), rest);
        assert_eq!(dispatch_steps(false)[0], Claim);
        assert_eq!(dispatch_steps(false)[1..], rest);
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
    fn a_workers_state_is_read_off_orcas_row() {
        let working = AgentReport { working: true, since: Some(900) };
        let stopped = AgentReport { working: false, since: Some(940) };
        assert_eq!(worker_state(0, Some(990), Some(working), 1000), WorkerState::Gone);
        assert_eq!(worker_state(1, Some(990), Some(working), 1000), WorkerState::Busy);
        // a stop report the runtime lost: `working`, but no output within grace
        assert_eq!(
            worker_state(1, Some(1000 - GRACE), Some(working), 1000),
            WorkerState::Idle { quiet: Some(GRACE) }
        );
        assert_eq!(worker_state(1, None, Some(working), 1000), WorkerState::Idle { quiet: None });
        assert_eq!(
            worker_state(1, Some(990), Some(stopped), 1000),
            WorkerState::Idle { quiet: Some(60) },
            "timed from when it said it stopped"
        );
        assert_eq!(worker_state(1, Some(990), None, 1000), WorkerState::Idle { quiet: None });
        assert_eq!(lead(&[stopped, working]), Some(stopped), "the newest report speaks");
        assert_eq!(lead(&[]), None);
    }
}

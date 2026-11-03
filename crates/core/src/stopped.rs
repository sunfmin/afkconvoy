//! A worker that stopped without a PR: what its worktree holds, and what the
//! tick does about what it declared.

use flux_rs::attrs::*;

use crate::lease::stamp_age;
use crate::verdict::{Phase, Verdict};
use crate::worker::{GRACE, WorkerState};

/// One worktree's git progress since its merge base with the remote base branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Commits on the branch past the merge base; `None` when the count could
    /// not be read at all — unreadable is not zero.
    pub commits_ahead: Option<u64>,
    /// Whether the committed tree equals the merge base's; `None` when unreadable.
    pub trees_equal: Option<bool>,
    /// Anything uncommitted or untracked.
    pub dirty: bool,
    /// When the last commit was made, and when a file was last touched.
    pub last_commit: Option<u64>,
    pub touched: Option<u64>,
}

/// The proof that a worktree's branch holds no work: no commits past the merge
/// base, a tree equal to it and nothing uncommitted. `Workless::of` is the only
/// way to one, so what asks for one — closing an issue as needing no change —
/// cannot be done over work, or over a branch nobody could read.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(ahead: int, equal: bool, dirty: bool)]
#[invariant(ahead == 0 && equal && !dirty)]
// The fields are what the invariant is stated on: nothing reads them back.
#[allow(dead_code)]
pub struct Workless {
    #[field(u64[ahead])]
    ahead: u64,
    #[field(bool[equal])]
    equal: bool,
    #[field(bool[dirty])]
    dirty: bool,
}

impl Workless {
    pub fn of(progress: &Progress) -> Option<Workless> {
        match (progress.commits_ahead, progress.trees_equal) {
            (Some(ahead), Some(equal)) => {
                if ahead == 0 && equal && !progress.dirty {
                    Some(Workless { ahead, equal, dirty: progress.dirty })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Why a stopped worker's claim is handed to a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// It declared `escalate`.
    Escalate,
    /// It declared `already-satisfied` over work sitting on its branch.
    Refuted,
    /// It declared a verdict naming no phase the fleet knows.
    UnknownPhase,
}

impl Why {
    /// The reason in the few words a tick's line has room for.
    pub fn brief(self) -> &'static str {
        match self {
            Why::Escalate => "its worker declared escalate",
            Why::Refuted => "already-satisfied, but its branch holds work",
            Why::UnknownPhase => "a verdict the fleet cannot read",
        }
    }

    /// The reason as the escalation states it; `said` points at what the
    /// worker wrote.
    pub fn reason(self, said: &str) -> String {
        match self {
            Why::Escalate => format!("The worker declared `escalate`: {said}"),
            Why::Refuted => format!(
                "The worker declared `already-satisfied`, but its branch holds work (commits \
                 past the merge base, a tree different from it, or uncommitted files), or \
                 could not be read: {said}"
            ),
            Why::UnknownPhase => {
                format!("The worker declared a verdict the fleet cannot read: {said}")
            }
        }
    }
}

/// What this host holds of nudging the worker in a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nudged {
    /// There is no worktree here: nowhere a nudge could be recorded.
    Nowhere,
    Never,
    /// Nudged at this epoch second.
    At(u64),
}

/// Work a stopped worker started itself — a shell or a monitor it runs in the
/// background — that its harness shows still running: first seen at
/// `since`, and waited on for no longer than `limit` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Background {
    pub since: u64,
    pub limit: u64,
}

/// How many lines of its screen to ask a stopped worker's terminal for, to see
/// whether its harness shows background work of its own running: the footer
/// under its prompt, and the blank lines around it.
pub const FOOTER_LINES: usize = 12;

/// The proof that a worker is silent: it stopped, showed no sign of life
/// within grace — and waits on no background work of its own, or has waited
/// on it past the limit — and declared nothing. Only `route` makes one, so what asks
/// for one — reading the worker's screen — is never done to a worker at work,
/// or to one that said what it stopped on.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(lively: bool, declared: bool)]
#[invariant(!lively && !declared)]
// The fields are what the invariant is stated on: nothing reads them back.
#[allow(dead_code)]
pub struct Silent {
    #[field(bool[lively])]
    lively: bool,
    #[field(bool[declared])]
    declared: bool,
    /// Whether it was nudged, and stayed silent a grace period more.
    pub nudged: bool,
    /// How long it waited on its own background work, when that ran past the
    /// limit — which is its silence, nudged or not.
    pub waited: Option<u64>,
}

/// The proof that the worker here was never nudged, used up by the one nudge
/// it allows.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(sent: int)]
#[invariant(sent == 0)]
// The field is what the invariant is stated on: nothing reads it back.
#[allow(dead_code)]
pub struct Unnudged {
    #[field(u64[sent])]
    sent: u64,
}

/// What the tick does with a PR-less claim.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Nothing: the worker is at work, showed a sign of life within grace, or
    /// stopped in a way this decision does not settle.
    Leave,
    /// Nothing: it stopped, and waits on its own background work, running
    /// since this epoch second.
    Waiting(u64),
    /// The issue needed no change: close it.
    Close(Workless),
    Escalate(Why),
    /// Its worker is gone: the claim is orphaned, and continued from what
    /// survived — never released back to the frontier.
    Continue,
    /// It declared `blocked`: where the issues it named stand decides
    /// (`blocked::blocked_route`).
    Blocked,
    /// It stopped and declared nothing: tell it, once, to carry on.
    Nudge(Unnudged),
    /// It stayed silent past its nudge, or cannot be nudged here: escalate,
    /// saying where it stopped.
    Stalled(Silent),
}

/// How many lines of a silent worker's screen travel in a reason, and how much
/// of each.
pub const SCREEN_LINES: usize = 30;
const SCREEN_LINE_CHARS: usize = 200;

/// The end of a worker's screen, small enough to carry in a comment.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(len: int)]
#[invariant(len <= SCREEN_LINES)]
pub struct Tail {
    /// Last line first.
    #[field(Vec<String>[len])]
    lines: Vec<String>,
}

impl Tail {
    #[spec(fn() -> Tail[0])]
    fn empty() -> Tail {
        Tail { lines: Vec::new() }
    }

    #[spec(fn(&Tail[@t]) -> bool[t.len >= SCREEN_LINES])]
    fn full(&self) -> bool {
        self.lines.len() >= SCREEN_LINES
    }

    #[spec(fn(self: &mut Self[@t], line: String) requires t.len < SCREEN_LINES ensures self: Self[t.len + 1])]
    fn push(&mut self, line: String) {
        self.lines.push(line);
    }

    /// The last `SCREEN_LINES` lines of `screen` with anything on them, each
    /// cut to a bounded width.
    pub fn of(screen: &[String]) -> Tail {
        let mut tail = Tail::empty();
        for line in screen.iter().rev() {
            if tail.full() {
                break;
            }
            let line = line.trim_end();
            if !line.trim().is_empty() {
                tail.push(line.chars().take(SCREEN_LINE_CHARS).collect());
            }
        }
        tail
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The lines, top to bottom.
    pub fn text(&self) -> String {
        let lines: Vec<&str> = self.lines.iter().rev().map(String::as_str).collect();
        lines.join("\n")
    }
}

/// How many lines of its screen to ask a silent worker's terminal for: enough
/// that the blank ones can be dropped. The one reason a terminal is ever read.
pub fn screen_request(_silent: &Silent) -> usize {
    SCREEN_LINES * 2
}

/// Why a silent worker's claim is handed to a human, with where it stopped.
pub fn stalled_reason(silent: &Silent, tail: &Tail) -> String {
    stalled("The worker stopped with no PR and no verdict", silent, tail)
}

/// Why the claim of a silent worker of the stage `stage` — its consult, when
/// `consult` — is handed to a human, with where it stopped.
pub fn stage_stalled_reason(stage: &str, consult: bool, silent: &Silent, tail: &Tail) -> String {
    let stopped = if consult {
        format!("The consult of the stage `{stage}` stopped with no ruling and no `escalate`")
    } else {
        format!("The worker of the stage `{stage}` stopped with no valid verdict")
    };
    stalled(&stopped, silent, tail)
}

/// `stopped` — what the worker stopped without — with how its silence was
/// established, and the end of its screen.
fn stalled(stopped: &str, silent: &Silent, tail: &Tail) -> String {
    let why = if let Some(waited) = silent.waited {
        format!(
            "{stopped}, and waited {waited}s on its own background work, past the limit, \
             without a word."
        )
    } else if silent.nudged {
        format!("{stopped}, and stayed silent after one nudge.")
    } else {
        format!("{stopped}, and this host has no worktree of its to nudge it in.")
    };
    if tail.is_empty() {
        return why;
    }
    format!("{why} Its terminal ended with:\n\n```\n{}\n```", tail.text())
}

/// One step of nudging a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeStep {
    /// Type the nudge line at the worker.
    Tell,
    /// Record the nudge in the worktree.
    Record,
}

/// A nudge under way: whether the worker has taken the line.
#[refined_by(told: bool)]
struct Nudging {
    #[field(bool[told])]
    told: bool,
}

impl Nudging {
    #[spec(fn(_unnudged: Unnudged) -> Nudging[false])]
    fn begin(_unnudged: Unnudged) -> Nudging {
        Nudging { told: false }
    }

    #[spec(fn(self: &mut Self[false]) -> NudgeStep ensures self: Self[true])]
    fn tell(&mut self) -> NudgeStep {
        self.told = true;
        NudgeStep::Tell
    }

    /// Recorded only once the worker took the line: a nudge recorded and never
    /// sent would escalate a worker nobody told.
    #[spec(fn(&Self[true]) -> NudgeStep)]
    fn record(&self) -> NudgeStep {
        NudgeStep::Record
    }
}

/// Nudges a worker: tell it → record that. It uses up the proof that the
/// worker was never nudged, so no worker is nudged twice.
pub fn nudge<E, F>(unnudged: Unnudged, mut carry_out: F) -> Result<(), E>
where
    F: FnMut(NudgeStep) -> Result<(), E>,
{
    let mut nudging = Nudging::begin(unnudged);
    carry_out(nudging.tell())?;
    carry_out(nudging.record())
}

/// How long ago the most recent sign of life was: terminal activity, the last
/// commit, a file touched, a nudge. `None` when none is known — which is never "within
/// grace". Commits ahead and a dirty tree are standing facts, true until the
/// branch lands: never signs of life.
pub fn quiet_for(
    terminal: Option<u64>,
    progress: &Progress,
    nudged: Nudged,
    now: u64,
) -> Option<u64> {
    let committed = match progress.last_commit {
        Some(at) => stamp_age(at, now),
        None => None,
    };
    let touched = match progress.touched {
        Some(at) => stamp_age(at, now),
        None => None,
    };
    let nudged = match nudged {
        Nudged::At(at) => stamp_age(at, now),
        Nudged::Nowhere | Nudged::Never => None,
    };
    least(least(least(terminal, committed), touched), nudged)
}

fn least(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if a <= b { a } else { b }),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// The verdict that still stands: a `blocked` one the fleet answered — every
/// blocker closed, a worker started on the issue again — is not the verdict of
/// the worker now in the worktree. `said` is the comment the verdict is in,
/// `answered` the one last answered here.
pub fn standing_verdict(
    verdict: Option<Verdict>,
    said: Option<&str>,
    answered: Option<&str>,
) -> Option<Verdict> {
    if said.is_some() && said == answered {
        return None;
    }
    verdict
}

/// What a worker's state alone settles for its claim, before anything else is
/// read: `None` for one that stopped, whose reasons must be gathered. `state` is `None` when the claim has no worktree on this host —
/// which is a worker that is gone.
pub fn settled_by_state(state: Option<WorkerState>) -> Option<Route> {
    match state {
        None | Some(WorkerState::Gone) => Some(Route::Continue),
        Some(WorkerState::Busy) => Some(Route::Leave),
        Some(WorkerState::Idle { .. }) => None,
    }
}

/// Where a worker that may have stopped stands, before anything it said is
/// read.
enum Quiet {
    Gone,
    /// At work, or a sign of life within grace.
    Lively,
    /// Stopped, waiting on its own background work since this epoch second.
    Waiting(u64),
    /// Stopped — having waited this long on its own background work, past
    /// the limit, when it did.
    Stopped(Option<u64>),
}

/// `handed`: when the worker here was last handed its work by the fleet —
/// started, told of a train's tip. A sign of life like a nudge: it gets a
/// grace period from then. `background`: its own background work, running —
/// it is waiting, not silent, until that has run past its limit.
fn quiet(
    state: WorkerState,
    progress: &Progress,
    nudged: Nudged,
    handed: Option<u64>,
    background: Option<Background>,
    now: u64,
) -> Quiet {
    let terminal = match state {
        WorkerState::Gone => return Quiet::Gone,
        WorkerState::Busy => return Quiet::Lively,
        WorkerState::Idle { quiet } => quiet,
    };
    let handed = match handed {
        Some(at) => stamp_age(at, now),
        None => None,
    };
    match least(quiet_for(terminal, progress, nudged, now), handed) {
        Some(quiet) if quiet < GRACE => Quiet::Lively,
        _ => match background {
            None => Quiet::Stopped(None),
            // A stamp from the future is work just seen: it is waited on.
            Some(background) => match stamp_age(background.since, now) {
                Some(waited) if waited >= background.limit => Quiet::Stopped(Some(waited)),
                _ => Quiet::Waiting(background.since),
            },
        },
    }
}

/// Whether a worker stopped and showed no sign of life within grace (see
/// `silence`): whether it waits on its own background work is what decides
/// its silence then, and so the one time that is worth reading off its
/// screen.
pub fn past_grace(
    state: WorkerState,
    progress: &Progress,
    nudged: Nudged,
    handed: Option<u64>,
    now: u64,
) -> bool {
    matches!(quiet(state, progress, nudged, handed, None, now), Quiet::Stopped(_))
}

/// The silence of a worker that declared nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Silence {
    /// Its terminal is gone.
    Gone,
    /// It is at work, or showed a sign of life within grace.
    Lively,
    /// It stopped, and waits on its own background work, running since this
    /// epoch second: it is not silent.
    Waiting(u64),
    /// It stopped: tell it, once, to carry on.
    Nudge(Unnudged),
    /// It stayed silent past its nudge, or cannot be nudged here.
    Stalled(Silent),
}

/// How silent a worker is that declared nothing — or whose word is not on its
/// issue to read: one with a PR open. The one place a nudge is allowed, and a
/// silence proven. A worker waiting on its own `background` work is neither
/// nudged nor silent until that work ends — and it stays still past its nudge
/// — or runs past its limit, which is a silence at once.
pub fn silence(
    state: WorkerState,
    progress: &Progress,
    nudged: Nudged,
    handed: Option<u64>,
    background: Option<Background>,
    now: u64,
) -> Silence {
    let silent = |nudged| Silent { lively: false, declared: false, nudged, waited: None };
    match quiet(state, progress, nudged, handed, background, now) {
        Quiet::Gone => Silence::Gone,
        Quiet::Lively => Silence::Lively,
        Quiet::Waiting(since) => Silence::Waiting(since),
        Quiet::Stopped(Some(waited)) => Silence::Stalled(Silent {
            waited: Some(waited),
            ..silent(matches!(nudged, Nudged::At(_)))
        }),
        Quiet::Stopped(None) => match nudged {
            Nudged::Never => Silence::Nudge(Unnudged { sent: 0 }),
            Nudged::At(_) => Silence::Stalled(silent(true)),
            Nudged::Nowhere => Silence::Stalled(silent(false)),
        },
    }
}

/// Routes one PR-less claim from its worker's state, its worktree's progress,
/// the latest verdict on its issue and the nudge this host recorded. Nothing
/// is retried: a failure escalates (docs/adr/0005).
///
/// What the worker declared is routed whether or not it was nudged; a nudge is
/// only ever for a worker that declared nothing. `handed` and `background`
/// (see `silence`) spare only that worker: a verdict is not kept waiting for
/// either.
pub fn route(
    state: WorkerState,
    progress: &Progress,
    verdict: Option<&Verdict>,
    nudged: Nudged,
    (handed, background): (Option<u64>, Option<Background>),
    now: u64,
) -> Route {
    let Some(verdict) = verdict else {
        return match silence(state, progress, nudged, handed, background, now) {
            Silence::Gone => Route::Continue,
            Silence::Lively => Route::Leave,
            Silence::Waiting(since) => Route::Waiting(since),
            Silence::Nudge(unnudged) => Route::Nudge(unnudged),
            Silence::Stalled(silent) => Route::Stalled(silent),
        };
    };
    match quiet(state, progress, nudged, None, None, now) {
        Quiet::Gone => return Route::Continue,
        Quiet::Lively => return Route::Leave,
        Quiet::Waiting(_) | Quiet::Stopped(_) => {}
    }
    match verdict.phase {
        Some(Phase::AlreadySatisfied) => match Workless::of(progress) {
            Some(workless) => Route::Close(workless),
            None => Route::Escalate(Why::Refuted),
        },
        Some(Phase::Escalate) => Route::Escalate(Why::Escalate),
        Some(Phase::Blocked) => Route::Blocked,
        None => Route::Escalate(Why::UnknownPhase),
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// An issue closed as needing no change over a tree different from the merge base.
    #[should_fail]
    fn a_different_tree_called_workless(equal: bool) -> Workless {
        Workless { ahead: 0, equal, dirty: false }
    }

    /// …or over uncommitted files.
    #[should_fail]
    fn a_dirty_tree_called_workless(dirty: bool) -> Workless {
        Workless { ahead: 0, equal: true, dirty }
    }

    /// …or with commits past the merge base, even when the trees are equal.
    #[should_fail]
    fn commits_ahead_called_workless(ahead: u64) -> Workless {
        Workless { ahead, equal: true, dirty: false }
    }

    /// A worker that declared a verdict, taken for silent: its screen read, a
    /// nudge typed over what it said.
    #[should_fail]
    fn a_worker_that_declared_called_silent(declared: bool) -> Silent {
        Silent { lively: false, declared, nudged: false, waited: None }
    }

    /// …or one that showed a sign of life within grace.
    #[should_fail]
    fn a_lively_worker_called_silent(lively: bool) -> Silent {
        Silent { lively, declared: false, nudged: false, waited: None }
    }

    /// A worker already nudged, nudged again.
    #[should_fail]
    fn a_second_nudge(sent: u64) -> Unnudged {
        Unnudged { sent }
    }

    /// A nudge recorded that the worker never took.
    #[should_fail]
    fn a_nudge_recorded_before_it_is_sent(unnudged: Unnudged) -> NudgeStep {
        Nudging::begin(unnudged).record()
    }

    /// One nudge's line typed twice.
    #[should_fail]
    fn a_nudge_typed_twice(unnudged: Unnudged) {
        let mut nudging = Nudging::begin(unnudged);
        nudging.tell();
        nudging.tell();
    }

    /// More of a screen carried than a comment should hold.
    #[should_fail]
    fn a_tail_past_one_screen(tail: &mut Tail, line: String) {
        tail.push(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 100_000;

    fn progress(ahead: u64, dirty: bool) -> Progress {
        Progress {
            commits_ahead: Some(ahead),
            trees_equal: Some(ahead == 0),
            dirty,
            last_commit: Some(1),
            touched: Some(1),
        }
    }

    fn said(phase: Option<Phase>) -> Verdict {
        Verdict { phase, blocked_by: Default::default(), comment: 0 }
    }

    const STOPPED: WorkerState = WorkerState::Idle { quiet: Some(GRACE) };

    #[test]
    fn a_busy_worker_and_a_recent_sign_of_life_are_left() {
        let verdict = said(Some(Phase::Escalate));
        assert_eq!(
            route(
                WorkerState::Busy,
                &progress(0, false),
                Some(&verdict),
                Nudged::Never,
                (None, None),
                NOW
            ),
            Route::Leave
        );
        let just = WorkerState::Idle { quiet: Some(GRACE - 1) };
        assert_eq!(
            route(just, &progress(0, false), Some(&verdict), Nudged::Never, (None, None), NOW),
            Route::Leave
        );
        let committed = Progress { last_commit: Some(NOW - 10), ..progress(0, false) };
        assert_eq!(
            route(STOPPED, &committed, Some(&verdict), Nudged::Never, (None, None), NOW),
            Route::Leave
        );
        let touched = Progress { touched: Some(NOW - 10), ..progress(0, false) };
        assert_eq!(
            route(STOPPED, &touched, Some(&verdict), Nudged::Never, (None, None), NOW),
            Route::Leave
        );
    }

    #[test]
    fn commits_ahead_and_a_dirty_tree_are_no_signs_of_life() {
        let verdict = said(Some(Phase::Escalate));
        let route =
            route(STOPPED, &progress(9, true), Some(&verdict), Nudged::Never, (None, None), NOW);
        assert_eq!(route, Route::Escalate(Why::Escalate));
        // nor is a stamp from the future, or no stamp at all
        let unknown = Progress { last_commit: Some(NOW * 2), touched: None, ..progress(0, false) };
        assert_eq!(quiet_for(None, &unknown, Nudged::Never, NOW), None);
        assert_eq!(quiet_for(Some(400), &progress(0, false), Nudged::Never, NOW), Some(400));
    }

    #[test]
    fn a_verdict_is_routed_by_who_can_supply_what_it_lacks() {
        let idle = WorkerState::Idle { quiet: None };
        let satisfied = said(Some(Phase::AlreadySatisfied));
        assert!(matches!(
            route(idle, &progress(0, false), Some(&satisfied), Nudged::Never, (None, None), NOW),
            Route::Close(_)
        ));
        for refuted in [
            progress(1, false),
            progress(0, true),
            Progress { trees_equal: Some(true), ..progress(9, false) },
            Progress { trees_equal: Some(false), ..progress(0, false) },
            Progress { trees_equal: None, ..progress(0, false) },
            Progress { commits_ahead: None, ..progress(0, false) },
        ] {
            assert_eq!(
                route(idle, &refuted, Some(&satisfied), Nudged::Never, (None, None), NOW),
                Route::Escalate(Why::Refuted)
            );
        }
        assert_eq!(
            route(idle, &progress(0, false), Some(&said(None)), Nudged::Never, (None, None), NOW),
            Route::Escalate(Why::UnknownPhase)
        );
        assert!(Why::Escalate.reason("https://x/1").contains("https://x/1"));
    }

    #[test]
    fn a_blocked_verdict_is_routed_by_its_blockers_unless_already_answered() {
        let blocked = said(Some(Phase::Blocked));
        assert_eq!(
            route(STOPPED, &progress(1, false), Some(&blocked), Nudged::Never, (None, None), NOW),
            Route::Blocked
        );
        let at = Some("https://x/1");
        assert_eq!(standing_verdict(Some(blocked.clone()), at, None), Some(blocked.clone()));
        assert_eq!(
            standing_verdict(Some(blocked.clone()), at, Some("https://x/0")),
            Some(blocked.clone())
        );
        assert_eq!(standing_verdict(Some(blocked), at, at), None);
        assert_eq!(standing_verdict(None, None, None), None);
    }

    #[test]
    fn a_silent_worker_is_nudged_once_then_stalled() {
        let work = progress(2, true);
        assert!(matches!(
            route(STOPPED, &work, None, Nudged::Never, (None, None), NOW),
            Route::Nudge(_)
        ));
        // the nudge is a sign of life for one grace period…
        let just = Nudged::At(NOW - GRACE + 1);
        assert_eq!(route(STOPPED, &work, None, just, (None, None), NOW), Route::Leave);
        assert_eq!(quiet_for(Some(9000), &work, just, NOW), Some(GRACE - 1));
        // …and then the silence is the worker's last word
        let Route::Stalled(silent) =
            route(STOPPED, &work, None, Nudged::At(NOW - GRACE), (None, None), NOW)
        else {
            panic!("a second silence is not nudged again")
        };
        assert!(silent.nudged);
        assert!(stalled_reason(&silent, &Tail::of(&[])).ends_with("after one nudge."));
        // a nudge stamped from the future is no sign of life
        assert!(matches!(
            route(STOPPED, &work, None, Nudged::At(NOW * 2), (None, None), NOW),
            Route::Stalled(_)
        ));
    }

    #[test]
    fn a_worker_waiting_on_its_own_background_work_is_not_silent() {
        let work = progress(1, false);
        let running = Some(Background { since: NOW - 600, limit: 3600 });
        // neither nudged, nor stalled past its nudge, while the work runs
        for nudged in [Nudged::Never, Nudged::At(NOW - GRACE)] {
            assert_eq!(
                route(STOPPED, &work, None, nudged, (None, running), NOW),
                Route::Waiting(NOW - 600)
            );
        }
        // a stamp from the future is work just seen
        let just = Some(Background { since: NOW * 2, limit: 3600 });
        assert_eq!(
            route(STOPPED, &work, None, Nudged::Never, (None, just), NOW),
            Route::Waiting(NOW * 2)
        );
        // a sign of life within grace is lively as ever
        let lively = WorkerState::Idle { quiet: Some(GRACE - 1) };
        assert_eq!(route(lively, &work, None, Nudged::Never, (None, running), NOW), Route::Leave);
        // once it ends, the silence is counted as ever: a nudge, then stalled past it
        assert!(matches!(
            route(STOPPED, &work, None, Nudged::Never, (None, None), NOW),
            Route::Nudge(_)
        ));
        let Route::Stalled(silent) =
            route(STOPPED, &work, None, Nudged::At(NOW - GRACE), (None, None), NOW)
        else {
            panic!("stalled past its nudge")
        };
        assert_eq!(silent.waited, None);
        // a verdict is not kept waiting for it
        let verdict = said(Some(Phase::Escalate));
        assert_eq!(
            route(STOPPED, &work, Some(&verdict), Nudged::Never, (None, running), NOW),
            Route::Escalate(Why::Escalate)
        );
    }

    #[test]
    fn a_worker_waiting_past_the_limit_is_silent_and_says_so() {
        let past = Some(Background { since: NOW - 3600, limit: 3600 });
        for nudged in [Nudged::Never, Nudged::At(NOW - GRACE)] {
            let Route::Stalled(silent) =
                route(STOPPED, &progress(0, false), None, nudged, (None, past), NOW)
            else {
                panic!("waiting past the limit is a silence, nudged or not")
            };
            assert_eq!(silent.waited, Some(3600));
            let reason = stalled_reason(&silent, &Tail::of(&[]));
            assert!(
                reason.contains("waited 3600s on its own background work, past the limit"),
                "{reason}"
            );
        }
    }

    #[test]
    fn a_worker_just_started_is_not_nudged_and_its_verdict_is_not_kept_waiting() {
        let started = Some(NOW - GRACE + 1);
        assert_eq!(
            route(STOPPED, &progress(0, false), None, Nudged::Never, (started, None), NOW),
            Route::Leave
        );
        let long_ago = Some(NOW - GRACE);
        assert!(matches!(
            route(STOPPED, &progress(0, false), None, Nudged::Never, (long_ago, None), NOW),
            Route::Nudge(_)
        ));
        let verdict = said(Some(Phase::Escalate));
        assert_eq!(
            route(
                STOPPED,
                &progress(0, false),
                Some(&verdict),
                Nudged::Never,
                (started, None),
                NOW
            ),
            Route::Escalate(Why::Escalate)
        );
    }

    #[test]
    fn with_nowhere_to_record_a_nudge_the_silence_stalls_at_once() {
        let Route::Stalled(silent) =
            route(STOPPED, &progress(0, false), None, Nudged::Nowhere, (None, None), NOW)
        else {
            panic!("nothing can be nudged where no worktree is")
        };
        assert!(!silent.nudged);
        assert!(stalled_reason(&silent, &Tail::of(&[])).contains("no worktree"));
    }

    #[test]
    fn a_verdict_is_never_overridden_by_a_nudge() {
        let human = said(Some(Phase::Escalate));
        for nudged in [Nudged::Never, Nudged::At(1), Nudged::Nowhere] {
            assert_eq!(
                route(STOPPED, &progress(0, false), Some(&human), nudged, (None, None), NOW),
                Route::Escalate(Why::Escalate)
            );
        }
        // nor is a worker at work, or one gone, told anything
        let gone =
            route(WorkerState::Gone, &progress(0, false), None, Nudged::Never, (None, None), NOW);
        assert_eq!(gone, Route::Continue);
        let busy =
            route(WorkerState::Busy, &progress(0, false), None, Nudged::Never, (None, None), NOW);
        assert_eq!(busy, Route::Leave);
    }

    #[test]
    fn an_orphaned_claim_is_continued_whatever_was_declared() {
        assert_eq!(settled_by_state(None), Some(Route::Continue));
        assert_eq!(settled_by_state(Some(WorkerState::Gone)), Some(Route::Continue));
        assert_eq!(settled_by_state(Some(WorkerState::Busy)), Some(Route::Leave));
        assert_eq!(settled_by_state(Some(STOPPED)), None);
        let human = said(Some(Phase::Escalate));
        assert_eq!(
            route(
                WorkerState::Gone,
                &progress(3, true),
                Some(&human),
                Nudged::At(1),
                (None, None),
                NOW
            ),
            Route::Continue
        );
    }

    #[test]
    fn a_nudge_tells_then_records() {
        let take =
            || match route(STOPPED, &progress(0, false), None, Nudged::Never, (None, None), NOW) {
                Route::Nudge(unnudged) => unnudged,
                other => panic!("{other:?}"),
            };
        let mut steps = vec![];
        let done: Result<(), ()> = nudge(take(), |step| {
            steps.push(step);
            Ok(())
        });
        assert_eq!((done, steps), (Ok(()), vec![NudgeStep::Tell, NudgeStep::Record]));
        // a line the worker did not take is not recorded: the next tick nudges again
        let mut steps = vec![];
        let refused = nudge(take(), |step| {
            steps.push(step);
            Err("refused")
        });
        assert_eq!((refused, steps), (Err("refused"), vec![NudgeStep::Tell]));
    }

    #[test]
    fn a_tail_is_the_end_of_the_screen_bounded() {
        let screen: Vec<String> = (0..100)
            .map(|n| if n % 2 == 0 { format!("line {n}   ") } else { "   ".to_string() })
            .collect();
        let tail = Tail::of(&screen);
        let text = tail.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), SCREEN_LINES);
        assert_eq!((lines[0], lines[SCREEN_LINES - 1]), ("line 40", "line 98"));
        let wide = Tail::of(&["é".repeat(500)]);
        assert_eq!(wide.text().chars().count(), SCREEN_LINE_CHARS);
        assert!(Tail::of(&["  ".to_string()]).is_empty());
        let silent =
            match route(STOPPED, &progress(0, false), None, Nudged::At(1), (None, None), NOW) {
                Route::Stalled(silent) => silent,
                other => panic!("{other:?}"),
            };
        assert_eq!(screen_request(&silent), 60);
        let reason = stalled_reason(&silent, &Tail::of(&["Proceed? [y/n]".to_string()]));
        assert!(reason.ends_with("```\nProceed? [y/n]\n```"), "{reason}");
    }
}

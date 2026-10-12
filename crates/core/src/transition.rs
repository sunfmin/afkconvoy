//! The transitions that end a claim other than a landing: `escalate`, `close`,
//! `park` and `hand_on`. Each runs in one order, writes the status board's last word,
//! releases the claim as its last write to the remote, and hands back a
//! `Released` only that release produces — so one cannot end half way and be
//! taken for done.
//!
//! The order is decided here; the effect layer hands in the one function that
//! carries a step out.

use flux_rs::attrs::*;

use crate::blocked::Parkable;
use crate::config::Config;
use crate::record;
use crate::stopped::Workless;
use crate::team::FLOW_LABEL;

/// What is left of a transition once its claim is gone: nothing to act on.
#[derive(Debug, PartialEq, Eq)]
pub struct Released {
    issue: u64,
}

impl Released {
    pub fn issue(&self) -> u64 {
        self.issue
    }
}

/// A claim's status board as its claim ends: whether it says how.
#[refined_by(written: bool)]
struct LastWord {
    #[field(bool[written])]
    written: bool,
}

impl LastWord {
    #[spec(fn() -> LastWord[false])]
    fn unwritten() -> LastWord {
        LastWord { written: false }
    }

    #[spec(fn(self: &mut Self[false]) ensures self: Self[true])]
    fn write(&mut self) {
        self.written = true;
    }

    /// The one way to a `Released`: released sooner, the claim is nobody's,
    /// and the board keeps saying the issue is being worked.
    #[spec(fn(&Self[true], issue: u64) -> Released)]
    fn release(&self, issue: u64) -> Released {
        Released { issue }
    }
}

const ESCALATION: &str = "convoy:escalation";

/// How far an escalation got. It is not known ahead of time: an escalation
/// cut short is finished by a later run, which reads from GitHub what was
/// done.
#[derive(Debug)]
#[refined_by(commented: bool, relabelled: bool)]
pub struct Escalation {
    #[field(bool[commented])]
    commented: bool,
    #[field(bool[relabelled])]
    relabelled: bool,
}

impl Escalation {
    /// An escalation as GitHub shows it: any combination may come back.
    #[spec(fn(commented: bool, relabelled: bool) -> Escalation[commented, relabelled])]
    pub fn read(commented: bool, relabelled: bool) -> Escalation {
        Escalation { commented, relabelled }
    }

    #[spec(fn(&Self[@e]) -> bool[e.commented])]
    pub fn commented(&self) -> bool {
        self.commented
    }

    #[spec(fn(&Self[@e]) -> bool[e.relabelled])]
    pub fn relabelled(&self) -> bool {
        self.relabelled
    }

    #[spec(fn(self: &mut Self[@e]) ensures self: Self[true, e.relabelled])]
    fn comment(&mut self) {
        self.commented = true;
    }

    /// The reason first: an issue is never handed over with nothing said.
    #[spec(fn(self: &mut Self[@e]) requires e.commented ensures self: Self[e.commented, true])]
    fn relabel(&mut self) {
        self.relabelled = true;
    }

    /// The claim last: until it goes, the escalation is this fleet's to finish.
    #[spec(fn(Self{e: e.commented && e.relabelled}, word: &LastWord[true], issue: u64) -> Released)]
    fn release(self, word: &LastWord, issue: u64) -> Released {
        word.release(issue)
    }
}

/// One step of an escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalateStep {
    /// Comment the reason on the issue.
    Comment,
    /// Swap what hands the issue to the fleet for the escalate label (`Relabel`).
    Relabel,
    /// Write the status board: escalated.
    Board,
    /// Delete the claim.
    Release,
    /// Remove the worktree: only one whose branch holds no work.
    RemoveWorktree,
}

/// Escalates issue `issue` from wherever `begun` says it got: does what is
/// left, in order, and releases last. The PR, the branch and the worktree are
/// kept as evidence — except a worktree whose branch holds no work
/// (`workless`), which goes once the claim has. A step that fails ends it
/// there, the claim still held.
pub fn escalate<E, F>(
    issue: u64,
    mut begun: Escalation,
    workless: Option<&Workless>,
    mut carry_out: F,
) -> Result<Released, E>
where
    F: FnMut(EscalateStep) -> Result<(), E>,
{
    if !begun.commented() {
        carry_out(EscalateStep::Comment)?;
        begun.comment();
    }
    if !begun.relabelled() {
        carry_out(EscalateStep::Relabel)?;
        begun.relabel();
    }
    let mut word = LastWord::unwritten();
    carry_out(EscalateStep::Board)?;
    word.write();
    carry_out(EscalateStep::Release)?;
    let released = begun.release(&word, issue);
    if workless.is_some() {
        carry_out(EscalateStep::RemoveWorktree)?;
    }
    Ok(released)
}

/// The comment an escalation leaves on the issue, under the record of whose
/// it is: the claim it ends, by its sha — a claim ref is written once, so each
/// claim of an issue has its own.
pub fn escalation_comment(claim: &str, reason: &str) -> String {
    let marker = record::marker(ESCALATION, &[("claim", claim)]);
    format!("{marker}\n**afkconvoy: escalated to a human.**\n\n{}\n", reason.trim())
}

/// Do an issue's comments (oldest first) already carry the escalation of the
/// claim `claim`? An issue's earlier escalations were of other claims: each
/// ended in a release, and the issue was claimed anew.
pub fn escalation_begun<'a>(bodies: impl IntoIterator<Item = &'a str>, claim: &str) -> bool {
    match record::latest_marker(ESCALATION, bodies) {
        Some((_, fields)) => fields.get("claim") == Some(claim),
        None => false,
    }
}

/// Is the issue labelled as handed over: `escalate_label` on, `ready_label` off?
pub fn relabelled(labels: &[String], config: &Config) -> bool {
    Relabel::of(labels, config, None).done()
}

/// The labels an escalation swaps on one issue: what hands it to the fleet
/// comes off, and the escalate label goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relabel {
    /// The escalate label, when the issue lacks it.
    pub add: Option<String>,
    /// What it carries that hands it to the fleet: the ready label, or — in
    /// a bound repo — every flow label.
    pub remove: Vec<String>,
}

impl Relabel {
    /// The swap for an issue carrying `labels`. `team_label` is the escalate
    /// label of the team a bound repo follows; `None` in an unbound repo,
    /// whose own config names it.
    pub fn of(labels: &[String], config: &Config, team_label: Option<&str>) -> Relabel {
        let escalate = team_label.unwrap_or(&config.escalate_label);
        let mut remove = vec![];
        for label in labels {
            let hands_over = match team_label {
                Some(_) => label.starts_with(FLOW_LABEL),
                None => *label == config.ready_label,
            };
            if hands_over {
                remove.push(label.clone());
            }
        }
        let carried = labels.iter().any(|label| label == escalate);
        Relabel { add: if carried { None } else { Some(escalate.to_string()) }, remove }
    }

    /// Whether nothing is left to swap.
    pub fn done(&self) -> bool {
        self.add.is_none() && self.remove.is_empty()
    }
}

/// One step of closing an issue that needed no change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseStep {
    CloseIssue,
    /// Write the status board: closed with nothing landed.
    Board,
    Release,
    RemoveWorktree,
}

/// Closes issue `issue` as needing no change: close it → release the claim →
/// remove the worktree. It asks for the proof that the branch holds no work.
pub fn close<E, F>(issue: u64, _workless: &Workless, mut carry_out: F) -> Result<Released, E>
where
    F: FnMut(CloseStep) -> Result<(), E>,
{
    // Closed first: a claim released on an open issue puts it back on the frontier.
    carry_out(CloseStep::CloseIssue)?;
    let mut word = LastWord::unwritten();
    carry_out(CloseStep::Board)?;
    word.write();
    carry_out(CloseStep::Release)?;
    let released = word.release(issue);
    carry_out(CloseStep::RemoveWorktree)?;
    Ok(released)
}

/// One step of parking a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkStep {
    /// Record the issue as blocked by `blocker` — a native dependency edge;
    /// one already there is left as it is.
    Edge {
        blocker: u64,
    },
    /// Write the status board: parked.
    Board,
    Release,
    RemoveWorktree,
}

/// A park under way: whether an edge holds the issue off the frontier yet.
#[refined_by(edged: bool)]
struct Parking {
    #[field(bool[edged])]
    edged: bool,
}

impl Parking {
    #[spec(fn() -> Parking[false])]
    fn begin() -> Parking {
        Parking { edged: false }
    }

    #[spec(fn(self: &mut Self, blocker: u64) -> ParkStep ensures self: Self[true])]
    fn edge(&mut self, blocker: u64) -> ParkStep {
        self.edged = true;
        ParkStep::Edge { blocker }
    }

    /// The claim goes only once an edge is written: released first, the issue
    /// is on the frontier — ready, unclaimed, unblocked — for whoever plans next.
    #[spec(fn(&Self[true], word: &LastWord[true], issue: u64) -> Released)]
    fn release(&self, word: &LastWord, issue: u64) -> Released {
        word.release(issue)
    }
}

/// Parks the claim on `issue`: an edge to every open blocker its worker named
/// → release the claim → remove the worktree, when its branch holds no work.
/// `ready_label` is not touched: the edges hold the issue back, and it returns
/// to the frontier by itself once the last blocker closes.
pub fn park<E, F>(
    issue: u64,
    parkable: &Parkable,
    workless: Option<&Workless>,
    mut carry_out: F,
) -> Result<Released, E>
where
    F: FnMut(ParkStep) -> Result<(), E>,
{
    let mut parking = Parking::begin();
    carry_out(parking.edge(parkable.first()))?;
    for blocker in parkable.rest() {
        carry_out(parking.edge(*blocker))?;
    }
    let mut word = LastWord::unwritten();
    carry_out(ParkStep::Board)?;
    word.write();
    carry_out(ParkStep::Release)?;
    let released = parking.release(&word, issue);
    // A worktree with work in it is what a later continuation starts from.
    if workless.is_some() {
        carry_out(ParkStep::RemoveWorktree)?;
    }
    Ok(released)
}

/// One step of ending the claim of a stage that declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandOnStep {
    /// Write the status board: the stage declared.
    Board,
    Release,
    RemoveWorktree,
}

/// Ends the claim on `issue` once its stage has declared, leaving the issue
/// where its stage records put it: write the board → release the claim. The
/// issue's labels are not touched: a later stage is the frontier's to start,
/// in the same worktree. `ended` is given when the workflow has ended, with
/// the proof that the branch holds no work when it does not: only then does
/// the worktree go.
pub fn hand_on<E, F>(issue: u64, ended: Option<&Workless>, mut carry_out: F) -> Result<Released, E>
where
    F: FnMut(HandOnStep) -> Result<(), E>,
{
    let mut word = LastWord::unwritten();
    carry_out(HandOnStep::Board)?;
    word.write();
    carry_out(HandOnStep::Release)?;
    let released = word.release(issue);
    if ended.is_some() {
        carry_out(HandOnStep::RemoveWorktree)?;
    }
    Ok(released)
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    fn said() -> LastWord {
        let mut word = LastWord::unwritten();
        word.write();
        word
    }

    /// A claim released while its status board still says it is being worked.
    #[should_fail]
    fn released_before_the_board_says_how_it_ended(issue: u64) -> Released {
        LastWord::unwritten().release(issue)
    }

    /// A parked claim released before any edge holds its issue back.
    #[should_fail]
    fn parked_with_no_edge(issue: u64) -> Released {
        Parking::begin().release(&said(), issue)
    }

    /// A claim released with nothing said on the issue.
    #[should_fail]
    fn released_before_the_reason_is_commented(issue: u64) -> Released {
        Escalation::read(false, true).release(&said(), issue)
    }

    /// A claim released with the issue still labelled ready: back on the frontier.
    #[should_fail]
    fn released_before_the_relabel(issue: u64) -> Released {
        let mut begun = Escalation::read(false, false);
        begun.comment();
        begun.release(&said(), issue)
    }

    /// Whatever GitHub showed, taken for an escalation that is complete.
    #[should_fail]
    fn released_on_whatever_was_read(issue: u64, commented: bool, relabelled: bool) -> Released {
        Escalation::read(commented, relabelled).release(&said(), issue)
    }

    /// An issue relabelled with no reason on it.
    #[should_fail]
    fn relabelled_before_the_comment() {
        let mut begun = Escalation::read(false, false);
        begun.relabel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocked::{Blocked, Standing, blocked_route};
    use crate::config::Settings;
    use crate::stopped::Progress;

    fn steps(
        begun: Escalation,
        workless: bool,
        fail_at: Option<EscalateStep>,
    ) -> (Vec<EscalateStep>, bool) {
        let clean =
            Progress { commits_ahead: Some(0), dirty: false, last_commit: None, touched: None };
        let proof = Workless::of(&clean).filter(|_| workless);
        let mut did = vec![];
        let ended = escalate(7, begun, proof.as_ref(), |step| {
            if fail_at == Some(step) {
                return Err(());
            }
            did.push(step);
            Ok(())
        });
        (did, ended.is_ok())
    }

    #[test]
    fn an_escalation_does_what_is_left_in_order_and_releases_last() {
        use EscalateStep::*;
        assert_eq!(
            steps(Escalation::read(false, false), false, None),
            (vec![Comment, Relabel, Board, Release], true)
        );
        assert_eq!(
            steps(Escalation::read(true, false), false, None),
            (vec![Relabel, Board, Release], true)
        );
        assert_eq!(
            steps(Escalation::read(true, true), true, None),
            (vec![Board, Release, RemoveWorktree], true)
        );
        // a relabel seen with no comment of this claim's: the reason is still owed
        assert_eq!(
            steps(Escalation::read(false, true), false, None),
            (vec![Comment, Board, Release], true)
        );
    }

    #[test]
    fn a_step_that_fails_ends_it_with_the_claim_held() {
        use EscalateStep::*;
        assert_eq!(
            steps(Escalation::read(false, false), true, Some(Relabel)),
            (vec![Comment], false)
        );
        assert_eq!(
            steps(Escalation::read(false, false), true, Some(Release)),
            (vec![Comment, Relabel, Board], false)
        );
    }

    #[test]
    fn an_escalation_is_recognised_by_the_claim_it_ends() {
        let comment = escalation_comment("abc123", "  stuck on the schema \n");
        assert!(comment.starts_with("<!--convoy:escalation claim=abc123-->\n"));
        assert!(comment.contains("\n\nstuck on the schema\n"));
        assert!(escalation_begun([comment.as_str()], "abc123"));
        assert!(!escalation_begun([comment.as_str()], "def456"), "an earlier claim's");
        assert!(!escalation_begun(["no marker"], "abc123"));
    }

    #[test]
    fn handed_over_means_the_escalate_label_on_and_the_ready_label_off() {
        let config = Config::new(Settings {
            base_branch: Some("main".into()),
            gate_command: Some("true".into()),
            ..Settings::default()
        })
        .unwrap();
        let labels = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert!(relabelled(&labels(&["ready-for-human"]), &config));
        assert!(!relabelled(&labels(&["ready-for-human", "ready-for-agent"]), &config));
        assert!(!relabelled(&labels(&["bug"]), &config));
    }

    #[test]
    fn in_a_bound_repo_every_flow_label_is_swapped_for_the_team_s_escalate_label() {
        let config = Config::new(Settings {
            base_branch: Some("main".into()),
            gate_command: Some("true".into()),
            team: Some("acme/team".into()),
            ..Settings::default()
        })
        .unwrap();
        let labels = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let carried = labels(&["flow:feature", "bug", "flow:chore", "ready-for-agent"]);
        let swap = Relabel::of(&carried, &config, Some("for-a-human"));
        assert_eq!(swap.add.as_deref(), Some("for-a-human"));
        assert_eq!(
            swap.remove,
            ["flow:feature", "flow:chore"],
            "the ready label means nothing here"
        );
        assert!(!swap.done());
        assert!(Relabel::of(&labels(&["bug", "for-a-human"]), &config, Some("for-a-human")).done());
    }

    #[test]
    fn handing_on_releases_after_the_board_and_keeps_a_worktree_another_stage_needs() {
        use HandOnStep::*;
        let run = |ended: Option<&Workless>, fail_at: Option<HandOnStep>| {
            let mut steps = vec![];
            let done = hand_on(7, ended, |step| {
                steps.push(step);
                if Some(step) == fail_at { Err("cut") } else { Ok(()) }
            });
            (done.map(|released| released.issue), steps)
        };
        assert_eq!(run(None, None), (Ok(7), vec![Board, Release]));
        let empty =
            Progress { commits_ahead: Some(0), dirty: false, last_commit: None, touched: None };
        let workless = Workless::of(&empty).unwrap();
        assert_eq!(run(Some(&workless), None), (Ok(7), vec![Board, Release, RemoveWorktree]));
        assert_eq!(run(None, Some(Board)), (Err("cut"), vec![Board]), "the claim is still held");
    }

    #[test]
    fn close_releases_after_the_issue_is_closed() {
        let clean =
            Progress { commits_ahead: Some(0), dirty: false, last_commit: None, touched: None };
        let proof = Workless::of(&clean).unwrap();
        let mut did = vec![];
        let released = close::<(), _>(7, &proof, |step| {
            did.push(step);
            Ok(())
        });
        assert_eq!(released.unwrap().issue(), 7);
        assert_eq!(
            did,
            [
                CloseStep::CloseIssue,
                CloseStep::Board,
                CloseStep::Release,
                CloseStep::RemoveWorktree
            ],
            "the board's last word, then the claim"
        );
        let cut =
            close(7, &proof, |step| if step == CloseStep::Release { Err("no") } else { Ok(()) });
        assert_eq!(cut, Err("no"));
    }

    #[test]
    fn park_writes_every_edge_before_it_releases() {
        let waiting = [(41, Standing::Waiting), (42, Standing::Closed), (43, Standing::Waiting)];
        let Blocked::Park(parkable) = blocked_route(&waiting) else { panic!() };
        let run = |workless: Option<&Workless>, fail_at: Option<ParkStep>| {
            let mut steps = vec![];
            let done = park(7, &parkable, workless, |step| {
                steps.push(step);
                if Some(step) == fail_at { Err("cut") } else { Ok(()) }
            });
            (done.map(|released| released.issue), steps)
        };
        let edges = [ParkStep::Edge { blocker: 41 }, ParkStep::Edge { blocker: 43 }];
        // a branch with work on it keeps its worktree
        let (done, steps) = run(None, None);
        assert_eq!(done, Ok(7));
        assert_eq!(steps, [edges[0], edges[1], ParkStep::Board, ParkStep::Release]);
        let empty =
            Progress { commits_ahead: Some(0), dirty: false, last_commit: None, touched: None };
        let workless = Workless::of(&empty).unwrap();
        let (_, steps) = run(Some(&workless), None);
        assert_eq!(
            steps,
            [edges[0], edges[1], ParkStep::Board, ParkStep::Release, ParkStep::RemoveWorktree]
        );
        // an edge that failed: the claim is still held
        let (done, steps) = run(None, Some(edges[1]));
        assert_eq!((done, steps), (Err("cut"), edges.to_vec()));
    }
}

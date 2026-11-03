//! The transitions that end a claim other than a landing: `escalate`, `close`,
//! `park`, `hand_on` and `forget`. Each runs in one order, writes the status board's last word,
//! leaves the work account where its end is one that does (`End`),
//! releases the claim as its last write to the remote, and hands back a
//! `Released` only that release produces — so one cannot end half way and be
//! taken for done.
//!
//! Each ends the claim's worker too, once the claim has gone: its worktree is
//! removed where nothing will use it again, and its terminal closed where the
//! worktree is kept. Sooner, a claim still held would be one with no worker,
//! and be continued.
//!
//! The order is decided here; the effect layer hands in the one function that
//! carries a step out.

use flux_rs::attrs::*;

use crate::account::End;
use crate::blocked::{Hold, Parkable};
use crate::config::Config;
use crate::left::Spare;
use crate::record;
use crate::stopped::Workless;
use crate::team::Team;

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

    /// The board of an issue that is gone: it went with the issue, and
    /// nothing is left to write it on.
    #[spec(fn() -> LastWord[true])]
    fn gone() -> LastWord {
        LastWord { written: true }
    }

    /// The one way to a `Released`: released sooner, the claim is nobody's,
    /// and the board keeps saying the issue is being worked — or the issue
    /// ended with no account, and no later end has a claim to leave it under.
    #[spec(fn(&Self[true], account: &Account[true], issue: u64) -> Released)]
    fn release(&self, _account: &Account, issue: u64) -> Released {
        Released { issue }
    }
}

/// A claim's work account as its claim ends: `left` once its end is
/// accounted for — an end that leaves none, or one whose account is left.
#[refined_by(left: bool)]
struct Account {
    #[field(bool[left])]
    left: bool,
}

impl Account {
    /// What `end` owes: an account, where it is an end that leaves one.
    #[spec(fn(&End[@accounted]) -> Account[!accounted])]
    fn owed(end: &End) -> Account {
        Account { left: !end.accounted() }
    }

    #[spec(fn(&Self[@account]) -> bool[account])]
    fn left(&self) -> bool {
        self.left
    }

    #[spec(fn(self: &mut Self) ensures self: Self[true])]
    fn leave(&mut self) {
        self.left = true;
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

    #[spec(fn(self: &mut Self[@e]) ensures self: Self[{ commented: true, ..e }])]
    fn comment(&mut self) {
        self.commented = true;
    }

    /// The reason first: an issue is never handed over with nothing said.
    #[spec(fn(self: &mut Self[@e]) requires e.commented ensures self: Self[{ relabelled: true, ..e }])]
    fn relabel(&mut self) {
        self.relabelled = true;
    }

    /// The claim last: until it goes, the escalation is this fleet's to finish.
    #[spec(fn(Self{e: e.commented && e.relabelled}, word: &LastWord[true], account: &Account[true], issue: u64) -> Released)]
    fn release(self, word: &LastWord, account: &Account, issue: u64) -> Released {
        word.release(account, issue)
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
    /// Leave the work account.
    Account,
    /// Delete the claim.
    Release,
    /// Remove the worktree: only one holding nothing the remote does not.
    RemoveWorktree,
    /// Close the worker's terminal, in a worktree that is kept.
    CloseTerminal,
}

/// Escalates issue `issue` from wherever `begun` says it got: does what is
/// left, in order, and releases last. The PR and the branch are kept as
/// evidence, and so is the worktree — except one holding nothing the remote
/// does not (`spare`), which goes once the claim has; one that is kept has its
/// worker's terminal closed. A step that fails ends it there, the claim still
/// held unless it was the last.
pub fn escalate<E, F>(
    issue: u64,
    mut begun: Escalation,
    spare: Option<&Spare>,
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
    let mut account = Account::owed(&End::Escalated);
    if !account.left() {
        carry_out(EscalateStep::Account)?;
        account.leave();
    }
    carry_out(EscalateStep::Release)?;
    let released = begun.release(&word, &account, issue);
    carry_out(match spare {
        Some(_) => EscalateStep::RemoveWorktree,
        None => EscalateStep::CloseTerminal,
    })?;
    Ok(released)
}

/// The comment an escalation leaves on the issue, under the record of whose
/// it is: the claim it ends, by the commit it was created at (`Claim::id`) —
/// each claim of an issue has its own, and keeps it through a takeover. `flow`
/// is the workflow a bound repo's issue follows as it is handed over: its
/// flow label comes off, and this is what
/// says which to put back.
pub fn escalation_comment(claim: &str, flow: Option<&str>, reason: &str) -> String {
    let marker = record::marker(ESCALATION, &[("claim", claim), ("flow", flow.unwrap_or(""))]);
    format!("{marker}\n**afkconvoy: escalated to a human.**\n\n{}\n", reason.trim())
}

/// No harness can work this issue after refusal: a consult would read the
/// same task on the same exhausted kit. Keep it with the human until ruled on.
pub fn refusal_exhausted_comment(claim: &str, flow: Option<&str>, reason: &str) -> String {
    let marker = record::marker(
        ESCALATION,
        &[("claim", claim), ("flow", flow.unwrap_or("")), ("refusal", "exhausted")],
    );
    format!("{marker}\n**afkconvoy: escalated to a human.**\n\n{}\n", reason.trim())
}

/// Whether this escalation cannot be answered by a consult on this host.
pub fn skips_consult(body: &str) -> bool {
    FleetFault::read(body).is_some()
        || record::read_marker(ESCALATION, body)
            .is_some_and(|fields| fields.get("refusal") == Some("exhausted"))
}

/// A hand-off caused by the fleet, its harness or its host, before a worker
/// took its brief or after the landing machinery stopped. These go straight
/// to a human: a consult may not fix the host (docs/adr/0030).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetFault {
    WorkerStart,
    WorktreeTrust,
}

impl FleetFault {
    fn word(self) -> &'static str {
        match self {
            FleetFault::WorkerStart => "worker-start",
            FleetFault::WorktreeTrust => "worktree-trust",
        }
    }

    /// The fault in the few words a tick's line has room for.
    pub fn brief(self) -> &'static str {
        match self {
            FleetFault::WorkerStart => "its worker start failed again",
            FleetFault::WorktreeTrust => "its harness's worktree trust gap",
        }
    }

    /// An escalation whose marker keeps later ticks from asking a consult.
    /// Its first sentence tells the operator where to fix it, also in the
    /// `waiting_on_human` reason that `doctor` reads from this comment.
    pub fn comment(self, claim: &str, flow: Option<&str>, reason: &str) -> String {
        let marker = record::marker(
            ESCALATION,
            &[("claim", claim), ("flow", flow.unwrap_or("")), ("fault", self.word())],
        );
        format!(
            "{marker}\n**afkconvoy: escalated to a human.**\n\n\
             Fleet fault: {}; the fix is on the host.\n\n{}\n",
            self.brief(),
            reason.trim()
        )
    }

    /// The fault recorded by this escalation, or none for a hand-off about
    /// the work. Only the fleet's marker is read, never its quoted evidence.
    pub fn read(body: &str) -> Option<FleetFault> {
        let fields = record::read_marker(ESCALATION, body)?;
        match fields.get("fault")? {
            "worker-start" => Some(FleetFault::WorkerStart),
            "worktree-trust" => Some(FleetFault::WorktreeTrust),
            _ => None,
        }
    }
}

/// The workflow an issue followed when it was last escalated, by its comments
/// (oldest first); `None` when it never was, or its last escalation names none.
pub fn escalated_from<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let (_, fields) = record::latest_marker(ESCALATION, bodies)?;
    fields.get("flow").map(str::to_string)
}

/// Which of an issue's comments (oldest first) is its latest escalation's;
/// `None` of an issue the fleet never escalated.
pub fn escalated_at<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Option<usize> {
    record::latest_marker(ESCALATION, bodies).map(|(at, _)| at)
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

/// The labels swapped on one issue. An escalation takes off what hands the
/// issue to the fleet and puts the escalate label on; `back` is the reverse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relabel {
    /// The escalate label, when the issue lacks it.
    pub add: Option<String>,
    /// What it carries that hands it to the fleet: the ready label, or — in
    /// a bound repo — every flow label.
    pub remove: Vec<String>,
}

impl Relabel {
    /// The swap for an issue carrying `labels`. `team` is the team a bound
    /// repo follows, which names its escalate label and its flow labels;
    /// `None` in an unbound repo, whose own config names the escalate label.
    pub fn of(labels: &[String], config: &Config, team: Option<&Team>) -> Relabel {
        let escalate = team.map_or(&config.escalate_label, |team| &team.escalate_label);
        let mut remove = vec![];
        let flows = team.map(|team| team.flows(labels));
        for label in labels {
            let hands_over = match &flows {
                Some(flows) => flows.contains(&label.as_str()),
                None => *label == config.ready_label,
            };
            if hands_over {
                remove.push(label.clone());
            }
        }
        let carried = labels.iter().any(|label| label == escalate);
        Relabel { add: if carried { None } else { Some(escalate.to_string()) }, remove }
    }

    /// The swap back, for an issue of a bound repo a human put at a stage
    /// again: its flow label `flow` goes on, and the team's escalate label
    /// `escalate` comes off.
    pub fn back(labels: &[String], flow: &str, escalate: &str) -> Relabel {
        let carries = |wanted: &str| labels.iter().any(|label| label == wanted);
        Relabel {
            add: if carries(flow) { None } else { Some(flow.to_string()) },
            remove: if carries(escalate) { vec![escalate.to_string()] } else { vec![] },
        }
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
    /// Leave the work account.
    Account,
    Release,
    RemoveWorktree,
}

/// Closes issue `issue` as needing no change: close it → write the board →
/// leave the work account → release the claim → remove the worktree. It asks for the proof that the branch holds no work.
pub fn close<E, F>(issue: u64, _workless: &Workless, mut carry_out: F) -> Result<Released, E>
where
    F: FnMut(CloseStep) -> Result<(), E>,
{
    // Closed first: a claim released on an open issue puts it back on the frontier.
    carry_out(CloseStep::CloseIssue)?;
    let mut word = LastWord::unwritten();
    carry_out(CloseStep::Board)?;
    word.write();
    let mut account = Account::owed(&End::Closed);
    if !account.left() {
        carry_out(CloseStep::Account)?;
        account.leave();
    }
    carry_out(CloseStep::Release)?;
    let released = word.release(&account, issue);
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
    /// Record the issue as waiting on the next cut of its repo — the label
    /// that says so; one already there is left as it is.
    Cut,
    /// Write the status board: parked.
    Board,
    Release,
    RemoveWorktree,
    /// Close the worker's terminal, in a worktree that is kept.
    CloseTerminal,
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

    /// What `hold` writes: an edge, or the label that waits on the cut.
    #[spec(fn(self: &mut Self, hold: Hold) -> ParkStep ensures self: Self[true])]
    fn hold(&mut self, hold: Hold) -> ParkStep {
        self.edged = true;
        match hold {
            Hold::Issue(blocker) => ParkStep::Edge { blocker },
            Hold::Cut => ParkStep::Cut,
        }
    }

    /// The claim goes only once an edge is written: released first, the issue
    /// is on the frontier — ready, unclaimed, unblocked — for whoever plans next.
    #[spec(fn(&Self[true], word: &LastWord[true], issue: u64) -> Released)]
    fn release(&self, word: &LastWord, issue: u64) -> Released {
        // Work goes on once its blockers close: a park leaves no account.
        word.release(&Account::owed(&End::Parked), issue)
    }
}

/// Parks the claim on `issue`: the label that waits on the next cut, where it
/// does, and an edge to every open blocker its worker named → release the claim → remove the worktree, when it holds nothing the remote
/// does not (`spare`) — a later dispatch recreates it at the pushed tip — and
/// close its worker's terminal when it does.
/// `ready_label` is not touched: the edges hold the issue back, and it returns
/// to the frontier by itself once the last blocker closes — and, waiting on the
/// cut, once a cut has installed a version and taken the label off.
pub fn park<E, F>(
    issue: u64,
    parkable: &Parkable,
    spare: Option<&Spare>,
    mut carry_out: F,
) -> Result<Released, E>
where
    F: FnMut(ParkStep) -> Result<(), E>,
{
    let mut parking = Parking::begin();
    carry_out(parking.hold(parkable.first()))?;
    for blocker in parkable.rest() {
        carry_out(parking.edge(*blocker))?;
    }
    let mut word = LastWord::unwritten();
    carry_out(ParkStep::Board)?;
    word.write();
    carry_out(ParkStep::Release)?;
    let released = parking.release(&word, issue);
    // A worktree holding what only it has is what a later continuation starts from.
    carry_out(match spare {
        Some(_) => ParkStep::RemoveWorktree,
        None => ParkStep::CloseTerminal,
    })?;
    Ok(released)
}

/// One step of ending the claim of a stage that declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandOnStep {
    /// Write the status board: the stage declared.
    Board,
    /// Leave the work account, at an end that leaves one.
    Account,
    Release,
    /// Remove the worktree: no stage is left to work in it.
    RemoveWorktree,
    /// Close the worker's terminal, in the worktree the next stage works in.
    CloseTerminal,
}

/// Ends the claim on `issue` once its stage has declared, leaving the issue
/// where its stage records put it: write the board → leave the work account,
/// where `end` is one that leaves it → release the claim → end its worker.
/// The issue's labels are not touched: a later stage is the frontier's to
/// start, in the same worktree and a terminal of its own. `end` is
/// `HandedOn`, `Closed` by a consult, or `Ended`, the workflow having ended:
/// only then does the worktree go, whatever its branch holds — a branch with a
/// commit past the base branch outlives it.
pub fn hand_on<E, F>(issue: u64, end: End, mut carry_out: F) -> Result<Released, E>
where
    F: FnMut(HandOnStep) -> Result<(), E>,
{
    let mut word = LastWord::unwritten();
    carry_out(HandOnStep::Board)?;
    word.write();
    let mut account = Account::owed(&end);
    if !account.left() {
        carry_out(HandOnStep::Account)?;
        account.leave();
    }
    carry_out(HandOnStep::Release)?;
    let released = word.release(&account, issue);
    carry_out(match end {
        End::Ended => HandOnStep::RemoveWorktree,
        _ => HandOnStep::CloseTerminal,
    })?;
    Ok(released)
}

/// One step of letting go of a claim whose issue is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetStep {
    Release,
    RemoveWorktree,
    /// Close the worker's terminal, in a worktree that is kept.
    CloseTerminal,
}

/// Lets go of the claim on `issue`, which GitHub says was deleted,
/// transferred or never issued: release the claim → remove the worktree, when it holds
/// nothing the remote does not (`spare`), and close its worker's terminal
/// when it does. Nothing is written on GitHub: the issue, its board and its
/// labels went together.
pub fn forget<E, F>(issue: u64, spare: Option<&Spare>, mut carry_out: F) -> Result<Released, E>
where
    F: FnMut(ForgetStep) -> Result<(), E>,
{
    carry_out(ForgetStep::Release)?;
    let released = LastWord::gone().release(&Account::owed(&End::Forgotten), issue);
    carry_out(match spare {
        Some(_) => ForgetStep::RemoveWorktree,
        None => ForgetStep::CloseTerminal,
    })?;
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

    fn accounted() -> Account {
        let mut account = Account::owed(&End::Escalated);
        account.leave();
        account
    }

    /// A claim released while its status board still says it is being worked.
    #[should_fail]
    fn released_before_the_board_says_how_it_ended(issue: u64) -> Released {
        LastWord::unwritten().release(&Account::owed(&End::Parked), issue)
    }

    /// A claim released at an end that leaves the work account, with none left.
    #[should_fail]
    fn released_before_the_account_is_left(issue: u64) -> Released {
        said().release(&Account::owed(&End::Escalated), issue)
    }

    /// Whatever end was read, taken for one that leaves no account.
    #[should_fail]
    fn released_on_whatever_end(issue: u64, end: End) -> Released {
        said().release(&Account::owed(&end), issue)
    }

    /// A parked claim released before any edge holds its issue back.
    #[should_fail]
    fn parked_with_no_edge(issue: u64) -> Released {
        Parking::begin().release(&said(), issue)
    }

    /// A claim released with nothing said on the issue.
    #[should_fail]
    fn released_before_the_reason_is_commented(issue: u64) -> Released {
        Escalation::read(false, true).release(&said(), &accounted(), issue)
    }

    /// A claim released with the issue still labelled ready: back on the frontier.
    #[should_fail]
    fn released_before_the_relabel(issue: u64) -> Released {
        let mut begun = Escalation::read(false, false);
        begun.comment();
        begun.release(&said(), &accounted(), issue)
    }

    /// Whatever GitHub showed, taken for an escalation that is complete.
    #[should_fail]
    fn released_on_whatever_was_read(issue: u64, commented: bool, relabelled: bool) -> Released {
        Escalation::read(commented, relabelled).release(&said(), &accounted(), issue)
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
    use crate::blocked::{Blocked, CutWait, Standing, blocked_route};
    use crate::config::Settings;
    use crate::left::{Held, Left};
    use crate::stopped::Progress;

    fn steps(
        begun: Escalation,
        spare: bool,
        fail_at: Option<EscalateStep>,
    ) -> (Vec<EscalateStep>, bool) {
        let left = Left::of(&Held { dirty: false, unpushed: Some(0) });
        let proof = left.spare().filter(|_| spare);
        let mut did = vec![];
        let ended = escalate(7, begun, proof, |step| {
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
            (vec![Comment, Relabel, Board, Account, Release, CloseTerminal], true)
        );
        assert_eq!(
            steps(Escalation::read(true, false), false, None),
            (vec![Relabel, Board, Account, Release, CloseTerminal], true)
        );
        assert_eq!(
            steps(Escalation::read(true, true), true, None),
            (vec![Board, Account, Release, RemoveWorktree], true)
        );
        // a relabel seen with no comment of this claim's: the reason is still owed
        assert_eq!(
            steps(Escalation::read(false, true), false, None),
            (vec![Comment, Board, Account, Release, CloseTerminal], true)
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
            (vec![Comment, Relabel, Board, Account], false)
        );
        assert_eq!(
            steps(Escalation::read(false, false), true, Some(Account)),
            (vec![Comment, Relabel, Board], false),
            "the claim is held until the account is left"
        );
    }

    #[test]
    fn an_escalation_is_recognised_by_the_claim_it_ends() {
        let comment = escalation_comment("abc123", None, "  stuck on the schema \n");
        assert!(comment.starts_with("<!--convoy:escalation claim=abc123-->\n"));
        assert!(comment.contains("\n\nstuck on the schema\n"));
        assert!(escalation_begun([comment.as_str()], "abc123"));
        assert!(!escalation_begun([comment.as_str()], "def456"), "an earlier claim's");
        assert!(!escalation_begun(["no marker"], "abc123"));
        // In a bound repo it says which workflow the issue left.
        let bound = escalation_comment("abc123", Some("feature"), "stuck");
        assert!(bound.starts_with("<!--convoy:escalation claim=abc123 flow=feature-->\n"));
        assert!(escalation_begun([bound.as_str()], "abc123"));
        assert_eq!(escalated_from([bound.as_str()]).as_deref(), Some("feature"));
        assert_eq!(escalated_from([bound.as_str(), comment.as_str()]), None, "the latest says");
    }

    #[test]
    fn a_swap_back_puts_the_flow_label_on_and_takes_the_escalate_label_off() {
        let labels = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let back = Relabel::back(&labels(&["bug", "stuck"]), "flow:feature", "stuck");
        assert_eq!(back.add.as_deref(), Some("flow:feature"));
        assert_eq!(back.remove, ["stuck"]);
        assert!(Relabel::back(&labels(&["flow:feature"]), "flow:feature", "stuck").done());
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
        let team = crate::team::tests::one(crate::team::tests::FEATURE);
        let carried = labels(&["flow:feature", "bug", "flow:chore", "ready-for-agent"]);
        let swap = Relabel::of(&carried, &config, Some(&team));
        assert_eq!(swap.add.as_deref(), Some("ready-for-human"));
        assert_eq!(
            swap.remove,
            ["flow:feature", "flow:chore"],
            "the ready label means nothing here"
        );
        assert!(!swap.done());
        assert!(Relabel::of(&labels(&["bug", "ready-for-human"]), &config, Some(&team)).done());
        // A workflow that names the ready label its flow label takes it off.
        let named = format!("label: ready-for-agent\n{}", crate::team::tests::FEATURE);
        let team = crate::team::tests::one(&named);
        let swap = Relabel::of(&carried, &config, Some(&team));
        assert_eq!(swap.remove, ["flow:feature", "flow:chore", "ready-for-agent"]);
        let back = Relabel::back(
            &labels(&["bug", "ready-for-human"]),
            "ready-for-agent",
            "ready-for-human",
        );
        assert_eq!(
            (back.add.as_deref(), back.remove),
            (Some("ready-for-agent"), vec!["ready-for-human".to_string()])
        );
    }

    #[test]
    fn handing_on_releases_after_the_board_and_keeps_a_worktree_another_stage_needs() {
        use HandOnStep::*;
        let run = |end: End, fail_at: Option<HandOnStep>| {
            let mut steps = vec![];
            let done = hand_on(7, end, |step| {
                steps.push(step);
                if Some(step) == fail_at { Err("cut") } else { Ok(()) }
            });
            (done.map(|released| released.issue), steps)
        };
        assert_eq!(run(End::HandedOn, None), (Ok(7), vec![Board, Release, CloseTerminal]));
        assert_eq!(
            run(End::Ended, None),
            (Ok(7), vec![Board, Account, Release, RemoveWorktree]),
            "a workflow that ended leaves the account"
        );
        assert_eq!(
            run(End::Closed, None),
            (Ok(7), vec![Board, Account, Release, CloseTerminal]),
            "a consult's close leaves it too"
        );
        assert_eq!(
            run(End::HandedOn, Some(Board)),
            (Err("cut"), vec![Board]),
            "the claim is still held"
        );
        assert_eq!(
            run(End::Ended, Some(Account)),
            (Err("cut"), vec![Board, Account]),
            "the claim is held until the account is left"
        );
    }

    #[test]
    fn every_transition_leaves_the_account_once_exactly_where_its_end_does() {
        let accounts = |steps: &[bool]| steps.iter().filter(|step| **step).count();
        let (escalated, _) = steps(Escalation::read(false, false), false, None);
        assert_eq!(
            accounts(&escalated.iter().map(|s| *s == EscalateStep::Account).collect::<Vec<_>>()),
            1
        );
        for (end, owed) in [(End::HandedOn, 0), (End::Closed, 1), (End::Ended, 1)] {
            let mut did = vec![];
            hand_on::<(), _>(7, end, |step| {
                did.push(step == HandOnStep::Account);
                Ok(())
            })
            .unwrap();
            assert_eq!(accounts(&did), owed, "{end:?}");
        }
        let waiting = [(41, Standing::Waiting)];
        let Blocked::Park(parkable) = blocked_route(&waiting, &CutWait::No) else { panic!() };
        let mut parked = 0;
        park::<(), _>(7, &parkable, None, |_| {
            parked += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(parked, 4, "an edge, the board, the release, the terminal: no account");
    }

    #[test]
    fn close_releases_after_the_issue_is_closed() {
        let clean = Progress {
            commits_ahead: Some(0),
            trees_equal: Some(true),
            dirty: false,
            last_commit: None,
            touched: None,
        };
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
                CloseStep::Account,
                CloseStep::Release,
                CloseStep::RemoveWorktree
            ],
            "the board's last word, the account, then the claim"
        );
        let cut =
            close(7, &proof, |step| if step == CloseStep::Release { Err("no") } else { Ok(()) });
        assert_eq!(cut, Err("no"));
    }

    #[test]
    fn forgetting_releases_first_writes_no_board_and_removes_only_a_spare_worktree() {
        use ForgetStep::*;
        let run = |spare: Option<&Spare>, fail_at: Option<ForgetStep>| {
            let mut steps = vec![];
            let done = forget(7, spare, |step| {
                steps.push(step);
                if Some(step) == fail_at { Err("cut") } else { Ok(()) }
            });
            (done.map(|released| released.issue), steps)
        };
        assert_eq!(run(None, None), (Ok(7), vec![Release, CloseTerminal]));
        let left = Left::of(&Held { dirty: false, unpushed: Some(0) });
        assert_eq!(run(left.spare(), None), (Ok(7), vec![Release, RemoveWorktree]));
        assert_eq!(run(None, Some(Release)), (Err("cut"), vec![Release]), "still held");
    }

    #[test]
    fn park_writes_every_edge_before_it_releases() {
        let waiting = [(41, Standing::Waiting), (42, Standing::Closed), (43, Standing::Waiting)];
        let Blocked::Park(parkable) = blocked_route(&waiting, &CutWait::No) else { panic!() };
        let run = |spare: Option<&Spare>, fail_at: Option<ParkStep>| {
            let mut steps = vec![];
            let done = park(7, &parkable, spare, |step| {
                steps.push(step);
                if Some(step) == fail_at { Err("cut") } else { Ok(()) }
            });
            (done.map(|released| released.issue), steps)
        };
        let edges = [ParkStep::Edge { blocker: 41 }, ParkStep::Edge { blocker: 43 }];
        // a worktree holding what the remote does not is kept
        let (done, steps) = run(None, None);
        assert_eq!(done, Ok(7));
        assert_eq!(
            steps,
            [edges[0], edges[1], ParkStep::Board, ParkStep::Release, ParkStep::CloseTerminal],
            "its worker's terminal is closed"
        );
        let left = Left::of(&Held { dirty: false, unpushed: Some(0) });
        let (_, steps) = run(left.spare(), None);
        assert_eq!(
            steps,
            [edges[0], edges[1], ParkStep::Board, ParkStep::Release, ParkStep::RemoveWorktree]
        );
        // an edge that failed: the claim is still held
        let (done, steps) = run(None, Some(edges[1]));
        assert_eq!((done, steps), (Err("cut"), edges.to_vec()));
    }

    #[test]
    fn park_on_the_cut_labels_the_issue_before_it_releases() {
        let waiting = [(41, Standing::Waiting)];
        let Blocked::Park(parkable) = blocked_route(&waiting, &CutWait::Coming) else { panic!() };
        let mut steps = vec![];
        park::<(), _>(7, &parkable, None, |step| {
            steps.push(step);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            steps,
            [
                ParkStep::Cut,
                ParkStep::Edge { blocker: 41 },
                ParkStep::Board,
                ParkStep::Release,
                ParkStep::CloseTerminal
            ]
        );
        let Blocked::Park(alone) = blocked_route(&[], &CutWait::Coming) else { panic!() };
        let mut steps = vec![];
        let failed = park(7, &alone, None, |step| {
            steps.push(step);
            if step == ParkStep::Cut { Err("gh down") } else { Ok(()) }
        });
        assert_eq!((failed.map(|r| r.issue), steps), (Err("gh down"), vec![ParkStep::Cut]));
    }
}

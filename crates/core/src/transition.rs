//! The transitions that end a claim other than a landing: `escalate` and
//! `close`. Each runs in one order, releases the claim as its last write to
//! the remote, and hands back a `Released` only that release produces — so
//! one cannot end half way and be taken for done.
//!
//! The order is decided here; the effect layer hands in the one function that
//! carries a step out.

use flux_rs::attrs::*;

use crate::config::Config;
use crate::record;
use crate::stopped::Workless;

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
    #[spec(fn(Self{e: e.commented && e.relabelled}, issue: u64) -> Released)]
    fn release(self, issue: u64) -> Released {
        Released { issue }
    }
}

/// One step of an escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalateStep {
    /// Comment the reason on the issue.
    Comment,
    /// Swap `ready_label` for `escalate_label`.
    Relabel,
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
    carry_out(EscalateStep::Release)?;
    let released = begun.release(issue);
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
    labels.contains(&config.escalate_label) && !labels.contains(&config.ready_label)
}

/// One step of closing an issue that needed no change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseStep {
    CloseIssue,
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
    carry_out(CloseStep::Release)?;
    let released = Released { issue };
    carry_out(CloseStep::RemoveWorktree)?;
    Ok(released)
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A claim released with nothing said on the issue.
    #[should_fail]
    fn released_before_the_reason_is_commented(issue: u64) -> Released {
        Escalation::read(false, true).release(issue)
    }

    /// A claim released with the issue still labelled ready: back on the frontier.
    #[should_fail]
    fn released_before_the_relabel(issue: u64) -> Released {
        let mut begun = Escalation::read(false, false);
        begun.comment();
        begun.release(issue)
    }

    /// Whatever GitHub showed, taken for an escalation that is complete.
    #[should_fail]
    fn released_on_whatever_was_read(issue: u64, commented: bool, relabelled: bool) -> Released {
        Escalation::read(commented, relabelled).release(issue)
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
            (vec![Comment, Relabel, Release], true)
        );
        assert_eq!(
            steps(Escalation::read(true, false), false, None),
            (vec![Relabel, Release], true)
        );
        assert_eq!(
            steps(Escalation::read(true, true), true, None),
            (vec![Release, RemoveWorktree], true)
        );
        // a relabel seen with no comment of this claim's: the reason is still owed
        assert_eq!(
            steps(Escalation::read(false, true), false, None),
            (vec![Comment, Release], true)
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
            (vec![Comment, Relabel], false)
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
        assert_eq!(did, [CloseStep::CloseIssue, CloseStep::Release, CloseStep::RemoveWorktree]);
        let cut =
            close(7, &proof, |step| if step == CloseStep::Release { Err("no") } else { Ok(()) });
        assert_eq!(cut, Err("no"));
    }
}

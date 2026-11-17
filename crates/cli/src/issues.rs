//! The fleet's issues: the one seam every read and write of a tracker's
//! issues crosses (GLOSSARY: Tracker, docs/adr/0091) — the
//! open ones, one issue, its comments, blockers and labels, one opened,
//! commented on or closed. What is the repo's — PRs, branches, the base
//! branch — stays on `GitHub`.
//!
//! An adapter answers each operation of [`Issues`] in domain types. Which
//! adapter a checkout's fleet holds is chosen in [`of`] alone, from the
//! tracker its host names (`checkout_list::tracker_of`): a GitHub repo's
//! (`crate::gh::issues::GitHubIssues`), or the host's local tracker
//! (`local::LocalIssues`). What only GitHub has — database and node ids,
//! label colours, visibility, quota — stays in its adapter.
//!
//! The operations are the reads and writes the fleet makes, not GitHub's
//! calls: each read is shaped so one paged query can answer it (an open
//! issue comes with its comments and blockers), which is what keeps a GitHub
//! tracker's reads within its quota (the `shared_reading` module).

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;

use afkconvoy_core::plan::{Issue, PullRequest};
use afkconvoy_core::tracker::Tracker;
use anyhow::{Result, bail};
use serde::Deserialize;

use crate::gh::GitHub;

pub mod local;

/// The issues of the tracker a fleet works, open or closed. Every method
/// names an issue by its number in the tracker; an issue that is no more —
/// deleted, transferred, never issued — is an error [`Gone`] says.
pub trait Issues: Send + Sync {
    /// Which tracker these are the issues of.
    fn tracker(&self) -> &Tracker;

    /// Every open issue of the fleet, each with all its comments and
    /// blockers; with `day_ending` set, the patrol landings of the day that
    /// ends then too, where the same read can say all of them.
    fn open_issues(&self, day_ending: Option<u64>) -> Result<OpenRead>;
    /// One issue, open or closed.
    fn issue(&self, number: u64) -> Result<IssueRead>;
    /// Every comment on one issue, oldest first.
    fn comments(&self, number: u64) -> Result<Vec<Comment>>;
    /// The issues one is recorded as blocked by, open or closed.
    fn blocked_by(&self, number: u64) -> Result<Vec<BlockedBy>>;
    /// The open ones among `edges`, lowest number first, deduplicated; the
    /// closed ones are seen closed (`seen_closed`).
    fn open_of(&self, edges: Vec<BlockedBy>) -> Vec<u64>;
    /// The fleet's open issues that carry `label`, lowest number first.
    fn open_with(&self, label: &str) -> Result<Vec<u64>>;
    /// `open_with`, read as the processes of the host waiting on it share
    /// one read an interval where a read costs them: a `land` waiting for
    /// its turn asks this of the fleet lock again and again.
    fn open_with_shared(&self, label: &str) -> Result<Vec<u64>> {
        self.open_with(label)
    }
    /// The fleet's closed issues that carry `label`, each with when it
    /// closed in epoch seconds — but for one with no time said.
    fn closed_with(&self, label: &str) -> Result<Vec<(u64, u64)>>;
    /// The fleet's issues that carry `label`, open or closed, each with when
    /// it closed — `None`: it is open, `Some(None)`: no time is said.
    fn carrying(&self, label: &str) -> Result<Vec<(u64, Option<Option<u64>>)>>;
    /// The fleet's issues updated since `since`, open or closed, each with
    /// all its comments.
    fn watching_issues(&self, since: u64) -> Result<Vec<WatchIssue>>;
    /// `(patrol, closed at)` of each patrol's issue completed in the day
    /// ending at `now`.
    fn patrol_landings(&self, now: u64) -> Result<Vec<(String, u64)>>;
    /// The work account on each of the fleet's issues, open or closed. A
    /// failed read of one issue's comments is that issue's row.
    fn accounts(&self) -> Result<Vec<afkconvoy_core::accounts::Issue>>;
    /// Every issue of the tracker, of whatever repo, with all its comments.
    fn account_issues(&self) -> Result<Vec<AccountIssue>>;
    /// Every label the tracker has, by name.
    fn labels(&self) -> Result<Vec<String>>;
    /// The issues this view has seen closed, whatever read or write saw it,
    /// shared by every view of one `GitHub` and its clones. These only route
    /// wakes; every woken tick reads for itself.
    fn seen_closed(&self) -> Vec<u64>;

    /// Opens an issue of the fleet carrying `labels` → its number.
    fn open_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64>;
    /// Posts one comment on an issue.
    fn comment(&self, number: u64, body: &str) -> Result<()>;
    /// Replaces the body of one comment on an issue, by its id.
    fn edit_comment(&self, id: u64, body: &str) -> Result<()>;
    /// Replaces the title of one issue, its body, or both: what is `None`
    /// is left as it is.
    fn edit_issue(&self, number: u64, title: Option<&str>, body: Option<&str>) -> Result<()>;
    /// Closes one issue as completed: one closed already is closed.
    fn close_issue(&self, number: u64) -> Result<()>;
    /// Opens one closed issue again: one open is left open.
    fn reopen_issue(&self, number: u64) -> Result<()>;
    /// Records issue `number` as blocked by issue `blocker`.
    fn add_blocker(&self, number: u64, blocker: u64) -> Result<()>;
    /// Records issue `number` as no longer blocked by issue `blocker`: an
    /// edge that is not there is no matter.
    fn remove_blocker(&self, number: u64, blocker: u64) -> Result<()>;
    /// Creates the label `name`, which the tracker has none of, saying what
    /// it is as `says` where the tracker shows that.
    fn create_label(&self, name: &str, says: &str) -> Result<()>;
    /// Puts one label on an issue; the tracker creates one it lacks.
    fn add_label(&self, number: u64, label: &str) -> Result<()>;
    /// Takes one label off an issue that carries it.
    fn remove_label(&self, number: u64, label: &str) -> Result<()>;

    /// Makes issue `number`, as `read` read it, the repo's in its tracker
    /// before a command hands it to the fleet (`Tracker::to_take`): puts the
    /// repo label on it, in a tracker that is another repo, where it carries
    /// none. An error of one that is another repo's, which is left as it is.
    fn take(&self, number: u64, read: &IssueRead) -> Result<()> {
        let issue = self.tracker().issue(number);
        match self.tracker().to_take(&read.labels, read.of.as_deref()) {
            Ok(Some(label)) => self.add_label(number, &label),
            Ok(None) => Ok(()),
            Err(why) => bail!("issue {issue} {why}"),
        }
    }

    /// The open issues among those `number` is blocked by.
    fn open_blockers(&self, number: u64) -> Result<Vec<u64>> {
        Ok(self.open_of(self.blocked_by(number)?))
    }

    /// Opens a failure report and leaves its reason in a comment, so every
    /// wait can link directly to the explanation. If commenting fails, the
    /// report's body still explains the wait on the next rebuild.
    fn open_report(&self, title: &str, body: &str, reason: &str, labels: &[&str]) -> Result<u64> {
        let issue = self.open_issue(title, body, labels)?;
        let marker = body.lines().next().unwrap_or("");
        self.comment(issue, &format!("{marker}\n{reason}"))?;
        Ok(issue)
    }
}

/// The issues of the tracker `gh` names, through the adapter of that kind of
/// tracker: the one place an adapter is chosen.
pub fn of(gh: &GitHub) -> Arc<dyn Issues> {
    match &gh.tracker {
        Tracker::Own | Tracker::Other { .. } => {
            Arc::new(crate::gh::issues::GitHubIssues::new(gh.clone()))
        }
        Tracker::Local { .. } => Arc::new(local::LocalIssues::new(gh.clone())),
    }
}

/// The body of the one work account among an issue's `comments`, none where
/// it has none; an error where it has several. A damaged opening marker
/// counts, so that no account is silently lost.
pub(crate) fn work_account(comments: Vec<Comment>) -> Result<Option<String>, String> {
    let mut accounts = comments.into_iter().filter(|comment| {
        comment.body.contains("<!--convoy:account ")
            || comment.body.contains("<!--convoy:account-->")
    });
    let body = accounts.next().map(|comment| comment.body);
    if accounts.next().is_some() {
        return Err("issue has several work accounts".into());
    }
    Ok(body)
}

/// One issue, open or closed, as `Issues::issue` reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct IssueRead {
    pub title: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    /// The number is a pull request's, not an issue's: a tracker whose
    /// issues and PRs share numbers says so.
    pub pull_request: bool,
    /// The login of whoever closed it, where the tracker says.
    #[serde(default)]
    pub closed_by: Option<String>,
    /// What it says, where it says anything.
    #[serde(default)]
    pub body: Option<String>,
    /// The repo it is the work of, where the tracker keeps that as a field
    /// of the issue — the local tracker — not as a repo label.
    #[serde(default)]
    pub of: Option<String>,
}

impl IssueRead {
    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }
}

/// One issue another is recorded as blocked by — a dependency edge.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockedBy {
    pub number: u64,
    pub state: String,
    /// The tracker it is in, `owner/name`, where the tracker says.
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// The repo it is the work of, where the tracker keeps that as a field
    /// of the issue — the local tracker — not as a repo label.
    #[serde(default)]
    pub of: Option<String>,
}

/// The one answer to which blockers are open, for both paged rebuilds and
/// the fresh read before a claim. Trackers spell state in either case.
pub(crate) fn open_blockers(edges: Vec<BlockedBy>) -> Vec<u64> {
    let mut open: Vec<u64> = edges
        .into_iter()
        .filter(|edge| edge.state.eq_ignore_ascii_case("open"))
        .map(|edge| edge.number)
        .collect();
    open.sort_unstable();
    open.dedup();
    open
}

/// One comment on an issue or a pull request.
#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub body: String,
    pub url: String,
    /// When it was written, in epoch seconds.
    #[serde(default)]
    pub at: Option<u64>,
}

/// An issue updated since a watching agent's last look.
#[derive(Debug, Deserialize)]
pub struct WatchIssue {
    pub number: u64,
    pub title: String,
    pub closed: Option<u64>,
    pub labels: Vec<String>,
    pub more: bool,
    pub comments: Vec<Comment>,
}

/// A tracker issue and all its comments for account conversion.
#[derive(Debug, Deserialize)]
pub struct AccountIssue {
    pub number: u64,
    pub url: String,
    pub closed: bool,
    #[serde(default)]
    pub landed: bool,
    pub labels: Vec<String>,
    pub more: bool,
    pub comments: Vec<Comment>,
}

/// The latest verdict belongs to this claim only when its comment was written
/// at or after the claim began. Missing timestamps cannot establish ownership.
pub(crate) fn verdict_since(
    comments: &[Comment],
    since: Option<u64>,
) -> Option<afkconvoy_core::verdict::Verdict> {
    let verdict =
        afkconvoy_core::verdict::latest_verdict(comments.iter().map(|c| c.body.as_str()))?;
    comments.get(verdict.comment)?.at.zip(since).filter(|(at, since)| at >= since)?;
    Some(verdict)
}

/// Every open issue, with the comments on each, as one read saw them.
#[derive(Debug, Default)]
pub struct OpenIssues {
    pub rows: Vec<Issue>,
    /// Native edges retain repo identity: issue numbers alone are not unique.
    pub blockers: HashMap<u64, Vec<BlockedBy>>,
    /// issue → its comments, oldest first.
    pub(crate) comments: HashMap<u64, Vec<Comment>>,
    /// Failure reports written before reason comments were added still
    /// carry their reason in the issue body.
    pub(crate) reports: HashMap<u64, Comment>,
    pub(crate) edited: HashMap<u64, u64>,
}

impl OpenIssues {
    /// Explains the shared waiting list from the same paged read that made
    /// it. A report's body is a fallback behind its reason comments.
    pub fn explain(&self, waiting: &mut [afkconvoy_core::waiting::Waiting]) {
        use afkconvoy_core::waiting::{Said, Waits};
        for row in waiting {
            let report = match row.waits {
                Waits::PatrolFailed { .. } | Waits::CutFailed => self.reports.get(&row.issue),
                _ => None,
            };
            let sources: Vec<Said<'_>> = report
                .into_iter()
                .chain(self.comments(row.issue))
                .map(|comment| Said { body: &comment.body, url: &comment.url, at: comment.at })
                .collect();
            row.explain(&sources);
        }
    }

    /// Every comment on the open issue `number`, oldest first; none of an
    /// issue that is not among them.
    pub fn comments(&self, number: u64) -> &[Comment] {
        self.comments.get(&number).map_or(&[], Vec::as_slice)
    }

    /// When the tracker last recorded an edit of the issue, where it records
    /// one.
    pub fn body_edited(&self, number: u64) -> Option<u64> {
        self.edited.get(&number).copied()
    }

    /// The body and link from the same read as the comments.
    pub fn body(&self, number: u64) -> Option<&Comment> {
        self.reports.get(&number)
    }
}

/// The open issues, and the patrol landings of a day read with them.
pub struct OpenRead {
    pub issues: OpenIssues,
    /// `None`: this read did not ask, or could not say all of the day.
    pub landings: Option<Vec<(String, u64)>>,
}

/// What is open: the tracker's issues and the repo's PRs, as a rebuild
/// reads them.
#[derive(Debug, Default)]
pub struct Open {
    pub issues: OpenIssues,
    pub prs: Vec<PullRequest>,
}

/// What is open, and the patrol landings read in that same query.
pub(crate) struct Opened {
    pub open: Open,
    /// `None`: this read did not ask, or the closed page may be short of the day.
    pub landings: Option<Vec<(String, u64)>>,
}

/// What is open — the issues of `issues` and the PRs of `gh`, asked for side
/// by side — and when `day_ending` is set, the patrol landings of that day.
pub(crate) fn open_with_landings(
    gh: &GitHub,
    issues: &dyn Issues,
    day_ending: Option<u64>,
) -> Result<Opened> {
    let (read, prs) = thread::scope(|scope| {
        let prs = scope.spawn(|| gh.open_prs());
        (issues.open_issues(day_ending), beside(prs))
    });
    let read = read?;
    Ok(Opened { open: Open { issues: read.issues, prs: prs? }, landings: read.landings })
}

/// What is open: the issues and the PRs, asked for side by side.
pub fn open(gh: &GitHub, issues: &dyn Issues) -> Result<Open> {
    Ok(open_with_landings(gh, issues, None)?.open)
}

/// What a read made beside another came to: a panic in it is this thread's.
pub fn beside<T>(read: thread::ScopedJoinHandle<'_, T>) -> T {
    read.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// An issue the tracker has no more: deleted, transferred to another repo,
/// or never issued. Like a closed one, it is never worked again, and it is
/// nothing the rest of the fleet waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gone {
    pub issue: u64,
    pub how: Went,
}

/// How the tracker says an issue is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Went {
    /// 410: it was deleted.
    Deleted,
    /// It was moved: a read of it follows the redirect into another repo,
    /// `to` — `owner/repo#n` — where the tracker says.
    Transferred { to: Option<String> },
    /// 404, and nothing moved: there is no such issue.
    Missing,
}

impl Gone {
    /// What happened to it, as a tick's line says why it let go of it:
    /// `released #231 (the issue was deleted)`.
    pub fn said(&self) -> String {
        match &self.how {
            Went::Deleted => "the issue was deleted".to_string(),
            Went::Transferred { to: Some(to) } => format!("the issue was transferred to {to}"),
            Went::Transferred { to: None } => {
                "the issue was transferred to another repo".to_string()
            }
            Went::Missing => "there is no such issue".to_string(),
        }
    }

    /// The word a dispatch's `gone` answers with.
    pub fn word(&self) -> &'static str {
        match self.how {
            Went::Deleted => "deleted",
            Went::Transferred { .. } => "transferred",
            Went::Missing => "missing",
        }
    }
}

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let issue = self.issue;
        match &self.how {
            Went::Deleted => write!(f, "issue #{issue} was deleted (HTTP 410)"),
            Went::Transferred { to: Some(to) } => {
                write!(f, "issue #{issue} was transferred to {to}")
            }
            Went::Transferred { to: None } => {
                write!(f, "issue #{issue} was transferred to another repo")
            }
            Went::Missing => write!(f, "no such issue: #{issue} (HTTP 404)"),
        }
    }
}

impl std::error::Error for Gone {}

/// The issue `error` says is gone, when that is why it was raised, whatever
/// was said around it since.
pub fn gone(error: &anyhow::Error) -> Option<&Gone> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

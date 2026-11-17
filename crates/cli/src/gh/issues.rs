//! The GitHub adapter of [`Issues`]: a tracker that is a GitHub repo's
//! issues, read and written through `gh` in the checkout (`GitHub`). In a
//! tracker that is not the checkout's own repo, the fleet's issues are those
//! that carry its repo label: the open ones are read by it, and one opened
//! carries it. GitHub's database ids, label colours and quota stay here.

use std::collections::BTreeSet;

use afkconvoy_core::plan::Issue;
use afkconvoy_core::tracker::Tracker;
use anyhow::{Context, Result};
use serde::Deserialize;

use super::{
    ACCOUNTS_JQ, ACCOUNTS_QUERY, BLOCKED_BY_JQ, CARRYING_JQ, CLOSED_JQ, GitHub, LABELS_JQ,
    LANDED_ISSUES_JQ, LANDED_ISSUES_QUERY, LANDED_PAGE, OPEN_AND_LANDED_JQ, OPEN_AND_LANDED_QUERY,
    OPEN_ISSUES_JQ, OPEN_ISSUES_QUERY, OPEN_LABELLED_JQ, REWRITE_ACCOUNTS_JQ,
    REWRITE_ACCOUNTS_QUERY, WATCH_ISSUES_JQ, WATCH_ISSUES_QUERY, carrying_labels, endpoint, path,
    variables,
};
use crate::issues::{
    AccountIssue, BlockedBy, Comment, IssueRead, Issues, OpenIssues, OpenRead, WatchIssue,
};

/// The issues of the GitHub tracker `gh` names.
pub(crate) struct GitHubIssues {
    gh: GitHub,
}

/// One open issue, as the issues query returns it.
#[derive(Debug, Deserialize)]
struct OpenRow {
    #[serde(default)]
    edited: Option<u64>,
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    at: Option<u64>,
    labels: Vec<String>,
    blockers: Vec<BlockedBy>,
    blockers_more: bool,
    more: bool,
    comments: Vec<Comment>,
}

/// One completed issue, as a landings query returns it.
#[derive(Debug, Deserialize)]
struct LandingRow {
    number: u64,
    at: Option<u64>,
    more: bool,
    comments: Vec<Comment>,
}

/// One line of the open-and-landed query: an open issue, a landing, or how
/// many closed issues the page held before the completed filter.
#[derive(Debug, Deserialize)]
#[serde(tag = "part")]
enum IssuePart {
    #[serde(rename = "open")]
    Open(OpenRow),
    #[serde(rename = "landed")]
    Landed(LandingRow),
    #[serde(rename = "landed_count")]
    Count { n: usize },
}

impl GitHubIssues {
    pub(crate) fn new(gh: GitHub) -> GitHubIssues {
        GitHubIssues { gh }
    }

    /// Closes one issue as not planned: its work goes on elsewhere — in a
    /// host's local tracker, where it was moved (docs/adr/0091). One closed
    /// already is closed.
    pub(crate) fn close_not_planned(&self, number: u64) -> Result<()> {
        let endpoint = endpoint(&self.gh.tracker, &format!("issues/{number}"));
        let closed = self.gh.gh(&[
            "api",
            "--method",
            "PATCH",
            &endpoint,
            "-f",
            "state=closed",
            "-f",
            "state_reason=not_planned",
            "--silent",
        ]);
        if let Err(error) = closed
            && !self.issue(number).is_ok_and(|issue| issue.is_closed())
        {
            return Err(error);
        }
        self.gh.saw_closed(number);
        Ok(())
    }

    fn take_open(&self, open: &mut OpenIssues, row: OpenRow) -> Result<()> {
        // Another repo's issue in the tracker is none of the fleet's.
        if !self.gh.tracker.holds(&row.labels) {
            return Ok(());
        }
        // Never infer an unblocked issue from the embedded page alone
        // when GitHub says another page follows. A failed full read
        // fails the rebuild instead of dispatching through a blocker.
        let blockers = if row.blockers_more { self.blocked_by(row.number)? } else { row.blockers };
        let comments = if row.more { self.comments(row.number)? } else { row.comments };
        open.comments.insert(row.number, comments);
        if let Some(edited) = row.edited {
            open.edited.insert(row.number, edited);
        }
        open.blockers.insert(row.number, blockers.clone());
        open.reports
            .insert(row.number, Comment { id: 0, body: row.body, url: row.url, at: row.at });
        open.rows.push(Issue {
            number: row.number,
            title: row.title,
            labels: row.labels,
            open_blockers: self.open_of(blockers),
        });
        Ok(())
    }

    /// Every open issue with its comments — all of them, page after page:
    /// the number of calls does not grow with the issues that have comments.
    /// An issue whose comments or blockers go past the embedded page is
    /// asked about again, for all of that connection.
    fn open_only(&self) -> Result<OpenIssues> {
        let out = self.gh.query(OPEN_ISSUES_QUERY, OPEN_ISSUES_JQ)?;
        let mut open = OpenIssues::default();
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: OpenRow =
                serde_json::from_str(line).context("gh api graphql: unexpected answer")?;
            self.take_open(&mut open, row)?;
        }
        Ok(open)
    }

    /// The open issues, and the patrol landings of the day ending at `now`,
    /// from one issues query. `None` for the landings when the closed page
    /// may be short of the day: the caller then reads them on their own pages.
    fn open_and_landed(&self, now: u64) -> Result<OpenRead> {
        let since = now.saturating_sub(afkconvoy_core::patrol::DAY);
        let since = format!("since={}", crate::log::stamp(since));
        let out = self.gh.query_with(OPEN_AND_LANDED_QUERY, OPEN_AND_LANDED_JQ, &[since])?;
        let mut open = OpenIssues::default();
        let mut landed_rows = vec![];
        let mut truncated = false;
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let part: IssuePart =
                serde_json::from_str(line).context("gh api graphql: unexpected answer")?;
            match part {
                IssuePart::Open(row) => self.take_open(&mut open, row)?,
                IssuePart::Landed(row) => landed_rows.push(row),
                IssuePart::Count { n } if n >= LANDED_PAGE => truncated = true,
                IssuePart::Count { .. } => {}
            }
        }
        if truncated {
            return Ok(OpenRead { issues: open, landings: None });
        }
        let mut seen = BTreeSet::new();
        let mut landed = vec![];
        for row in landed_rows {
            if !seen.insert(row.number) {
                continue;
            }
            landed.extend(self.take_landing(&row, now)?);
        }
        Ok(OpenRead { issues: open, landings: Some(landed) })
    }

    /// `(patrol, closed at)` when `row` is a patrol's completed issue inside
    /// the day ending at `now`. Comments past the embedded page are read.
    fn take_landing(&self, row: &LandingRow, now: u64) -> Result<Option<(String, u64)>> {
        let Some(at) = row.at else { return Ok(None) };
        if now.saturating_sub(at) >= afkconvoy_core::patrol::DAY {
            return Ok(None);
        }
        let rest = if row.more { Some(self.comments(row.number)?) } else { None };
        let comments = rest.as_deref().unwrap_or(row.comments.as_slice());
        let patrol = afkconvoy_core::intake::opened_by(comments.iter().map(|c| c.body.as_str()));
        Ok(patrol.map(|patrol| (patrol, at)))
    }

    fn open_with_read(
        &self,
        label: &str,
        read: impl FnOnce(&[&str]) -> Result<String>,
    ) -> Result<Vec<u64>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            labels: Vec<String>,
        }
        let tracker = &self.gh.tracker;
        let labels = carrying_labels(tracker, label);
        let endpoint =
            endpoint(tracker, &format!("issues?state=open&labels={labels}&per_page=100"));
        let out = read(&["api", "--paginate", &endpoint, "--jq", OPEN_LABELLED_JQ])?;
        let mut numbers = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            if tracker.holds(&row.labels) && row.labels.iter().any(|held| held == label) {
                numbers.push(row.number);
            }
        }
        numbers.sort_unstable();
        Ok(numbers)
    }
}

impl Issues for GitHubIssues {
    fn tracker(&self) -> &Tracker {
        &self.gh.tracker
    }

    fn open_issues(&self, day_ending: Option<u64>) -> Result<OpenRead> {
        match day_ending {
            None => self.open_only().map(|issues| OpenRead { issues, landings: None }),
            Some(now) => self.open_and_landed(now),
        }
    }

    /// `Gone` when GitHub says it was deleted or transferred, or has no such
    /// issue.
    fn issue(&self, number: u64) -> Result<IssueRead> {
        Ok(self.gh.read_issue(number)?.read)
    }

    /// `Gone` of one GitHub says was deleted or transferred.
    fn comments(&self, number: u64) -> Result<Vec<Comment>> {
        self.gh.comments_in(path(&self.gh.tracker), number)
    }

    /// `Gone` when `number` was deleted or transferred.
    fn blocked_by(&self, number: u64) -> Result<Vec<BlockedBy>> {
        let endpoint = endpoint(
            &self.gh.tracker,
            &format!("issues/{number}/dependencies/blocked_by?per_page=100"),
        );
        let read = self.gh.read(&["api", "--paginate", &endpoint, "--jq", BLOCKED_BY_JQ]);
        let out = self.gh.about_issue(number, read)?;
        out.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("gh api blocked_by: unexpected answer"))
            .collect()
    }

    fn open_of(&self, edges: Vec<BlockedBy>) -> Vec<u64> {
        for edge in &edges {
            if edge.state.eq_ignore_ascii_case("closed") {
                self.gh.saw_closed(edge.number);
            }
        }
        crate::issues::open_blockers(edges)
    }

    /// Lock checks need only this REST listing, not a rebuild with every
    /// comment and blocker, and remain available without GraphQL.
    fn open_with(&self, label: &str) -> Result<Vec<u64>> {
        self.open_with_read(label, |args| self.gh.read(args))
    }

    /// One read an interval of the host's processes (`crate::shared_reading`).
    fn open_with_shared(&self, label: &str) -> Result<Vec<u64>> {
        let tracker = self.gh.tracker.named().map(ToString::to_string).unwrap_or_default();
        let what = format!("open {tracker} {label} {:?}", self.gh.tracker.repo_label());
        self.open_with_read(label, |args| crate::shared_reading::rest(&self.gh, &what, args))
    }

    fn closed_with(&self, label: &str) -> Result<Vec<(u64, u64)>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            at: Option<u64>,
        }
        let labels = carrying_labels(&self.gh.tracker, label);
        let endpoint = endpoint(
            &self.gh.tracker,
            &format!("issues?state=closed&labels={labels}&per_page=100"),
        );
        let out = self.gh.read(&["api", "--paginate", &endpoint, "--jq", CLOSED_JQ])?;
        let mut closed = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            closed.extend(row.at.map(|at| (row.number, at)));
        }
        Ok(closed)
    }

    fn carrying(&self, label: &str) -> Result<Vec<(u64, Option<Option<u64>>)>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            open: bool,
            closed_at: Option<u64>,
        }
        let labels = carrying_labels(&self.gh.tracker, label);
        let endpoint =
            endpoint(&self.gh.tracker, &format!("issues?state=all&labels={labels}&per_page=100"));
        let out = self.gh.read(&["api", "--paginate", &endpoint, "--jq", CARRYING_JQ])?;
        let mut carrying = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            carrying.push((row.number, (!row.open).then_some(row.closed_at)));
        }
        Ok(carrying)
    }

    /// This uses GitHub's update time, not a search index, and pages through
    /// every result without a search result cap.
    fn watching_issues(&self, since: u64) -> Result<Vec<WatchIssue>> {
        let query = format!("query={WATCH_ISSUES_QUERY}");
        let since = format!("since={}", crate::log::stamp(since));
        let variables = variables(&self.gh.tracker);
        let mut args = vec!["api", "graphql", "--paginate"];
        args.extend(variables.iter().map(String::as_str));
        args.extend(["-f", &query, "-f", &since, "--jq", WATCH_ISSUES_JQ]);
        let out = self.gh.read(&args)?;
        let mut issues = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let mut issue: WatchIssue = serde_json::from_str(line)
                .context("gh api graphql: unexpected watching issues answer")?;
            if !self.gh.tracker.holds(&issue.labels)
                || afkconvoy_core::tracker::repo_labels(&issue.labels).len() > 1
            {
                continue;
            }
            if issue.more {
                issue.comments = self.comments(issue.number)?;
            }
            issues.push(issue);
        }
        Ok(issues)
    }

    /// Read only when a configured patrol has a landing bound. Ownership
    /// follows the latest intake record, regardless of the issue's labels now.
    fn patrol_landings(&self, now: u64) -> Result<Vec<(String, u64)>> {
        let out = self.gh.query(LANDED_ISSUES_QUERY, LANDED_ISSUES_JQ)?;
        let mut landed = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: LandingRow =
                serde_json::from_str(line).context("gh api graphql: unexpected answer")?;
            landed.extend(self.take_landing(&row, now)?);
        }
        Ok(landed)
    }

    /// A failed issue-comment read is data for the report; a failed issue
    /// listing fails the command as a whole.
    fn accounts(&self) -> Result<Vec<afkconvoy_core::accounts::Issue>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            #[serde(default)]
            closed: bool,
            url: String,
            labels: Vec<String>,
            more: bool,
            comments: Vec<Comment>,
        }
        let out = self.gh.query(ACCOUNTS_QUERY, ACCOUNTS_JQ)?;
        let mut issues = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row = serde_json::from_str(line).context("gh api: invalid accounts answer")?;
            if !self.gh.tracker.holds(&row.labels)
                || afkconvoy_core::tracker::repo_labels(&row.labels).len() > 1
            {
                continue;
            }
            let comments = if row.more { self.comments(row.number) } else { Ok(row.comments) };
            let account = comments
                .map_err(|error| format!("{error:#}"))
                .and_then(crate::issues::work_account);
            issues.push(afkconvoy_core::accounts::Issue {
                number: row.number,
                closed: row.closed,
                url: row.url,
                account,
            });
        }
        Ok(issues)
    }

    fn account_issues(&self) -> Result<Vec<AccountIssue>> {
        let query = format!("query={REWRITE_ACCOUNTS_QUERY}");
        let variables = variables(&self.gh.tracker);
        let mut args = vec!["api", "graphql", "--paginate"];
        args.extend(variables.iter().take(4).map(String::as_str));
        args.extend(["-f", &query, "--jq", REWRITE_ACCOUNTS_JQ]);
        let out = self.gh.read(&args)?;
        let mut issues = Vec::new();
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let mut issue: AccountIssue =
                serde_json::from_str(line).context("gh api: invalid tracker accounts")?;
            if issue.more {
                issue.comments = self.comments(issue.number)?;
            }
            issues.push(issue);
        }
        Ok(issues)
    }

    fn labels(&self) -> Result<Vec<String>> {
        let endpoint = endpoint(&self.gh.tracker, "labels?per_page=100");
        let out = self.gh.read(&["api", "--paginate", &endpoint, "--jq", LABELS_JQ])?;
        Ok(out.lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
    }

    fn seen_closed(&self) -> Vec<u64> {
        self.gh.seen_closed()
    }

    /// In a tracker that is not the checkout's own repo it carries the repo
    /// label too: it is the work of the checkout's repo.
    fn open_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64> {
        let (title, body) = (format!("title={title}"), format!("body={body}"));
        let repo_label = self.gh.tracker.repo_label();
        let labels = labels.iter().copied().chain(repo_label.as_deref());
        let labels: Vec<String> = labels.map(|label| format!("labels[]={label}")).collect();
        let endpoint = endpoint(&self.gh.tracker, "issues");
        let mut args = vec!["api", "--method", "POST", &endpoint, "-f", &title, "-f", &body];
        for label in &labels {
            args.extend(["-f", label.as_str()]);
        }
        args.extend(["--jq", ".number"]);
        let out = self.gh.gh(&args)?;
        out.trim().parse().with_context(|| format!("gh api issues: no number in {out:?}"))
    }

    /// `Gone` of one GitHub says was deleted or transferred.
    fn comment(&self, number: u64, body: &str) -> Result<()> {
        self.gh.comment_in(path(&self.gh.tracker), number, body)
    }

    fn edit_comment(&self, id: u64, body: &str) -> Result<()> {
        self.gh.edit_comment_in(path(&self.gh.tracker), id, body)
    }

    /// A close GitHub refuses while it closes the issue itself — a merged PR
    /// that says `Closes` it — is a close: what is asked is that the issue be
    /// closed (sunfmin/control#247).
    fn close_issue(&self, number: u64) -> Result<()> {
        let endpoint = endpoint(&self.gh.tracker, &format!("issues/{number}"));
        let closed =
            self.gh.gh(&["api", "--method", "PATCH", &endpoint, "-f", "state=closed", "--silent"]);
        if let Err(error) = closed
            && !self.issue(number).is_ok_and(|issue| issue.is_closed())
        {
            return Err(error);
        }
        self.gh.saw_closed(number);
        Ok(())
    }

    /// GitHub names the blocker of an edge by its database id: the one a read
    /// of it in this view found (`GitHub::read_issue`) — an id never changes —
    /// or a read of it now.
    fn add_blocker(&self, number: u64, blocker: u64) -> Result<()> {
        let id = self.gh.seen().ids.get(&blocker).copied();
        let id = match id {
            Some(id) => id,
            None => self.gh.read_issue(blocker)?.id,
        };
        let endpoint =
            endpoint(&self.gh.tracker, &format!("issues/{number}/dependencies/blocked_by"));
        let field = format!("issue_id={id}");
        self.gh.gh(&["api", "--method", "POST", &endpoint, "-F", &field, "--silent"])?;
        Ok(())
    }

    /// Shown in the label's colour every afkconvoy label has.
    fn create_label(&self, name: &str, says: &str) -> Result<()> {
        let (name, description) = (format!("name={name}"), format!("description={says}"));
        let endpoint = endpoint(&self.gh.tracker, "labels");
        self.gh.gh(&[
            "api",
            "--method",
            "POST",
            &endpoint,
            "-f",
            &name,
            "-f",
            &description,
            "-f",
            "color=1d76db",
            "--silent",
        ])?;
        Ok(())
    }

    fn add_label(&self, number: u64, label: &str) -> Result<()> {
        let endpoint = endpoint(&self.gh.tracker, &format!("issues/{number}/labels"));
        let field = format!("labels[]={label}");
        self.gh.gh(&["api", "--method", "POST", &endpoint, "-f", &field, "--silent"])?;
        Ok(())
    }

    fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let label = crate::judge::label_segment(label);
        let endpoint = endpoint(&self.gh.tracker, &format!("issues/{number}/labels/{label}"));
        self.gh.gh(&["api", "--method", "DELETE", &endpoint, "--silent"])?;
        Ok(())
    }
}

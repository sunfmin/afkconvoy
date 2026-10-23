//! GitHub, through `gh` as a subprocess run in the checkout: `gh` works out
//! which repository that is, so nothing here names one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::layout::{Guard, Protection};
use afkconvoy_core::plan::{Issue, PullRequest};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The open issues, a page at a time, each with its labels, blockers and
/// its first `COMMENTS_WITH` comments: one call a page,
/// however many of the issues have comments to read. `gh api graphql` fills
/// `{owner}` and `{repo}` in, and `$endCursor` page after page.
pub const OPEN_ISSUES_QUERY: &str = "query($owner: String!, $repo: String!, $endCursor: String) { \
    repository(owner: $owner, name: $repo) { \
    issues(states: OPEN, first: 100, after: $endCursor, \
    orderBy: {field: CREATED_AT, direction: DESC}) { \
    pageInfo { hasNextPage endCursor } \
    nodes { number title labels(first: 100) { nodes { name } } \
    blockedBy(first: 100) { pageInfo { hasNextPage } nodes { number state } } \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } } }";
/// What is read of every open issue: `more` says its comments go on past
/// those that came with it.
pub const OPEN_ISSUES_JQ: &str = ".data.repository.issues.nodes[] | {number, title, \
    labels: [.labels.nodes[].name], blockers: [.blockedBy.nodes[] | select(. != null) | {number, state}], \
    blockers_more: .blockedBy.pageInfo.hasNextPage, \
    more: .comments.pageInfo.hasNextPage, comments: [.comments.nodes[] | \
    {id: (.fullDatabaseId | tonumber), body, url, at: (.createdAt | fromdateiso8601? // null)}]}";
/// Completed issues with closure times and intake comments, paged together so
/// reading a patrol's landings never makes one request per ordinary issue.
pub const LANDED_ISSUES_QUERY: &str = "query($owner: String!, $repo: String!, $endCursor: String) { \
    repository(owner: $owner, name: $repo) { \
    issues(states: CLOSED, first: 100, after: $endCursor, \
    orderBy: {field: UPDATED_AT, direction: DESC}) { \
    pageInfo { hasNextPage endCursor } \
    nodes { number stateReason closedAt \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } } }";
pub const LANDED_ISSUES_JQ: &str = ".data.repository.issues.nodes[] | \
    select(.stateReason == \"COMPLETED\") | \
    {number, at: (.closedAt | fromdateiso8601? // null), \
    more: .comments.pageInfo.hasNextPage, comments: [.comments.nodes[] | \
    {id: (.fullDatabaseId | tonumber), body, url, at: (.createdAt | fromdateiso8601? // null)}]}";
/// How many comments of an issue come with it (`OPEN_ISSUES_QUERY`).
pub const COMMENTS_WITH: usize = 100;
/// What is read of one issue, open or not.
pub const ISSUE_JQ: &str = "{id, title, state, state_reason, labels: [.labels[].name], \
    pull_request: (.pull_request != null), closed_by: .closed_by.login}";
/// What is read of each closed issue that carries one label.
pub const CLOSED_JQ: &str = ".[] | select(.pull_request == null) | \
    {number, at: (.closed_at | fromdateiso8601? // null)}";
/// What is read of each issue that carries one label, open or closed.
pub const CARRYING_JQ: &str = ".[] | select(.pull_request == null) | \
    {number, open: (.state == \"open\"), closed_at: (.closed_at | fromdateiso8601? // null)}";
/// What is read of each comment on an issue or a PR.
pub const COMMENTS_JQ: &str =
    ".[] | {id, body, url: .html_url, at: (.created_at | fromdateiso8601? // null)}";
/// What is read of each issue another is blocked by.
pub const BLOCKED_BY_JQ: &str = ".[] | {number, state}";
/// What is read of each label a repo has.
pub const LABELS_JQ: &str = ".[].name";
pub const PR_FIELDS: &str = "number,title,headRefName,headRefOid,body,closingIssuesReferences";

pub struct GitHub {
    pub dir: PathBuf,
}

/// One issue, open or closed, as `GitHub::issue` reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct IssueRead {
    /// GitHub's database id: what a dependency edge names an issue by.
    pub id: u64,
    pub title: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub pull_request: bool,
    /// The login of whoever closed it, where GitHub says.
    #[serde(default)]
    pub closed_by: Option<String>,
}

/// One issue another is recorded as blocked by — a native dependency edge.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockedBy {
    pub number: u64,
    pub state: String,
}

/// The one answer to which blockers are open, for both paged rebuilds and
/// the fresh read before a claim. REST and GraphQL spell state differently.
fn open_blockers(edges: Vec<BlockedBy>) -> Vec<u64> {
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

/// Every open issue, with the comments on each, as one read of GitHub saw
/// them.
#[derive(Debug, Default)]
pub struct OpenIssues {
    pub rows: Vec<Issue>,
    /// issue → its comments, oldest first.
    comments: HashMap<u64, Vec<Comment>>,
}

impl OpenIssues {
    /// Every comment on the open issue `number`, oldest first; none of an
    /// issue that is not among them.
    pub fn comments(&self, number: u64) -> &[Comment] {
        self.comments.get(&number).map_or(&[], Vec::as_slice)
    }
}

/// What is open on GitHub, as a rebuild reads it.
#[derive(Debug, Default)]
pub struct Open {
    pub issues: OpenIssues,
    pub prs: Vec<PullRequest>,
}

/// How many times a read is made before it is said that GitHub could not be
/// reached, and how long is waited between two of them.
const TRIES: u32 = 3;
const BETWEEN_TRIES: Duration = Duration::from_secs(1);

/// A read GitHub gave no answer to, each of the `TRIES` times it was made: it
/// failed in transit. Nothing is known of what was asked about, and nothing
/// is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreached {
    /// The call that was made: `gh api repos/acme/dev/commits/HEAD`.
    pub call: String,
    /// What `gh` said of the last try.
    pub said: String,
}

impl std::fmt::Display for Unreached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GitHub could not be reached: {}: {}", self.call, self.said)
    }
}

impl std::error::Error for Unreached {}

/// The read `error` says got no answer, when that is why it was raised,
/// whatever was said around it since.
pub fn unreached(error: &anyhow::Error) -> Option<&Unreached> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

/// An issue GitHub answered for with 410 or 404: deleted, or transferred to
/// another repo. Like a closed one, it is never worked again, and it is
/// nothing the rest of the fleet waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gone {
    pub issue: u64,
    /// 410: it was deleted. 404: it is not in this repo — transferred.
    pub deleted: bool,
}

impl Gone {
    /// What happened to it, as a tick's line says why it let go of it:
    /// `released #231 (the issue was deleted)`.
    pub fn said(&self) -> String {
        match self.deleted {
            true => "the issue was deleted".to_string(),
            false => "the issue is gone from this repo".to_string(),
        }
    }
}

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.deleted {
            true => write!(f, "issue #{} was deleted (HTTP 410)", self.issue),
            false => write!(f, "issue #{} is not in this repo (HTTP 404): transferred", self.issue),
        }
    }
}

impl std::error::Error for Gone {}

/// The issue `error` says is gone, when that is why it was raised, whatever
/// was said around it since.
pub fn gone(error: &anyhow::Error) -> Option<&Gone> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

/// The HTTP status what `gh` said of a call that failed names, when it
/// names one: `gh: Not Found (HTTP 404)` → 404.
fn status(said: &str) -> Option<u16> {
    let said = said.to_ascii_lowercase();
    let (_, after) = said.split_once("http ")?;
    after.get(..3).filter(|code| code.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()
}

/// `made`, a call about issue `number`, with a failure GitHub answered 410
/// or 404 raised as `Gone`.
fn about_issue<T>(number: u64, made: Result<T>) -> Result<T> {
    made.map_err(|error| match status(&format!("{error}")) {
        Some(410) => Gone { issue: number, deleted: true }.into(),
        Some(404) => Gone { issue: number, deleted: false }.into(),
        _ => error,
    })
}

/// Whether what `gh` said of a call that failed says it failed in transit:
/// an answer cut short, a 5xx, a connection reset or a timeout. Any other
/// status — a 404, a 403 — is GitHub's answer, and asking again gets the same.
fn in_transit(said: &str) -> bool {
    if let Some(status) = status(said) {
        return status / 100 == 5;
    }
    let said = said.to_ascii_lowercase();
    let cut_short = ["unexpected end of json input", "unexpected eof", ": eof"];
    let unanswered = ["connection reset", "timeout", "timed out"];
    cut_short.iter().chain(&unanswered).any(|mark| said.contains(mark))
}

/// What a read made beside another came to: a panic in it is this thread's.
pub fn beside<T>(read: thread::ScopedJoinHandle<'_, T>) -> T {
    read.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

impl IssueRead {
    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }
}

impl GitHub {
    /// One `gh` call → its stdout, or what it is named by and what `gh` said
    /// of its failing.
    fn call(&self, args: &[&str]) -> Result<Result<String, (String, String)>> {
        let out = Command::new("gh")
            .current_dir(&self.dir)
            .args(args)
            .output()
            .context("gh could not run")?;
        if !out.status.success() {
            let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
            let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Ok(Err((format!("gh {what}"), said)));
        }
        Ok(Ok(String::from_utf8_lossy(&out.stdout).into_owned()))
    }

    /// One `gh` call that must succeed → its stdout. It is made once: what
    /// writes is not made again on a failure that may have written.
    pub fn gh(&self, args: &[&str]) -> Result<String> {
        match self.call(args)? {
            Ok(out) => Ok(out),
            Err((call, said)) => bail!("{call} failed: {said}"),
        }
    }

    /// One `gh` call that only reads → its stdout. One that fails in transit
    /// (`in_transit`) is made again, `TRIES` times in all; none of them
    /// answered is `Unreached`. A failure GitHub answered with is as `gh`'s.
    pub fn read(&self, args: &[&str]) -> Result<String> {
        let mut tries = 1;
        loop {
            match self.call(args)? {
                Ok(out) => return Ok(out),
                Err((call, said)) if !in_transit(&said) => bail!("{call} failed: {said}"),
                Err((call, said)) if tries == TRIES => return Err(Unreached { call, said }.into()),
                Err(_) => thread::sleep(BETWEEN_TRIES),
            }
            tries += 1;
        }
    }

    /// The commit at the head of the default branch of `repo`.
    pub fn head_commit(&self, repo: &RepoName) -> Result<String> {
        let out = self.read(&["api", &format!("repos/{repo}/commits/HEAD"), "--jq", ".sha"])?;
        let commit = out.trim();
        let plain = !commit.is_empty() && commit.bytes().all(|b| b.is_ascii_hexdigit());
        if !plain {
            bail!("gh api: {repo} has no commit at the head of its default branch");
        }
        Ok(commit.to_string())
    }

    /// Clones the head of `repo`'s default branch, and nothing behind it, into
    /// the bare repo `into`. `gh` picks the protocol the user set it up with.
    pub fn clone_head(&self, repo: &RepoName, into: &std::path::Path) -> Result<()> {
        let into = into.to_string_lossy();
        self.gh(&[
            "repo",
            "clone",
            repo.as_str(),
            &into,
            "--",
            "--bare",
            "--depth",
            "1",
            "--quiet",
        ])?;
        Ok(())
    }

    /// The protection of one branch, as far as it can be read. A branch with
    /// none is a read that succeeded; only a read GitHub refused, or one
    /// nobody can parse, is unreadable.
    pub fn protection(&self, branch: &str) -> Guard {
        let endpoint = format!("repos/{{owner}}/{{repo}}/branches/{branch}/protection");
        let out = match Command::new("gh").current_dir(&self.dir).args(["api", &endpoint]).output()
        {
            Ok(out) => out,
            Err(error) => return Guard::Unreadable(format!("gh could not run: {error}")),
        };
        let (stdout, stderr) =
            (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() {
            let said = format!("{stderr}{stdout}");
            return if said.contains("Branch not protected") {
                Guard::Unprotected
            } else {
                Guard::Unreadable(stderr.trim().to_string())
            };
        }
        #[derive(Deserialize)]
        struct Check {
            context: String,
        }
        #[derive(Deserialize, Default)]
        struct Checks {
            #[serde(default)]
            contexts: Vec<String>,
            #[serde(default)]
            checks: Vec<Check>,
        }
        #[derive(Deserialize)]
        struct Switch {
            enabled: bool,
        }
        #[derive(Deserialize)]
        struct Rules {
            required_status_checks: Option<Checks>,
            required_pull_request_reviews: Option<serde_json::Value>,
            restrictions: Option<serde_json::Value>,
            lock_branch: Option<Switch>,
        }
        let Ok(rules) = serde_json::from_str::<Rules>(&stdout) else {
            return Guard::Unreadable("GitHub's answer could not be parsed".to_string());
        };
        let checks = rules.required_status_checks.unwrap_or_default();
        let mut required = checks.contexts;
        required.extend(checks.checks.into_iter().map(|check| check.context));
        required.sort();
        required.dedup();
        Guard::Protected(Protection::new(
            required,
            rules.required_pull_request_reviews.is_some(),
            rules.restrictions.is_some(),
            rules.lock_branch.is_some_and(|lock| lock.enabled),
        ))
    }

    /// Every open issue with its comments — all of them, page after page:
    /// the number of calls does not grow with the issues that have comments.
    /// Only an issue with more than `COMMENTS_WITH` comments is asked about
    /// again, for all of them.
    pub fn open_issues(&self) -> Result<OpenIssues> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            title: String,
            labels: Vec<String>,
            blockers: Vec<BlockedBy>,
            blockers_more: bool,
            more: bool,
            comments: Vec<Comment>,
        }
        let query = format!("query={OPEN_ISSUES_QUERY}");
        let out = self.read(&[
            "api",
            "graphql",
            "--paginate",
            "-F",
            "owner={owner}",
            "-F",
            "repo={repo}",
            "-f",
            &query,
            "--jq",
            OPEN_ISSUES_JQ,
        ])?;
        let mut open = OpenIssues::default();
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api graphql: unexpected answer")?;
            // Native dependencies are capped below this page size. Refuse a
            // truncated answer if GitHub ever raises that limit; never infer
            // an unblocked issue from an incomplete read.
            if row.blockers_more {
                bail!(
                    "GitHub returned more than 100 blockers for #{}; cannot rebuild a complete frontier",
                    row.number
                );
            }
            let comments = if row.more { self.comments(row.number)? } else { row.comments };
            open.comments.insert(row.number, comments);
            open.rows.push(Issue {
                number: row.number,
                title: row.title,
                labels: row.labels,
                open_blockers: open_blockers(row.blockers),
            });
        }
        Ok(open)
    }

    /// Patrol ownership and completion times in the rolling day. Read only
    /// when a configured patrol has a landing bound. Ownership follows the
    /// latest intake record, regardless of the issue's labels now.
    pub fn patrol_landings(&self, now: u64) -> Result<Vec<(String, u64)>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            at: Option<u64>,
            more: bool,
            comments: Vec<Comment>,
        }
        let query = format!("query={LANDED_ISSUES_QUERY}");
        let out = self.read(&[
            "api",
            "graphql",
            "--paginate",
            "-F",
            "owner={owner}",
            "-F",
            "repo={repo}",
            "-f",
            &query,
            "--jq",
            LANDED_ISSUES_JQ,
        ])?;
        let mut landed = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api graphql: unexpected answer")?;
            let Some(at) = row.at else { continue };
            if now.saturating_sub(at) >= afkconvoy_core::patrol::DAY {
                continue;
            }
            let comments = if row.more { self.comments(row.number)? } else { row.comments };
            if let Some(patrol) =
                afkconvoy_core::intake::opened_by(comments.iter().map(|c| c.body.as_str()))
            {
                landed.push((patrol, at));
            }
        }
        Ok(landed)
    }

    /// What is open: the issues and the PRs, asked for side by side.
    pub fn open(&self) -> Result<Open> {
        let (issues, prs) = thread::scope(|scope| {
            let prs = scope.spawn(|| self.open_prs());
            (self.open_issues(), beside(prs))
        });
        Ok(Open { issues: issues?, prs: prs? })
    }

    /// Every closed issue that carries `label`, each with when it closed, in
    /// epoch seconds — but for one GitHub gives no time for.
    pub fn closed_with(&self, label: &str) -> Result<Vec<(u64, u64)>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            at: Option<u64>,
        }
        let label = crate::judge::label_segment(label);
        let endpoint =
            format!("repos/{{owner}}/{{repo}}/issues?state=closed&labels={label}&per_page=100");
        let out = self.read(&["api", "--paginate", &endpoint, "--jq", CLOSED_JQ])?;
        let mut closed = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            closed.extend(row.at.map(|at| (row.number, at)));
        }
        Ok(closed)
    }

    /// Every issue that carries `label`, open or closed, each with when it
    /// closed, in epoch seconds — `None`: it is open, `Some(None)`: GitHub
    /// gives no time for it.
    pub fn carrying(&self, label: &str) -> Result<Vec<(u64, Option<Option<u64>>)>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            open: bool,
            closed_at: Option<u64>,
        }
        let label = crate::judge::label_segment(label);
        let endpoint =
            format!("repos/{{owner}}/{{repo}}/issues?state=all&labels={label}&per_page=100");
        let out = self.read(&["api", "--paginate", &endpoint, "--jq", CARRYING_JQ])?;
        let mut carrying = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            carrying.push((row.number, (!row.open).then_some(row.closed_at)));
        }
        Ok(carrying)
    }

    /// Every open pull request.
    pub fn open_prs(&self) -> Result<Vec<PullRequest>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Row {
            number: u64,
            title: Option<String>,
            head_ref_name: String,
            head_ref_oid: String,
            body: Option<String>,
            closing_issues_references: Option<Vec<Linked>>,
        }
        #[derive(Deserialize)]
        struct Linked {
            number: u64,
        }
        let out =
            self.read(&["pr", "list", "--state", "open", "--limit", "1000", "--json", PR_FIELDS])?;
        let rows: Vec<Row> = serde_json::from_str(&out).context("gh pr list: unexpected answer")?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let linked: Vec<u64> = row
                    .closing_issues_references
                    .unwrap_or_default()
                    .iter()
                    .map(|l| l.number)
                    .collect();
                PullRequest {
                    number: row.number,
                    title: row.title.unwrap_or_default(),
                    head_branch: row.head_ref_name,
                    head_sha: row.head_ref_oid,
                    closes: PullRequest::closing(row.body.as_deref().unwrap_or(""), &linked),
                }
            })
            .collect())
    }

    /// One issue, open or closed. An error when it cannot be read: `Gone`
    /// when GitHub says it was deleted or transferred.
    pub fn issue(&self, number: u64) -> Result<IssueRead> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}");
        let out = about_issue(number, self.read(&["api", &endpoint, "--jq", ISSUE_JQ]))?;
        serde_json::from_str(&out)
            .with_context(|| format!("gh api issue {number}: unexpected answer"))
    }

    /// Every comment on an issue or a PR, oldest first: `Gone` of one GitHub
    /// says was deleted or transferred.
    pub fn comments(&self, number: u64) -> Result<Vec<Comment>> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments?per_page=100");
        let read = self.read(&["api", "--paginate", &endpoint, "--jq", COMMENTS_JQ]);
        let out = about_issue(number, read)?;
        out.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("gh api comments: unexpected answer"))
            .collect()
    }

    /// Posts one comment on an issue or a PR: `Gone` of one GitHub says was
    /// deleted or transferred.
    pub fn comment(&self, number: u64, body: &str) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments");
        let body = format!("body={body}");
        about_issue(number, self.gh(&["api", "--method", "POST", &endpoint, "-f", &body]))?;
        Ok(())
    }

    /// Replaces the body of one comment, on an issue or a PR.
    pub fn edit_comment(&self, id: u64, body: &str) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/comments/{id}");
        self.gh(&[
            "api",
            "--method",
            "PATCH",
            &endpoint,
            "-f",
            &format!("body={body}"),
            "--silent",
        ])?;
        Ok(())
    }

    /// Opens an issue carrying `labels` → its number.
    pub fn open_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64> {
        let (title, body) = (format!("title={title}"), format!("body={body}"));
        let labels: Vec<String> = labels.iter().map(|label| format!("labels[]={label}")).collect();
        let mut args = vec![
            "api",
            "--method",
            "POST",
            "repos/{owner}/{repo}/issues",
            "-f",
            &title,
            "-f",
            &body,
        ];
        for label in &labels {
            args.extend(["-f", label.as_str()]);
        }
        args.extend(["--jq", ".number"]);
        let out = self.gh(&args)?;
        out.trim().parse().with_context(|| format!("gh api issues: no number in {out:?}"))
    }

    /// Closes one issue as completed.
    pub fn close_issue(&self, number: u64) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}");
        self.gh(&["api", "--method", "PATCH", &endpoint, "-f", "state=closed", "--silent"])?;
        Ok(())
    }

    /// Closes one PR without merging it, with a comment saying why.
    pub fn close_pr(&self, number: u64, comment: &str) -> Result<()> {
        self.gh(&["pr", "close", &number.to_string(), "--comment", comment])?;
        Ok(())
    }

    /// The open PRs of another repo, `repo`: the number and the head branch
    /// of each.
    pub fn open_prs_of(&self, repo: &RepoName) -> Result<Vec<(u64, String)>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Row {
            number: u64,
            head_ref_name: String,
        }
        let fields = "number,headRefName";
        let out = self.read(&[
            "pr",
            "list",
            "--repo",
            repo.as_str(),
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            fields,
        ])?;
        let rows: Vec<Row> = serde_json::from_str(&out).context("gh pr list: unexpected answer")?;
        Ok(rows.into_iter().map(|row| (row.number, row.head_ref_name)).collect())
    }

    /// What PR `number` of another repo, `repo`, changes, as `git diff`
    /// writes it.
    pub fn pr_diff_of(&self, repo: &RepoName, number: u64) -> Result<String> {
        self.read(&["pr", "diff", &number.to_string(), "--repo", repo.as_str()])
    }

    /// Closes one PR of another repo, `repo`, without merging it, with a
    /// comment saying why.
    pub fn close_pr_of(&self, repo: &RepoName, number: u64, comment: &str) -> Result<()> {
        let number = number.to_string();
        self.gh(&["pr", "close", &number, "--repo", repo.as_str(), "--comment", comment])?;
        Ok(())
    }

    /// The issues GitHub records `number` as blocked by, open or closed:
    /// `Gone` when `number` was deleted or transferred.
    pub fn blocked_by(&self, number: u64) -> Result<Vec<BlockedBy>> {
        let endpoint = format!(
            "repos/{{owner}}/{{repo}}/issues/{number}/dependencies/blocked_by?per_page=100"
        );
        let read = self.read(&["api", "--paginate", &endpoint, "--jq", BLOCKED_BY_JQ]);
        let out = about_issue(number, read)?;
        out.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("gh api blocked_by: unexpected answer"))
            .collect()
    }

    /// The open issues among those `number` is blocked by.
    pub fn open_blockers(&self, number: u64) -> Result<Vec<u64>> {
        Ok(open_blockers(self.blocked_by(number)?))
    }

    /// Records `number` as blocked by the issue whose database id is `blocker_id`.
    pub fn add_blocker(&self, number: u64, blocker_id: u64) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/dependencies/blocked_by");
        let field = format!("issue_id={blocker_id}");
        self.gh(&["api", "--method", "POST", &endpoint, "-F", &field, "--silent"])?;
        Ok(())
    }

    /// Every label the repo has, by name.
    pub fn labels(&self) -> Result<Vec<String>> {
        let endpoint = "repos/{owner}/{repo}/labels?per_page=100";
        let out = self.read(&["api", "--paginate", endpoint, "--jq", LABELS_JQ])?;
        Ok(out.lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
    }

    /// Creates the label `name` on the repo, which has none of that name, to
    /// be shown with `description`.
    pub fn create_label(&self, name: &str, description: &str) -> Result<()> {
        let (name, description) = (format!("name={name}"), format!("description={description}"));
        let endpoint = "repos/{owner}/{repo}/labels";
        self.gh(&[
            "api",
            "--method",
            "POST",
            endpoint,
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

    /// Puts one label on an issue; GitHub creates a label the repo lacks.
    pub fn add_label(&self, number: u64, label: &str) -> Result<()> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/labels");
        let field = format!("labels[]={label}");
        self.gh(&["api", "--method", "POST", &endpoint, "-f", &field, "--silent"])?;
        Ok(())
    }

    /// Takes one label off an issue that carries it.
    pub fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let label = crate::judge::label_segment(label);
        let endpoint = format!("repos/{{owner}}/{{repo}}/issues/{number}/labels/{label}");
        self.gh(&["api", "--method", "DELETE", &endpoint, "--silent"])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_failed_in_transit_when_it_got_no_answer() {
        for said in [
            "unexpected end of JSON input",
            "gh: Server Error (HTTP 502)",
            "HTTP 503: Service Unavailable (https://api.github.com/graphql)",
            "Get \"https://api.github.com/repos/a/b\": read tcp: connection reset by peer",
            "Post \"https://api.github.com/graphql\": net/http: TLS handshake timeout",
            "Get \"https://api.github.com/repos/a/b\": context deadline exceeded (Client.Timeout exceeded while awaiting headers)",
            "Post \"https://api.github.com/graphql\": unexpected EOF",
            "Post \"https://api.github.com/graphql\": EOF",
        ] {
            assert!(in_transit(said), "{said}");
        }
        for said in [
            "gh: Not Found (HTTP 404)",
            "gh: Resource not accessible by integration (HTTP 403)",
            "HTTP 404: Not Found (https://api.github.com/repos/a/timeout)",
            "GraphQL: Could not resolve to a Repository with the name 'a/b'.",
            "fake gh: injected failure",
        ] {
            assert!(!in_transit(said), "{said}");
        }
    }

    #[test]
    fn an_issue_answered_with_410_or_404_is_gone_and_any_other_failure_is_as_it_was() {
        let failed = |said: &str| -> Result<()> { bail!("gh api failed: {said}") };
        let deleted = about_issue(231, failed("gh: This issue was deleted (HTTP 410)"));
        let error = deleted.unwrap_err().context("the claim cannot be read");
        assert_eq!(gone(&error), Some(&Gone { issue: 231, deleted: true }));
        let moved = about_issue(7, failed("gh: Not Found (HTTP 404)")).unwrap_err();
        assert_eq!(gone(&moved), Some(&Gone { issue: 7, deleted: false }));
        for said in ["gh: Resource not accessible by integration (HTTP 403)", "gh: no"] {
            let error = about_issue(7, failed(said)).unwrap_err();
            assert_eq!((gone(&error), error.to_string()), (None, format!("gh api failed: {said}")));
        }
        assert_eq!(Gone { issue: 231, deleted: true }.said(), "the issue was deleted");
    }

    #[test]
    fn a_read_that_got_no_answer_is_found_under_what_was_said_around_it() {
        let call = "gh api repos/acme/dev/commits/HEAD".to_string();
        let raised = Unreached { call, said: "unexpected end of JSON input".to_string() };
        let error =
            anyhow::Error::from(raised.clone()).context("the team acme/team cannot be read");
        assert_eq!(unreached(&error), Some(&raised));
        assert_eq!(unreached(&anyhow::anyhow!("gh pr list failed: no")), None);
        assert_eq!(
            raised.to_string(),
            "GitHub could not be reached: gh api repos/acme/dev/commits/HEAD: unexpected end of \
             JSON input"
        );
    }
}

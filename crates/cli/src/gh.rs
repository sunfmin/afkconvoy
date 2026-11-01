//! GitHub, through `gh` as a subprocess run in the checkout. Its PRs and the
//! protection of its branches are the checkout's repo's, which `gh` works out
//! from the checkout, so nothing here names it. Every issue — the open ones,
//! their comments, blockers and labels, one opened or closed — is the
//! tracker's (`Tracker`), which every issue call names. In a tracker that is
//! not the checkout's own repo, the fleet's issues are those that carry its
//! repo label: the open ones are read by it, and one opened carries it.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::layout::{Guard, Protection};
use afkconvoy_core::plan::{Issue, PullRequest};
use afkconvoy_core::tracker::Linked;
pub use afkconvoy_core::tracker::Tracker;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::git::Repo;
use crate::look::Reading;

/// The open issues, a page at a time, each with its labels, blockers and
/// its first `COMMENTS_WITH` comments: one call a page,
/// however many of the issues have comments to read. `$owner` and `$repo`
/// name the tracker (`GitHub::query`), `$labels` — where it is given — the
/// repo label every issue read carries, and `gh api graphql` fills
/// `$endCursor` in page after page. Ten embedded blockers keep the nested
/// label connections at five GraphQL points per page instead of forty-one.
/// A longer blocker list is read in full through the paginated REST endpoint.
pub const OPEN_ISSUES_QUERY: &str = "query($owner: String!, $repo: String!, $labels: [String!], \
    $endCursor: String) { \
    repository(owner: $owner, name: $repo) { \
    issues(states: OPEN, labels: $labels, first: 40, after: $endCursor, \
    orderBy: {field: CREATED_AT, direction: DESC}) { \
    pageInfo { hasNextPage endCursor } \
    nodes { number title body url createdAt lastEditedAt labels(first: 100) { nodes { name } } \
    blockedBy(first: 10) { pageInfo { hasNextPage } nodes { number state \
    repository { nameWithOwner } labels(first: 100) { nodes { name } } } } \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } } }";
/// What is read of every open issue: `more` says its comments go on past
/// those that came with it.
pub const OPEN_ISSUES_JQ: &str = ".data.repository.issues.nodes[] | {number, title, body, url, \
    at: (.createdAt | fromdateiso8601? // null), edited: (.lastEditedAt | fromdateiso8601? // null), \
    labels: [.labels.nodes[].name], blockers: [.blockedBy.nodes[] | select(. != null) | \
    {number, state, repo: .repository.nameWithOwner, labels: [.labels.nodes[].name]}], \
    blockers_more: .blockedBy.pageInfo.hasNextPage, \
    more: .comments.pageInfo.hasNextPage, comments: [.comments.nodes[] | \
    {id: (.fullDatabaseId | tonumber), body, url, at: (.createdAt | fromdateiso8601? // null)}]}";
/// Completed issues with closure times and intake comments, paged together so
/// reading a patrol's landings never makes one request per ordinary issue:
/// of a tracker that is another repo, those carrying the repo label, `$labels`
/// (`OPEN_ISSUES_QUERY`).
pub const LANDED_ISSUES_QUERY: &str = "query($owner: String!, $repo: String!, \
    $labels: [String!], $endCursor: String) { \
    repository(owner: $owner, name: $repo) { \
    issues(states: CLOSED, labels: $labels, first: 100, after: $endCursor, \
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
/// Every issue of the tracker, without filtering by a code repo label.
pub const REWRITE_ACCOUNTS_QUERY: &str = "query ConvoyRewriteAccounts($owner: String!, $repo: String!, \
    $endCursor: String) { repository(owner: $owner, name: $repo) { \
    issues(first: 100, after: $endCursor) { pageInfo { hasNextPage endCursor } \
    nodes { number url closed labels(first: 100) { nodes { name } } \
    timelineItems(last: 1, itemTypes: [CLOSED_EVENT]) { nodes { ... on ClosedEvent { \
    closer { __typename ... on PullRequest { merged } ... on Commit { \
    associatedPullRequests(first: 10) { nodes { merged } } } } } } } \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } } }";
pub const REWRITE_ACCOUNTS_JQ: &str = ".data.repository.issues.nodes[] | {number, url, closed, \
    labels: [.labels.nodes[].name], more: .comments.pageInfo.hasNextPage, \
    landed: ([.timelineItems.nodes[].closer | select((.__typename == \"PullRequest\" and .merged == true) or \
    (.__typename == \"Commit\" and any(.associatedPullRequests.nodes[]?; .merged == true)))] | length > 0), \
    comments: [.comments.nodes[] | {id: (.fullDatabaseId | tonumber), body, url, \
    at: (.createdAt | fromdateiso8601? // null)}]}";

/// How many closed issues the open read takes with it. More than this and the
/// day may run past the page, so the landed read falls back to its own pages.
pub const LANDED_PAGE: usize = 100;
/// The open issues and, beside them, the closed issues updated since `$since`,
/// in one call. The closed connection has no `pageInfo`: `gh api --paginate`
/// follows the first `endCursor` it finds, and a second one is not a page of
/// the open issues. A page repeats the closed issues; the reader keeps one of
/// each. Only a patrol that bounds its landings in a day asks this.
pub const OPEN_AND_LANDED_QUERY: &str = "query($owner: String!, $repo: String!, \
    $labels: [String!], $endCursor: String, $since: DateTime!) { \
    repository(owner: $owner, name: $repo) { \
    issues(states: OPEN, labels: $labels, first: 40, after: $endCursor, \
    orderBy: {field: CREATED_AT, direction: DESC}) { \
    pageInfo { hasNextPage endCursor } \
    nodes { number title body url createdAt lastEditedAt labels(first: 100) { nodes { name } } \
    blockedBy(first: 10) { pageInfo { hasNextPage } nodes { number state \
    repository { nameWithOwner } labels(first: 100) { nodes { name } } } } \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } \
    landed: issues(states: CLOSED, labels: $labels, filterBy: {since: $since}, \
    first: 100, orderBy: {field: UPDATED_AT, direction: DESC}) { \
    nodes { number stateReason closedAt \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } } }";
pub const OPEN_AND_LANDED_JQ: &str = ".data.repository as $repo \
    | ($repo.landed.nodes | length) as $n \
    | ($repo.issues.nodes[] | {part: \"open\", number, title, body, url, \
    at: (.createdAt | fromdateiso8601? // null), edited: (.lastEditedAt | fromdateiso8601? // null), \
    labels: [.labels.nodes[].name], blockers: [.blockedBy.nodes[] | select(. != null) | \
    {number, state, repo: .repository.nameWithOwner, labels: [.labels.nodes[].name]}], \
    blockers_more: .blockedBy.pageInfo.hasNextPage, \
    more: .comments.pageInfo.hasNextPage, comments: [.comments.nodes[] | \
    {id: (.fullDatabaseId | tonumber), body, url, at: (.createdAt | fromdateiso8601? // null)}]}), \
    ($repo.landed.nodes[] | select(.stateReason == \"COMPLETED\") | \
    {part: \"landed\", number, at: (.closedAt | fromdateiso8601? // null), \
    more: .comments.pageInfo.hasNextPage, comments: [.comments.nodes[] | \
    {id: (.fullDatabaseId | tonumber), body, url, at: (.createdAt | fromdateiso8601? // null)}]}), \
    {part: \"landed_count\", n: $n}";
/// Issues updated since a watching agent's last look, open or closed, in
/// the tracker and scoped to this repo's label. Comments past the first page
/// are read through the ordinary comments endpoint.
pub const WATCH_ISSUES_QUERY: &str = "query($owner: String!, $repo: String!, \
    $labels: [String!], $since: DateTime!, $endCursor: String) { \
    repository(owner: $owner, name: $repo) { \
    issues(labels: $labels, filterBy: {since: $since}, first: 100, after: $endCursor, \
    orderBy: {field: UPDATED_AT, direction: ASC}) { \
    pageInfo { hasNextPage endCursor } nodes { number title closedAt \
    labels(first: 100) { nodes { name } } \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } } }";
pub const WATCH_ISSUES_JQ: &str = ".data.repository.issues.nodes[] | \
    {number, title, closed: (.closedAt | fromdateiso8601? // null), \
    labels: [.labels.nodes[].name], more: .comments.pageInfo.hasNextPage, \
    comments: [.comments.nodes[] | \
    {id: (.fullDatabaseId | tonumber), body, url, at: (.createdAt | fromdateiso8601? // null)}]}";
/// Every issue, open or closed, with account comments. This is a connection,
/// not search: there is no search-result cap. Long comment threads are read
/// separately, and their read failures belong to that issue's unreadable row.
pub const ACCOUNTS_QUERY: &str = "query ConvoyAccounts($owner: String!, $repo: String!, \
    $labels: [String!], $endCursor: String) { repository(owner: $owner, name: $repo) { \
    issues(labels: $labels, first: 100, after: $endCursor) { \
    pageInfo { hasNextPage endCursor } nodes { number url closed labels(first: 100) { nodes { name } } \
    comments(first: 100) { pageInfo { hasNextPage } \
    nodes { fullDatabaseId body url createdAt } } } } } }";
pub const ACCOUNTS_JQ: &str = ".data.repository.issues.nodes[] | {number, url, closed, \
    labels: [.labels.nodes[].name], more: .comments.pageInfo.hasNextPage, \
    comments: [.comments.nodes[] | {id: (.fullDatabaseId | tonumber), body, url, \
    at: (.createdAt | fromdateiso8601? // null)}]}";
/// PRs ordered by most recently updated: watching stops paging once a page
/// reaches before its lower boundary, including every tie at that boundary.
pub const WATCH_PRS_JQ: &str = ".[] | {number, title, body, head_branch: .head.ref, \
    updated: (.updated_at | fromdateiso8601), \
    closed: (.closed_at | fromdateiso8601? // null), \
    merged: (.merged_at | fromdateiso8601? // null), \
    fork: (.head.repo.full_name != .base.repo.full_name), of: .base.repo.full_name}";

#[derive(Debug, Deserialize)]
pub struct WatchIssue {
    pub number: u64,
    pub title: String,
    pub closed: Option<u64>,
    pub labels: Vec<String>,
    pub more: bool,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Deserialize)]
pub struct WatchPr {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub head_branch: String,
    pub updated: u64,
    pub closed: Option<u64>,
    pub merged: Option<u64>,
    pub fork: bool,
    pub of: Option<String>,
}
/// How many comments of an issue come with it (`OPEN_ISSUES_QUERY`).
pub const COMMENTS_WITH: usize = 100;
/// What is read of one issue, open or not.
pub const ISSUE_JQ: &str = "{id, url, title, body, state, state_reason, labels: [.labels[].name], \
    pull_request: (.pull_request != null), closed_by: .closed_by.login}";
/// What is read of each closed issue that carries one label.
pub const CLOSED_JQ: &str = ".[] | select(.pull_request == null) | \
    {number, at: (.closed_at | fromdateiso8601? // null)}";
/// What is read of each issue that carries one label, open or closed.
pub const CARRYING_JQ: &str = ".[] | select(.pull_request == null) | \
    {number, open: (.state == \"open\"), closed_at: (.closed_at | fromdateiso8601? // null)}";
/// Open issue identities and labels, without comments or dependency reads.
pub const OPEN_LABELLED_JQ: &str = ".[] | select(.pull_request == null) | \
    {number, labels: [.labels[].name]}";
/// Same-repository PR heads. No closing-issue relationships are inferred.
pub const PR_HEADS_JQ: &str = ".[] | select(.head.repo != null and \
    .head.repo.full_name == .base.repo.full_name) | \
    {number, title: (.title // \"\"), body: (.body // \"\"), head_branch: .head.ref, head_sha: .head.sha, base_branch: .base.ref}";

/// Full history for final-review recovery, including closed and merged PRs.
pub const DELIVERY_PRS_JQ: &str = ".[] | {number, body: (.body // \"\"), head_branch: .head.ref, base_branch: .base.ref, state, draft, merged, url: .html_url, fork: (.head.repo.full_name != .base.repo.full_name)}";

#[derive(Clone, Deserialize)]
pub struct DeliveryPr {
    pub number: u64,
    pub body: String,
    pub head_branch: String,
    pub base_branch: String,
    pub state: String,
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    pub url: String,
    pub fork: bool,
}
/// What is read of each comment on an issue or a PR.
pub const COMMENTS_JQ: &str =
    ".[] | {id, body, url: .html_url, at: (.created_at | fromdateiso8601? // null)}";
/// What is read of each issue another is blocked by.
pub const BLOCKED_BY_JQ: &str = ".[] | {number, state, repo: (.repository_url | split(\"/repos/\")[1]), labels: [.labels[].name]}";
/// What is read of each label a repo has.
pub const LABELS_JQ: &str = ".[].name";
/// What is read of a repo for who may see it: `public`, `private` or
/// `internal`.
pub const VISIBILITY_JQ: &str = ".visibility";
/// What is read of each open PR: `isCrossRepository` says its head is in
/// another repository — a fork's (`GitHub::open_prs`) — and
/// `headRepository` and `headRepositoryOwner` name the repo it is of.
pub const PR_FIELDS: &str = "number,title,headRefName,headRefOid,baseRefName,body,closingIssuesReferences,\
    isCrossRepository,headRepository,headRepositoryOwner";
/// What is read of each open PR of another repo.
pub const PR_OF_FIELDS: &str = "number,headRefName,isCrossRepository,createdAt";
/// The creation time of another repo's PR in epoch seconds, where GitHub says it.
pub const PR_OF_JQ: &str = "map({number, headRefName, isCrossRepository, \
    since: (.createdAt | fromdateiso8601? // null)})";

/// The checkout's own repo, as an endpoint names it: `gh` fills it in from
/// the checkout it runs in.
const OWN: &str = "{owner}/{repo}";

/// The tracker as an endpoint names it: `{owner}/{repo}`, which `gh` fills
/// in from the checkout, or `owner/repo`.
fn path(tracker: &Tracker) -> &str {
    tracker.named().map_or(OWN, RepoName::as_str)
}

/// The variables of a query of the tracker, as `gh api graphql` takes them:
/// `owner` and `repo` — `-F` fills `{owner}` and `{repo}` in, and `-f`
/// sends a name as it is, never as a number — and, of another repo, the
/// one label of `labels`, its repo label.
fn variables(tracker: &Tracker) -> Vec<String> {
    let (flag, owner, repo) = match tracker.named() {
        None => ("-F", "{owner}", "{repo}"),
        Some(named) => {
            let (owner, repo) = named.as_str().split_once('/').expect("owner/repo");
            ("-f", owner, repo)
        }
    };
    let mut variables =
        vec![flag.to_string(), format!("owner={owner}"), flag.to_string(), format!("repo={repo}")];
    if let Some(label) = tracker.repo_label() {
        variables.extend(["-f".to_string(), format!("labels[]={label}")]);
    }
    variables
}

/// The endpoint `rest` of the tracker: `repos/{owner}/{repo}/issues`.
fn endpoint(tracker: &Tracker, rest: &str) -> String {
    format!("repos/{}/{rest}", path(tracker))
}

/// What a listing of the tracker's issues that carry `label` asks for: of
/// another repo, those that carry its repo label too.
fn carrying_labels(tracker: &Tracker, label: &str) -> String {
    let label = crate::judge::label_segment(label);
    match tracker.repo_label() {
        Some(repo) => format!("{label},{}", crate::judge::label_segment(&repo)),
        None => label,
    }
}

/// Why a repo's HEAD was not read. A transit failure keeps its type.
#[derive(Debug, Clone)]
pub(crate) enum HeadMiss {
    Unreached(Unreached),
    Other(String),
}

/// One HEAD sha per repo, read once in this look. A transit failure stays
/// [`Unreached`], so a later ask in the look still says GitHub could not be
/// reached and does not read again. A process that is one look — `doctor` —
/// reads each HEAD once.
pub(crate) static HEADS: Reading<Result<String, HeadMiss>> = Reading::per_look("github head");
/// Tracker visibility is shared by the ordinary checkout look and the
/// tracker-wide work look. No fleet operation changes repository visibility.
static VISIBILITY: Reading<Result<String, String>> = Reading::per_look("github visibility");

/// `gh`, run in `dir`: the checkout a command is about, or where none is at
/// hand the host's temporary directory (`in_no_checkout`). Every issue call
/// is made of `tracker`.
#[derive(Clone)]
pub struct GitHub {
    pub dir: PathBuf,
    pub tracker: Tracker,
    /// Closures this view read or made, shared with its clones. These only
    /// route wakes; every woken tick reads GitHub for itself.
    closed: Arc<Mutex<BTreeSet<u64>>>,
    quota: Arc<Mutex<Option<crate::gh_quota::Scope>>>,
    /// The checkout's own repo, as its remote names it, where it does:
    /// asked once.
    own: Arc<OnceLock<Option<String>>>,
}

/// An open PR from another repo's own branch, with when it was opened.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherPr {
    pub number: u64,
    pub head_ref_name: String,
    pub since: Option<u64>,
}

impl GitHub {
    /// `gh` for the checkout `repo`: its PRs in its own repo, and its issues
    /// in the tracker it says on this host (`checkout_list::tracker_of`).
    pub fn of(repo: &Repo) -> Result<GitHub> {
        let tracker = crate::checkout_list::tracker_of(repo)?;
        Ok(GitHub::new(repo.dir.clone(), tracker))
    }

    /// A new read of `tracker` from `dir`, with its closure observations
    /// shared only by this view and its clones.
    pub fn new(dir: PathBuf, tracker: Tracker) -> GitHub {
        GitHub { dir, tracker, closed: Arc::default(), quota: Arc::default(), own: Arc::default() }
    }

    /// The issues this view has seen closed, whatever read or write saw it.
    pub fn closed(&self) -> Vec<u64> {
        self.closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .copied()
            .collect()
    }

    fn saw_closed(&self, number: u64) {
        self.closed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(number);
    }

    pub(crate) fn blockers(&self, edges: Vec<BlockedBy>) -> Vec<u64> {
        for edge in &edges {
            if edge.state.eq_ignore_ascii_case("closed") {
                self.saw_closed(edge.number);
            }
        }
        open_blockers(edges)
    }

    /// `gh` for the calls that each name their repo, where no checkout is at
    /// hand: they need none, and are made in the host's temporary directory.
    pub fn in_no_checkout() -> GitHub {
        GitHub::new(std::env::temp_dir(), Tracker::Own)
    }
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
    /// What it says, where it says anything.
    #[serde(default)]
    pub body: Option<String>,
    /// GitHub's API URL of it: of another repo, or another number, where
    /// GitHub redirected the read because the issue moved.
    #[serde(default)]
    pub url: Option<String>,
}

/// One issue another is recorded as blocked by — a native dependency edge.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockedBy {
    pub number: u64,
    pub state: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
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

/// Every open issue, with the comments on each, as one read of GitHub saw
/// them.
#[derive(Debug, Default)]
pub struct OpenIssues {
    pub rows: Vec<Issue>,
    /// Native edges retain repo identity: issue numbers alone are not unique.
    pub blockers: HashMap<u64, Vec<BlockedBy>>,
    /// issue → its comments, oldest first.
    comments: HashMap<u64, Vec<Comment>>,
    /// Failure reports written before reason comments were added still
    /// carry their reason in the issue body.
    reports: HashMap<u64, Comment>,
    edited: HashMap<u64, u64>,
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

    /// When GitHub last recorded an edit of the issue, where it records one.
    pub fn body_edited(&self, number: u64) -> Option<u64> {
        self.edited.get(&number).copied()
    }

    /// The body and link from the same read as the comments.
    pub fn body(&self, number: u64) -> Option<&Comment> {
        self.reports.get(&number)
    }
}

/// What is open on GitHub, as a rebuild reads it.
#[derive(Debug, Default)]
pub struct Open {
    pub issues: OpenIssues,
    pub prs: Vec<PullRequest>,
}

/// The open issues and the patrol landings of one day, from one issues query.
/// `landings` is `None` when the closed page may be short of the day.
struct OpenAndLanded {
    issues: OpenIssues,
    landings: Option<Vec<(String, u64)>>,
}

/// What is open, and the patrol landings read in that same query.
pub(crate) struct Opened {
    pub open: Open,
    /// `None`: this read did not ask, or the closed page may be short of the day.
    pub landings: Option<Vec<(String, u64)>>,
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

#[derive(Debug)]
struct PrimaryQuota(String);
impl std::fmt::Display for PrimaryQuota {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GitHub primary quotas unavailable: {}; next attempt at reset (Unix seconds)",
            self.0
        )
    }
}
impl std::error::Error for PrimaryQuota {}

/// The read `error` says got no answer, when that is why it was raised,
/// whatever was said around it since.
pub fn unreached(error: &anyhow::Error) -> Option<&Unreached> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

/// An issue GitHub has no more: deleted, transferred to another repo, or
/// never issued. Like a closed one, it is never worked again, and it is
/// nothing the rest of the fleet waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gone {
    pub issue: u64,
    pub how: Went,
}

/// How GitHub says an issue is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Went {
    /// 410: it was deleted.
    Deleted,
    /// GitHub moved it: a read of it follows GitHub's redirect into another
    /// repo, `to` — `owner/repo#n` — where GitHub says.
    Transferred { to: Option<String> },
    /// 404, and GitHub says nothing moved: there is no such issue.
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

/// The HTTP status what `gh` said of a call that failed names, when it
/// names one: `gh: Not Found (HTTP 404)` → 404.
fn status(said: &str) -> Option<u16> {
    let said = said.to_ascii_lowercase();
    let (_, after) = said.split_once("http ")?;
    after.get(..3).filter(|code| code.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()
}

/// `made`, a read of issue `number` itself, with a failure GitHub answered
/// 410 raised as `Gone` deleted, and 404 as `Gone` missing: a moved issue's
/// read follows GitHub's redirect, so a 404 of it is no move.
fn answered<T>(number: u64, made: Result<T>) -> Result<T> {
    made.map_err(|error| match status(&format!("{error}")) {
        Some(410) => Gone { issue: number, how: Went::Deleted }.into(),
        Some(404) => Gone { issue: number, how: Went::Missing }.into(),
        _ => error,
    })
}

/// Where the issue read as `url`, GitHub's API URL of it, is: `owner/repo`
/// and its number.
fn located(url: &str) -> Option<(String, u64)> {
    let path = url.split_once("/repos/")?.1;
    let (repo, number) = path.rsplit_once("/issues/")?;
    Some((repo.to_string(), number.parse().ok()?))
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
        let mut command = Command::new("gh");
        command.current_dir(&self.dir).args(args);
        let text_body = args.iter().any(|s| {
            s.contains("application/vnd.github.raw")
                || s.contains("application/vnd.github.diff")
                || *s == LABELS_JQ
        });
        let included = args.first() == Some(&"api") && !text_body;
        if included {
            command.arg("--include");
        }
        let out = crate::process::output(&mut command).context("gh could not run")?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let (body, headers) = if included {
            crate::gh_quota::response(&stdout)
        } else {
            (stdout.into_owned(), String::new())
        };
        if out.status.success()
            && let Ok(scope) = self.quota.try_lock()
            && let Some(scope) = scope.as_ref()
        {
            scope.headers(&headers)?;
        }
        if !out.status.success() {
            let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
            let said = format!("{}\n{}", String::from_utf8_lossy(&out.stderr).trim(), headers)
                .trim()
                .to_string();
            return Ok(Err((format!("gh {what}"), said)));
        }
        Ok(Ok(body))
    }

    /// One `gh` call that must succeed → its stdout. It is made once: what
    /// writes is not made again on a failure that may have written.
    pub(crate) fn raw(&self, args: &[&str]) -> Result<String> {
        match self.call(args)? {
            Ok(out) => Ok(out),
            Err((call, said)) => bail!("{call} failed: {said}"),
        }
    }

    pub fn gh(&self, args: &[&str]) -> Result<String> {
        self.routed(args, true)
    }

    /// Installs this look's quota scope from the note on disk, without asking
    /// `rate_limit`. A read that may refuse spends no round trip on the probe.
    /// A held login is still refused, and a primary-quota error still observes
    /// (`quota`, docs/adr/0043).
    pub(crate) fn note_without_probe(&self) -> Result<()> {
        if self.quota.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return Ok(());
        }
        let made = crate::gh_quota::Scope::of(self)?;
        let mut held = self.quota.lock().unwrap_or_else(|e| e.into_inner());
        if held.is_none() {
            *held = Some(made);
        }
        Ok(())
    }

    fn quota(&self) -> Result<crate::gh_quota::Scope> {
        let mut scope = self.quota.lock().unwrap_or_else(|e| e.into_inner());
        if scope.is_none() {
            let made = crate::gh_quota::Scope::of(self)?;
            let now = crate::gh_quota::now();
            let held = ["core", "graphql", "secondary"]
                .iter()
                .any(|name| made.read(name).is_ok_and(|r| r.remaining == Some(0) && r.reset > now));
            // Once in this look, and not while the note already says the
            // login is held. A second ask in the same look does nothing
            // (`observe`, docs/adr/0043).
            if !held {
                made.observe(self)?;
            }
            *scope = Some(made);
        }
        Ok(scope.as_ref().expect("scope was established").clone())
    }

    /// A quota rejection or an unexpired observation. Only this error type
    /// authorizes another backend; neither transport errors nor a generic 403 do.
    fn routed(&self, args: &[&str], writing: bool) -> Result<String> {
        let Some(api) = crate::gh_quota::Api::of(args) else { return self.raw(args) };
        let made = self.on_api(args, writing).and_then(|out| {
            if writing { Ok(out) } else { crate::gh_equivalent::complete_listing(self, args, out) }
        });
        match made {
            Err(error) if error.is::<PrimaryQuota>() => {
                let scope = self.quota()?;
                let why = scope.describe()?;
                eprintln!(
                    "GitHub API: {} primary quota exhausted; trying {} for gh {}; {why}",
                    api.name(),
                    api.other().name(),
                    args.iter().take(2).copied().collect::<Vec<_>>().join(" ")
                );
                let result = crate::gh_equivalent::run(self, args, writing).with_context(|| format!(
                    "GitHub API fallback for gh {} from {} to {}; {why}; retry after reset (Unix seconds)",
                    args.iter().take(2).copied().collect::<Vec<_>>().join(" "), api.name(), api.other().name()));
                self.log_api(args, api.other(), &why, result.as_ref().err());
                result
            }
            other => other,
        }
    }

    fn log_api(
        &self,
        args: &[&str],
        api: crate::gh_quota::Api,
        why: &str,
        error: Option<&anyhow::Error>,
    ) {
        let outcome = format!(
            "selected {} after primary exhaustion; {why}{}",
            api.name(),
            error.map(|e| format!("; {e:#}")).unwrap_or_default()
        );
        if let Ok(repo) = Repo::at(&self.dir)
            && let Ok(log) = crate::log::Log::of(&repo)
        {
            let entry = crate::log::Entry {
                at: crate::gh_quota::now(),
                instance: String::new(),
                host: String::new(),
                what: crate::log::What::Command {
                    command: "github-api".into(),
                    on: Some(args.iter().take(2).copied().collect::<Vec<_>>().join(" ")),
                    by: "quota routing".into(),
                    outcome,
                    failed: error.is_some(),
                },
            };
            if let Err(e) = log.append(&entry) {
                eprintln!("GitHub API selection log could not be written: {e:#}");
            }
        }
    }

    /// The quota note as it stands. A look that read GitHub already carried
    /// the response headers into it; this reports that note and does not ask
    /// `rate_limit` of its own (docs/adr/0043). An empty note is an answer.
    pub(crate) fn quota_status(&self) -> Result<serde_json::Value> {
        crate::gh_quota::Scope::of(self)?.json()
    }

    /// Executes one backend only. Compound equivalents use this entry so a
    /// failed alternate cannot bounce recursively to its unavailable original.
    pub(crate) fn on_api(&self, args: &[&str], writing: bool) -> Result<String> {
        let Some(api) = crate::gh_quota::Api::of(args) else { return self.raw(args) };
        let scope = self.quota()?;
        let now = crate::gh_quota::now();
        let secondary = scope.read("secondary")?;
        if secondary.reset > now {
            bail!(
                "GitHub secondary rate limit: both APIs wait until {} (Unix seconds); {}",
                secondary.reset,
                scope.describe()?
            );
        }
        let known = scope.read(api.name())?;
        if known.remaining == Some(0) && known.reset > now {
            return Err(PrimaryQuota(scope.describe()?).into());
        }
        let made = if writing { self.raw(args) } else { self.read_once_backend(args) };
        match made {
            Err(error) if crate::gh_quota::secondary(&error.to_string()).is_some() => {
                let delay = crate::gh_quota::secondary(&error.to_string()).unwrap_or(60);
                let reset = now.saturating_add(delay);
                scope.save(
                    "secondary",
                    &crate::gh_quota::Observation { remaining: Some(0), reset },
                )?;
                bail!(
                    "GitHub secondary rate limit: both APIs wait until {reset} (Unix seconds); {error}"
                );
            }
            Err(error) if crate::gh_quota::primary(&error.to_string()) => {
                scope.headers(&error.to_string())?;
                scope.observe(self)?;
                let api = if args.first() != Some(&"api")
                    && scope.read(api.name())?.remaining != Some(0)
                    && scope.read(api.other().name())?.remaining == Some(0)
                {
                    api.other()
                } else {
                    api
                };
                let known = scope.read(api.name())?;
                // An error is authoritative even if a later quota observation
                // already reports points. Bound a missing reset to one minute.
                let reset = if known.reset > now { known.reset } else { now.saturating_add(60) };
                scope.save(
                    api.name(),
                    &crate::gh_quota::Observation { remaining: Some(0), reset },
                )?;
                // High-level gh writes can contain several HTTP operations;
                // their error cannot prove that earlier writes did not happen.
                if writing && args.first() != Some(&"api") {
                    bail!(
                        "{error}; write outcome uncertain: inspect the authoritative issue/PR before retrying; {}",
                        scope.describe()?
                    );
                }
                Err(PrimaryQuota(scope.describe()?).into())
            }
            Err(error) if writing && in_transit(&error.to_string()) => {
                bail!(
                    "{error}; write outcome uncertain: inspect the authoritative issue/PR before retrying; no alternate write attempted"
                )
            }
            other => other,
        }
    }

    /// One `gh` call that only reads → its stdout. One that fails in transit
    /// (`in_transit`) is made again, `TRIES` times in all; none of them
    /// answered is `Unreached`. A failure GitHub answered with is as `gh`'s.
    pub fn read(&self, args: &[&str]) -> Result<String> {
        self.routed(args, false)
    }

    fn read_once_backend(&self, args: &[&str]) -> Result<String> {
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

    /// The GraphQL `query` of the tracker, page after page, each answer read
    /// with `jq`: `query` takes the tracker as `$owner` and `$repo`, and the
    /// repo label of another as `$labels`. `fields` are further `-f` values,
    /// such as `since=`.
    fn query_with(&self, query: &str, jq: &str, fields: &[String]) -> Result<String> {
        let query = format!("query={query}");
        let variables = variables(&self.tracker);
        let mut args = vec!["api", "graphql", "--paginate"];
        args.extend(variables.iter().map(String::as_str));
        for field in fields {
            args.extend(["-f", field]);
        }
        args.extend(["-f", &query, "--jq", jq]);
        self.read(&args)
    }

    fn query(&self, query: &str, jq: &str) -> Result<String> {
        self.query_with(query, jq, &[])
    }

    /// Who GitHub lets see the tracker, as it says it (`VISIBILITY_JQ`).
    pub fn visibility(&self) -> Result<String> {
        let endpoint = format!("repos/{}", path(&self.tracker));
        let key = self
            .tracker
            .named()
            .map(ToString::to_string)
            .or_else(|| {
                Repo::at(&self.dir)
                    .and_then(|repo| crate::checkout_list::named_repo(&repo))
                    .ok()
                    .map(|name| name.to_string())
            })
            .unwrap_or_else(|| self.dir.display().to_string());
        VISIBILITY
            .read(&key, || {
                self.read(&["api", &endpoint, "--jq", VISIBILITY_JQ])
                    .map(|s| s.trim().to_string())
                    .map_err(|e| format!("{e:#}"))
            })
            .map_err(anyhow::Error::msg)
    }

    /// The commit at the head of the default branch of `repo`, read once per
    /// repo in this look (`HEADS`). Another checkout asking the same repo
    /// waits on that read instead of starting another.
    pub fn head_commit(&self, repo: &RepoName) -> Result<String> {
        let stored = HEADS.read(repo.as_str(), || match self.read_head(repo) {
            Ok(sha) => Ok(sha),
            Err(error) => Err(match unreached(&error) {
                Some(miss) => HeadMiss::Unreached(miss.clone()),
                None => HeadMiss::Other(format!("{error:#}")),
            }),
        });
        match stored {
            Ok(sha) => Ok(sha),
            Err(HeadMiss::Unreached(miss)) => Err(miss.into()),
            Err(HeadMiss::Other(error)) => bail!("{error}"),
        }
    }

    fn read_head(&self, repo: &RepoName) -> Result<String> {
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
        let endpoint = format!("repos/{OWN}/branches/{branch}/protection");
        let stdout = match self.read(&["api", &endpoint]) {
            Ok(out) => out,
            Err(error) if error.to_string().contains("Branch not protected") => {
                return Guard::Unprotected;
            }
            Err(error) => return Guard::Unreadable(format!("{error:#}")),
        };
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
    /// An issue whose comments or blockers go past the embedded page is
    /// asked about again, for all of that connection.
    pub fn open_issues(&self) -> Result<OpenIssues> {
        let out = self.query(OPEN_ISSUES_QUERY, OPEN_ISSUES_JQ)?;
        let mut open = OpenIssues::default();
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: OpenRow =
                serde_json::from_str(line).context("gh api graphql: unexpected answer")?;
            self.take_open(&mut open, row)?;
        }
        Ok(open)
    }

    fn take_open(&self, open: &mut OpenIssues, row: OpenRow) -> Result<()> {
        // Another repo's issue in the tracker is none of the fleet's.
        if !self.tracker.holds(&row.labels) {
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
            open_blockers: self.blockers(blockers),
        });
        Ok(())
    }

    /// The open issues, and the patrol landings of the day ending at `now`,
    /// from one issues query. `None` for the landings when the closed page
    /// may be short of the day: the caller then reads them on their own pages.
    fn open_issues_and_landings(&self, now: u64) -> Result<OpenAndLanded> {
        let since = now.saturating_sub(afkconvoy_core::patrol::DAY);
        let since = format!("since={}", crate::log::stamp(since));
        let out = self.query_with(OPEN_AND_LANDED_QUERY, OPEN_AND_LANDED_JQ, &[since])?;
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
            return Ok(OpenAndLanded { issues: open, landings: None });
        }
        let mut seen = BTreeSet::new();
        let mut landed = vec![];
        for row in landed_rows {
            if !seen.insert(row.number) {
                continue;
            }
            landed.extend(self.take_landing(&row, now)?);
        }
        Ok(OpenAndLanded { issues: open, landings: Some(landed) })
    }

    /// The issues that may hold a closing or a stage/intake record since
    /// `since`. This uses GitHub's update time, not a search index, and pages
    /// through every result without a search result cap.
    pub fn watching_issues(&self, since: u64) -> Result<Vec<WatchIssue>> {
        let query = format!("query={WATCH_ISSUES_QUERY}");
        let since = format!("since={}", crate::log::stamp(since));
        let variables = variables(&self.tracker);
        let mut args = vec!["api", "graphql", "--paginate"];
        args.extend(variables.iter().map(String::as_str));
        args.extend(["-f", &query, "-f", &since, "--jq", WATCH_ISSUES_JQ]);
        let out = self.read(&args)?;
        let mut issues = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let mut issue: WatchIssue = serde_json::from_str(line)
                .context("gh api graphql: unexpected watching issues answer")?;
            if !self.tracker.holds(&issue.labels)
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

    /// Read every issue of this tracker, including the other code repos.
    pub fn account_issues(&self) -> Result<Vec<AccountIssue>> {
        let query = format!("query={REWRITE_ACCOUNTS_QUERY}");
        let variables = variables(&self.tracker);
        let mut args = vec!["api", "graphql", "--paginate"];
        args.extend(variables.iter().take(4).map(String::as_str));
        args.extend(["-f", &query, "--jq", REWRITE_ACCOUNTS_JQ]);
        let out = self.read(&args)?;
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

    /// Accounts on this repo's issues. A failed issue-comment read is data
    /// for the report; a failed issue listing fails the command as a whole.
    pub fn accounts(&self) -> Result<Vec<afkconvoy_core::accounts::Issue>> {
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
        let out = self.query(ACCOUNTS_QUERY, ACCOUNTS_JQ)?;
        let mut issues = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row = serde_json::from_str(line).context("gh api: invalid accounts answer")?;
            if !self.tracker.holds(&row.labels)
                || afkconvoy_core::tracker::repo_labels(&row.labels).len() > 1
            {
                continue;
            }
            let comments = if row.more { self.comments(row.number) } else { Ok(row.comments) };
            let account = comments.map_err(|error| format!("{error:#}")).and_then(|comments| {
                let mut accounts = comments.into_iter().filter(|comment| {
                    // Include a damaged opening marker so it is not silently lost.
                    comment.body.contains("<!--convoy:account ")
                        || comment.body.contains("<!--convoy:account-->")
                });
                let body = accounts.next().map(|comment| comment.body);
                if accounts.next().is_some() {
                    return Err("issue has several work accounts".into());
                }
                Ok(body)
            });
            issues.push(afkconvoy_core::accounts::Issue {
                number: row.number,
                closed: row.closed,
                url: row.url,
                account,
            });
        }
        Ok(issues)
    }

    /// Recently updated PRs in the checkout's own repo, including ones
    /// which disappeared from the open list between looks. Forks are not
    /// fleet state, but still count toward pagination's lower boundary.
    pub fn watching_prs(&self, since: u64) -> Result<Vec<WatchPr>> {
        let mut prs = vec![];
        for page in 1.. {
            let endpoint = format!(
                "repos/{OWN}/pulls?state=all&sort=updated&direction=desc&per_page=100&page={page}"
            );
            let out = self.read(&["api", &endpoint, "--jq", WATCH_PRS_JQ])?;
            let rows: Vec<WatchPr> = out
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str(line).context("gh api: unexpected watching PRs answer")
                })
                .collect::<Result<_>>()?;
            let finished = rows.len() < 100 || rows.iter().any(|row| row.updated < since);
            prs.extend(rows.into_iter().filter(|row| {
                row.updated >= since
                    && !row.fork
                    && !crate::delivery::final_pr(
                        &row.head_branch,
                        row.body.as_deref().unwrap_or(""),
                    )
            }));
            if finished {
                break;
            }
        }
        Ok(prs)
    }

    /// Patrol ownership and completion times in the rolling day. Read only
    /// when a configured patrol has a landing bound. Ownership follows the
    /// latest intake record, regardless of the issue's labels now.
    pub fn patrol_landings(&self, now: u64) -> Result<Vec<(String, u64)>> {
        let out = self.query(LANDED_ISSUES_QUERY, LANDED_ISSUES_JQ)?;
        let mut landed = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: LandingRow =
                serde_json::from_str(line).context("gh api graphql: unexpected answer")?;
            landed.extend(self.take_landing(&row, now)?);
        }
        Ok(landed)
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

    /// What is open: the issues and the PRs, asked for side by side.
    pub fn open(&self) -> Result<Open> {
        Ok(self.open_with_landings(None)?.open)
    }

    /// [`open`], and when `now` is set, the patrol landings of the day ending
    /// then, from the same issues query. `None` for the landings means they
    /// were not asked, or the closed page may be short of the day.
    pub(crate) fn open_with_landings(&self, now: Option<u64>) -> Result<Opened> {
        let (read, prs) = thread::scope(|scope| {
            let prs = scope.spawn(|| self.open_prs());
            let read = match now {
                None => self.open_issues().map(|issues| OpenAndLanded { issues, landings: None }),
                Some(now) => self.open_issues_and_landings(now),
            };
            (read, beside(prs))
        });
        let read = read?;
        Ok(Opened { open: Open { issues: read.issues, prs: prs? }, landings: read.landings })
    }

    /// Every closed issue that carries `label`, each with when it closed, in
    /// epoch seconds — but for one GitHub gives no time for.
    pub fn closed_with(&self, label: &str) -> Result<Vec<(u64, u64)>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            at: Option<u64>,
        }
        let labels = carrying_labels(&self.tracker, label);
        let endpoint =
            endpoint(&self.tracker, &format!("issues?state=closed&labels={labels}&per_page=100"));
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
        let labels = carrying_labels(&self.tracker, label);
        let endpoint =
            endpoint(&self.tracker, &format!("issues?state=all&labels={labels}&per_page=100"));
        let out = self.read(&["api", "--paginate", &endpoint, "--jq", CARRYING_JQ])?;
        let mut carrying = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            carrying.push((row.number, (!row.open).then_some(row.closed_at)));
        }
        Ok(carrying)
    }

    /// Open issues carrying `label` in this fleet's tracker scope, lowest
    /// number first. Lock checks need only this REST listing, not a rebuild
    /// with every comment and blocker, and remain available without GraphQL.
    pub fn open_with(&self, label: &str) -> Result<Vec<u64>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            labels: Vec<String>,
        }
        let labels = carrying_labels(&self.tracker, label);
        let endpoint =
            endpoint(&self.tracker, &format!("issues?state=open&labels={labels}&per_page=100"));
        let out = self.read(&["api", "--paginate", &endpoint, "--jq", OPEN_LABELLED_JQ])?;
        let mut numbers = vec![];
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row =
                serde_json::from_str(line).context("gh api issues: unexpected answer")?;
            if self.tracker.holds(&row.labels) && row.labels.iter().any(|held| held == label) {
                numbers.push(row.number);
            }
        }
        numbers.sort_unstable();
        Ok(numbers)
    }

    /// The same-repository open PR headed by `branch`, for a turn on no
    /// issue. REST supplies its head without GraphQL quota. `closes` is empty:
    /// callers associating a PR with an issue must use `open_prs` instead.
    pub fn pr_of_branch(&self, branch: &str) -> Result<Option<PullRequest>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            title: String,
            body: String,
            head_branch: String,
            head_sha: String,
            base_branch: String,
        }
        let endpoint = format!("repos/{OWN}/pulls?state=open&per_page=100");
        let out = self.read(&["api", "--paginate", &endpoint, "--jq", PR_HEADS_JQ])?;
        for line in out.lines().filter(|line| !line.trim().is_empty()) {
            let row: Row = serde_json::from_str(line).context("gh api pulls: unexpected answer")?;
            if row.head_branch == branch && !crate::delivery::final_pr(&row.head_branch, &row.body)
            {
                return Ok(Some(PullRequest {
                    number: row.number,
                    title: row.title,
                    head_branch: row.head_branch,
                    head_sha: row.head_sha,
                    base_branch: row.base_branch,
                    closes: vec![],
                }));
            }
        }
        Ok(None)
    }

    /// Every open pull request whose head is a branch of the repo itself. One
    /// from a fork is anyone's and no part of fleet state (GLOSSARY: Fleet
    /// state): it is dropped here, so that nothing of the fleet reads it.
    /// The issues each closes are the tracker's (`Tracker::closed_by`).
    pub fn open_prs(&self) -> Result<Vec<PullRequest>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Row {
            number: u64,
            title: Option<String>,
            head_ref_name: String,
            head_ref_oid: String,
            base_ref_name: String,
            body: Option<String>,
            closing_issues_references: Vec<Link>,
            is_cross_repository: bool,
            head_repository: Option<Named>,
            head_repository_owner: Option<Owner>,
        }
        #[derive(Deserialize)]
        struct Link {
            number: u64,
            repository: Option<Repository>,
        }
        #[derive(Deserialize)]
        struct Repository {
            name: String,
            owner: Owner,
        }
        #[derive(Deserialize)]
        struct Named {
            name: String,
        }
        #[derive(Deserialize)]
        struct Owner {
            login: String,
        }
        let out =
            self.read(&["pr", "list", "--state", "open", "--limit", "1000", "--json", PR_FIELDS])?;
        let rows: Vec<Row> = serde_json::from_str(&out).context("gh pr list: unexpected answer")?;
        Ok(rows
            .into_iter()
            .filter(|row| {
                !row.is_cross_repository
                    && !crate::delivery::final_pr(
                        &row.head_ref_name,
                        row.body.as_deref().unwrap_or(""),
                    )
            })
            .map(|row| {
                let linked: Vec<Linked> = row
                    .closing_issues_references
                    .into_iter()
                    .map(|link| Linked {
                        number: link.number,
                        repo: link.repository.map(|of| format!("{}/{}", of.owner.login, of.name)),
                    })
                    .collect();
                let of = match (row.head_repository_owner, row.head_repository) {
                    (Some(owner), Some(repo)) => Some(format!("{}/{}", owner.login, repo.name)),
                    _ => None,
                };
                let body = row.body.as_deref().unwrap_or("");
                PullRequest {
                    number: row.number,
                    title: row.title.unwrap_or_default(),
                    head_branch: row.head_ref_name,
                    head_sha: row.head_ref_oid,
                    base_branch: row.base_ref_name,
                    closes: self.tracker.closed_by(body, &linked, of.as_deref()),
                }
            })
            .collect())
    }

    /// One issue, open or closed. An error when it cannot be read: `Gone`
    /// when GitHub says it was deleted or transferred, or has no such issue.
    pub fn issue(&self, number: u64) -> Result<IssueRead> {
        let endpoint = endpoint(&self.tracker, &format!("issues/{number}"));
        let out = answered(number, self.read(&["api", &endpoint, "--jq", ISSUE_JQ]))?;
        let issue: IssueRead = serde_json::from_str(&out)
            .with_context(|| format!("gh api issue {number}: unexpected answer"))?;
        if let Some(to) = self.moved(number, &issue) {
            return Err(Gone { issue: number, how: Went::Transferred { to: Some(to) } }.into());
        }
        if issue.is_closed() {
            self.saw_closed(number);
        }
        Ok(issue)
    }

    /// Where issue `number` is, `owner/repo#n`, when its read `issue` is of
    /// another repo or number: GitHub redirected the read because it moved.
    fn moved(&self, number: u64, issue: &IssueRead) -> Option<String> {
        let (repo, at) = located(issue.url.as_deref()?)?;
        let here = match self.tracker.named() {
            Some(named) => Some(named.to_string()),
            None => self.own().map(str::to_string),
        };
        let elsewhere = here.is_some_and(|here| !here.eq_ignore_ascii_case(&repo));
        (at != number || elsewhere).then(|| format!("{repo}#{at}"))
    }

    /// The checkout's own repo, `owner/repo`, where its remote names one.
    fn own(&self) -> Option<&str> {
        self.own
            .get_or_init(|| {
                let repo = Repo::new(self.dir.clone());
                crate::checkout_list::named_repo(&repo).ok().map(|named| named.to_string())
            })
            .as_deref()
    }

    /// `made`, a call about issue `number`, with a failure GitHub answered
    /// 410 raised as `Gone` deleted. A 404 says no more than that the call
    /// found nothing, so the issue is read: `Gone` as that read says —
    /// transferred where GitHub redirects it, else missing — and the failure
    /// as it was of one GitHub still has here.
    fn about_issue<T>(&self, number: u64, made: Result<T>) -> Result<T> {
        made.map_err(|error| match status(&format!("{error}")) {
            Some(410) => Gone { issue: number, how: Went::Deleted }.into(),
            Some(404) => match self.issue(number) {
                Err(read) if gone(&read).is_some() => read,
                _ => error,
            },
            _ => error,
        })
    }

    /// Every comment on an issue, oldest first: `Gone` of one GitHub says was
    /// deleted or transferred.
    pub fn comments(&self, number: u64) -> Result<Vec<Comment>> {
        self.comments_in(path(&self.tracker), number)
    }

    /// Every comment on a PR, oldest first.
    pub fn pr_comments(&self, pr: u64) -> Result<Vec<Comment>> {
        self.comments_in(OWN, pr)
    }

    /// Every comment on the issue or the PR `number` of `repo`, oldest first.
    fn comments_in(&self, repo: &str, number: u64) -> Result<Vec<Comment>> {
        let endpoint = format!("repos/{repo}/issues/{number}/comments?per_page=100");
        let read = self.read(&["api", "--paginate", &endpoint, "--jq", COMMENTS_JQ]);
        let out = self.about_issue(number, read)?;
        out.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("gh api comments: unexpected answer"))
            .collect()
    }

    /// Posts one comment on an issue: `Gone` of one GitHub says was deleted
    /// or transferred.
    pub fn comment(&self, number: u64, body: &str) -> Result<()> {
        self.comment_in(path(&self.tracker), number, body)
    }

    /// Posts one comment on a PR.
    pub fn comment_on_pr(&self, pr: u64, body: &str) -> Result<()> {
        self.comment_in(OWN, pr, body)
    }

    /// Posts one comment on the issue or the PR `number` of `repo`.
    fn comment_in(&self, repo: &str, number: u64, body: &str) -> Result<()> {
        let endpoint = format!("repos/{repo}/issues/{number}/comments");
        let body = format!("body={body}");
        self.about_issue(number, self.gh(&["api", "--method", "POST", &endpoint, "-f", &body]))?;
        Ok(())
    }

    /// Opens a failure report and leaves its reason in a comment, so every
    /// wait can link directly to the explanation. If commenting fails, the
    /// report's body still explains the wait on the next rebuild.
    pub fn open_report(
        &self,
        title: &str,
        body: &str,
        reason: &str,
        labels: &[&str],
    ) -> Result<u64> {
        let issue = self.open_issue(title, body, labels)?;
        let marker = body.lines().next().unwrap_or("");
        self.comment(issue, &format!("{marker}\n{reason}"))?;
        Ok(issue)
    }

    /// Replaces the body of one comment on an issue.
    pub fn edit_comment(&self, id: u64, body: &str) -> Result<()> {
        self.edit_comment_in(path(&self.tracker), id, body)
    }

    /// Replaces the body of one comment on a PR.
    pub fn edit_pr_comment(&self, id: u64, body: &str) -> Result<()> {
        self.edit_comment_in(OWN, id, body)
    }

    /// Replaces the body of one comment of `repo`, on an issue or a PR.
    fn edit_comment_in(&self, repo: &str, id: u64, body: &str) -> Result<()> {
        let endpoint = format!("repos/{repo}/issues/comments/{id}");
        let body = format!("body={body}");
        self.gh(&["api", "--method", "PATCH", &endpoint, "-f", &body, "--silent"])?;
        Ok(())
    }

    /// Opens an issue carrying `labels` → its number. In a tracker that is
    /// not the checkout's own repo it carries the repo label too: it is the
    /// work of the checkout's repo.
    pub fn open_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64> {
        let (title, body) = (format!("title={title}"), format!("body={body}"));
        let repo_label = self.tracker.repo_label();
        let labels = labels.iter().copied().chain(repo_label.as_deref());
        let labels: Vec<String> = labels.map(|label| format!("labels[]={label}")).collect();
        let endpoint = endpoint(&self.tracker, "issues");
        let mut args = vec!["api", "--method", "POST", &endpoint, "-f", &title, "-f", &body];
        for label in &labels {
            args.extend(["-f", label.as_str()]);
        }
        args.extend(["--jq", ".number"]);
        let out = self.gh(&args)?;
        out.trim().parse().with_context(|| format!("gh api issues: no number in {out:?}"))
    }

    /// Closes one issue as completed. A close GitHub refuses while it closes
    /// the issue itself — a merged PR that says `Closes` it — is a close: what
    /// is asked is that the issue be closed (sunfmin/control#247).
    pub fn close_issue(&self, number: u64) -> Result<()> {
        let endpoint = endpoint(&self.tracker, &format!("issues/{number}"));
        let closed =
            self.gh(&["api", "--method", "PATCH", &endpoint, "-f", "state=closed", "--silent"]);
        if let Err(error) = closed
            && !self.issue(number).is_ok_and(|issue| issue.is_closed())
        {
            return Err(error);
        }
        self.saw_closed(number);
        Ok(())
    }

    /// The PRs that merged carrying `label`, by number: none where no PR, or
    /// no label, is so.
    pub fn merged_prs_labelled(&self, label: &str) -> Result<Vec<u64>> {
        let out = self.read(&[
            "pr",
            "list",
            "--state",
            "merged",
            "--label",
            label,
            "--limit",
            "1000",
            "--json",
            "number",
            "--jq",
            ".[].number",
        ])?;
        Ok(out.lines().filter_map(|line| line.trim().parse().ok()).collect())
    }

    /// Opens a PR of the branch `head` onto `base` → its number.
    pub fn open_pr(&self, base: &str, head: &str, title: &str, body: &str) -> Result<u64> {
        let args =
            ["pr", "create", "--base", base, "--head", head, "--title", title, "--body", body];
        let out = self.gh(&args)?;
        // `gh` says the PR's URL, which ends in its number.
        let number = out.trim().rsplit('/').next().and_then(|number| number.parse().ok());
        number.with_context(|| format!("gh pr create: no PR number in {out:?}"))
    }

    pub fn repository(&self) -> Result<String> {
        self.read(&["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
            .map(|name| name.trim().to_string())
    }

    pub fn delivery_prs(&self) -> Result<Vec<DeliveryPr>> {
        let endpoint = format!("repos/{OWN}/pulls?state=all&per_page=100");
        let out = self.read(&["api", "--paginate", &endpoint, "--jq", DELIVERY_PRS_JQ])?;
        out.lines()
            .filter(|s| !s.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).context("gh api: unreadable delivery PR history")
            })
            .collect()
    }

    pub fn open_draft_pr(&self, base: &str, head: &str, title: &str, body: &str) -> Result<()> {
        self.gh(&[
            "pr", "create", "--draft", "--base", base, "--head", head, "--title", title, "--body",
            body,
        ])?;
        Ok(())
    }

    /// Replaces one PR's body. It does not open a PR and does not change reviews.
    pub fn edit_pr_body(&self, number: u64, body: &str) -> Result<()> {
        self.gh(&["pr", "edit", &number.to_string(), "--body", body])?;
        Ok(())
    }

    /// Marks one open PR ready for review, or returns it to draft with `--undo`.
    /// Neither call dismisses reviews.
    pub fn set_draft(&self, number: u64, draft: bool) -> Result<()> {
        let number = number.to_string();
        if draft {
            self.gh(&["pr", "ready", &number, "--undo"])?;
        } else {
            self.gh(&["pr", "ready", &number])?;
        }
        Ok(())
    }

    /// Closes one PR without merging it, with a comment saying why.
    pub fn close_pr(&self, number: u64, comment: &str) -> Result<()> {
        self.gh(&["pr", "close", &number.to_string(), "--comment", comment])?;
        Ok(())
    }

    /// The open PRs of another repo, `repo`, from a branch of its own — as
    /// `open_prs` reads this one's: its number, head branch and creation time.
    pub fn open_prs_of(&self, repo: &RepoName) -> Result<Vec<OtherPr>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Row {
            #[serde(flatten)]
            pr: OtherPr,
            is_cross_repository: bool,
        }
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
            PR_OF_FIELDS,
            "--jq",
            PR_OF_JQ,
        ])?;
        let rows: Vec<Row> = serde_json::from_str(&out).context("gh pr list: unexpected answer")?;
        let own = rows.into_iter().filter(|row| !row.is_cross_repository);
        Ok(own.map(|row| row.pr).collect())
    }

    /// The open issues of another repo, `repo`: the number and the body of
    /// each.
    pub fn open_issues_of(&self, repo: &RepoName) -> Result<Vec<(u64, String)>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            #[serde(default)]
            body: String,
        }
        let out = self.read(&[
            "issue",
            "list",
            "--repo",
            repo.as_str(),
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            "number,body",
        ])?;
        let rows: Vec<Row> =
            serde_json::from_str(&out).context("gh issue list: unexpected answer")?;
        Ok(rows.into_iter().map(|row| (row.number, row.body)).collect())
    }

    /// Opens an issue in another repo, `repo`, carrying `label`.
    pub fn open_issue_of(
        &self,
        repo: &RepoName,
        title: &str,
        body: &str,
        label: &str,
    ) -> Result<()> {
        let args = ["issue", "create", "--repo", repo.as_str(), "--title", title, "--body", body];
        self.gh(&[&args[..], &["--label", label]].concat())?;
        Ok(())
    }

    /// The file at `path` on the default branch of another repo, `repo`;
    /// `None` where it has none there.
    pub fn file_of(&self, repo: &RepoName, path: &str) -> Result<Option<String>> {
        let endpoint = format!("repos/{}/contents/{path}", repo.as_str());
        match self.read(&["api", "-H", "Accept: application/vnd.github.raw", &endpoint]) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.to_string().contains("HTTP 404") => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The issues GitHub records `number` as blocked by, open or closed:
    /// `Gone` when `number` was deleted or transferred.
    pub fn blocked_by(&self, number: u64) -> Result<Vec<BlockedBy>> {
        let endpoint = endpoint(
            &self.tracker,
            &format!("issues/{number}/dependencies/blocked_by?per_page=100"),
        );
        let read = self.read(&["api", "--paginate", &endpoint, "--jq", BLOCKED_BY_JQ]);
        let out = self.about_issue(number, read)?;
        out.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("gh api blocked_by: unexpected answer"))
            .collect()
    }

    /// The open issues among those `number` is blocked by.
    pub fn open_blockers(&self, number: u64) -> Result<Vec<u64>> {
        Ok(self.blockers(self.blocked_by(number)?))
    }

    /// Records `number` as blocked by the issue whose database id is `blocker_id`.
    pub fn add_blocker(&self, number: u64, blocker_id: u64) -> Result<()> {
        let endpoint = endpoint(&self.tracker, &format!("issues/{number}/dependencies/blocked_by"));
        let field = format!("issue_id={blocker_id}");
        self.gh(&["api", "--method", "POST", &endpoint, "-F", &field, "--silent"])?;
        Ok(())
    }

    /// Every label the repo has, by name.
    pub fn labels(&self) -> Result<Vec<String>> {
        let endpoint = endpoint(&self.tracker, "labels?per_page=100");
        let out = self.read(&["api", "--paginate", &endpoint, "--jq", LABELS_JQ])?;
        Ok(out.lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
    }

    /// Creates the label `name` on the repo, which has none of that name, to
    /// be shown with `description`.
    pub fn create_label(&self, name: &str, description: &str) -> Result<()> {
        let (name, description) = (format!("name={name}"), format!("description={description}"));
        let endpoint = endpoint(&self.tracker, "labels");
        self.gh(&[
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

    /// Puts one label on an issue; GitHub creates a label the repo lacks.
    pub fn add_label(&self, number: u64, label: &str) -> Result<()> {
        let endpoint = endpoint(&self.tracker, &format!("issues/{number}/labels"));
        let field = format!("labels[]={label}");
        self.gh(&["api", "--method", "POST", &endpoint, "-f", &field, "--silent"])?;
        Ok(())
    }

    /// Makes issue `number`, carrying `labels`, the repo's in its tracker
    /// before a command hands it to the fleet (`Tracker::to_take`): puts the
    /// repo label on it, in a tracker that is another repo, where it carries
    /// none. An error of one that is another repo's, which is left as it is.
    pub fn take(&self, number: u64, labels: &[String]) -> Result<()> {
        let issue = self.tracker.issue(number);
        match self.tracker.to_take(labels) {
            Ok(Some(label)) => self.add_label(number, &label),
            Ok(None) => Ok(()),
            Err(why) => bail!("issue {issue} {why}"),
        }
    }

    /// Takes one label off an issue that carries it.
    pub fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let label = crate::judge::label_segment(label);
        let endpoint = endpoint(&self.tracker, &format!("issues/{number}/labels/{label}"));
        self.gh(&["api", "--method", "DELETE", &endpoint, "--silent"])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_landed_page_adds_no_second_end_cursor() {
        let open = OPEN_ISSUES_QUERY.matches("endCursor").count();
        assert_eq!(OPEN_AND_LANDED_QUERY.matches("endCursor").count(), open);
        let landed = OPEN_AND_LANDED_QUERY.split_once("landed:").unwrap().1;
        assert!(!landed.contains("endCursor"), "{landed}");
        assert!(OPEN_AND_LANDED_QUERY.contains("first: 100"));
        assert_eq!(LANDED_PAGE, 100);
        let nodes = "nodes { number title body url createdAt lastEditedAt labels(first: 100) { nodes { name } }";
        assert!(OPEN_ISSUES_QUERY.contains(nodes));
        assert!(OPEN_AND_LANDED_QUERY.contains(nodes));
    }

    #[test]
    fn the_tracker_is_named_by_gh_filling_the_checkouts_repo_in_or_by_its_name() {
        let own = Tracker::Own;
        assert_eq!(endpoint(&own, "issues/7"), "repos/{owner}/{repo}/issues/7");
        assert_eq!(variables(&own), ["-F", "owner={owner}", "-F", "repo={repo}"]);
        assert_eq!(carrying_labels(&own, "slow gate"), "slow%20gate");
        // a name is sent as it is: `-F` would send `owner=123` as a number
        let other = Tracker::Other {
            tracker: RepoName::read("123/tracker").unwrap(),
            repo: RepoName::read("acme/dev").unwrap(),
        };
        assert_eq!(endpoint(&other, "issues/7"), "repos/123/tracker/issues/7");
        assert_eq!(
            variables(&other),
            ["-f", "owner=123", "-f", "repo=tracker", "-f", "labels[]=repo:acme/dev"]
        );
        assert_eq!(carrying_labels(&other, "slow-gate"), "slow-gate,repo%3Aacme%2Fdev");
    }

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
    fn an_issue_read_answered_with_410_is_deleted_and_with_404_is_no_such_issue() {
        let failed = |said: &str| -> Result<()> { bail!("gh api failed: {said}") };
        let deleted = answered(231, failed("gh: This issue was deleted (HTTP 410)"));
        let error = deleted.unwrap_err().context("the claim cannot be read");
        assert_eq!(gone(&error), Some(&Gone { issue: 231, how: Went::Deleted }));
        let missing = answered(7, failed("gh: Not Found (HTTP 404)")).unwrap_err();
        assert_eq!(gone(&missing), Some(&Gone { issue: 7, how: Went::Missing }));
        assert_eq!(missing.to_string(), "no such issue: #7 (HTTP 404)");
        for said in ["gh: Resource not accessible by integration (HTTP 403)", "gh: no"] {
            let error = answered(7, failed(said)).unwrap_err();
            assert_eq!((gone(&error), error.to_string()), (None, format!("gh api failed: {said}")));
        }
        assert_eq!(Gone { issue: 231, how: Went::Deleted }.said(), "the issue was deleted");
        assert_eq!(Gone { issue: 7, how: Went::Missing }.said(), "there is no such issue");
    }

    #[test]
    fn an_issue_is_transferred_only_where_its_read_was_redirected_elsewhere() {
        let gh = GitHub::new(
            PathBuf::from("/nowhere"),
            Tracker::Other {
                tracker: RepoName::read("sunfmin/hkj").unwrap(),
                repo: RepoName::read("sunfmin/app").unwrap(),
            },
        );
        let read = |url: Option<&str>| IssueRead {
            id: 1,
            title: String::new(),
            state: "open".to_string(),
            state_reason: None,
            labels: vec![],
            pull_request: false,
            closed_by: None,
            body: None,
            url: url.map(str::to_string),
        };
        let here = read(Some("https://api.github.com/repos/sunfmin/hkj/issues/22"));
        assert_eq!(gh.moved(22, &here), None);
        assert_eq!(gh.moved(22, &read(None)), None);
        let moved = read(Some("https://api.github.com/repos/sunfmin/control/issues/2"));
        assert_eq!(gh.moved(22, &moved), Some("sunfmin/control#2".to_string()));
        let same_number = read(Some("https://api.github.com/repos/sunfmin/control/issues/22"));
        assert_eq!(gh.moved(22, &same_number), Some("sunfmin/control#22".to_string()));
        let gone =
            Gone { issue: 22, how: Went::Transferred { to: Some("sunfmin/control#2".into()) } };
        assert_eq!(gone.to_string(), "issue #22 was transferred to sunfmin/control#2");
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

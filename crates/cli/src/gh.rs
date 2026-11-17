//! GitHub, through `gh` as a subprocess run in the checkout. Its PRs and the
//! protection of its branches are the checkout's repo's, which `gh` works out
//! from the checkout, so nothing here names it. The issues of a tracker that
//! is a GitHub repo are read and written through the GitHub adapter of the
//! fleet's issues (`issues::GitHubIssues`, the seam `crate::issues`), which
//! makes its calls through this; the queries it makes are here, beside the
//! others.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::layout::{Guard, Protection};
use afkconvoy_core::plan::PullRequest;
use afkconvoy_core::tracker::Linked;
pub use afkconvoy_core::tracker::Tracker;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::git::Repo;
use crate::issues::{Comment, Gone, IssueRead, Went, gone};
use crate::look::Reading;

pub(crate) mod issues;

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
/// hand the host's temporary directory (`in_no_checkout`). The issues of
/// `tracker` are read through `crate::issues::of`; the PRs a PR read finds
/// name the issues of it they close.
#[derive(Clone)]
pub struct GitHub {
    pub dir: PathBuf,
    pub tracker: Tracker,
    /// What the issue reads and writes of this view saw, shared with its
    /// clones and so with every adapter made of one of them.
    seen: Arc<Mutex<Seen>>,
    quota: Arc<Mutex<Option<crate::gh_quota::Scope>>>,
    /// The checkout's own repo, as its remote names it, where it does:
    /// asked once.
    own: Arc<OnceLock<Option<String>>>,
}

/// What a view's issue calls saw (`GitHub::seen`).
#[derive(Default)]
struct Seen {
    /// Closures read or made. These only route wakes; every woken tick reads
    /// GitHub for itself.
    closed: BTreeSet<u64>,
    /// issue → its database id, from a read of it: what an edge that names
    /// it as a blocker is written with (`GitHubIssues::add_blocker`).
    ids: HashMap<u64, u64>,
}

/// One issue as GitHub's REST read says it: its database id and API URL
/// beside what any tracker says of it.
#[derive(Deserialize)]
struct IssueRow {
    /// What a dependency edge names an issue by.
    id: u64,
    /// GitHub's API URL of it: of another repo, or another number, where
    /// GitHub redirected the read because the issue moved.
    #[serde(default)]
    url: Option<String>,
    #[serde(flatten)]
    read: IssueRead,
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
        GitHub { dir, tracker, seen: Arc::default(), quota: Arc::default(), own: Arc::default() }
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn saw_closed(&self, number: u64) {
        self.seen().closed.insert(number);
    }

    /// `gh` for the calls that each name their repo, where no checkout is at
    /// hand: they need none, and are made in the host's temporary directory.
    pub fn in_no_checkout() -> GitHub {
        GitHub::new(std::env::temp_dir(), Tracker::Own)
    }
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
/// a 5xx, or no status and a marker of `transient::GH`. Any other status — a
/// 404, a 403 — is GitHub's answer, and asking again gets the same.
fn in_transit(said: &str) -> bool {
    if let Some(status) = status(said) {
        return status / 100 == 5;
    }
    crate::transient::met(crate::transient::GH, said)
}

/// How the help names the failures `in_transit` finds.
pub(crate) fn in_transit_help() -> String {
    format!(
        "`gh` said an HTTP 5xx, or named no HTTP status and said {}, in upper or lower case",
        crate::transient::listed(crate::transient::GH)
    )
}

impl GitHub {
    /// One `gh` call → its stdout, or what it is named by and what `gh` said
    /// of its failing.
    fn call(&self, args: &[&str]) -> Result<Result<String, (String, String)>> {
        let mut command = crate::census::command("gh");
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
        if let Err(error) = &made
            && let Some(reset) = crate::gh_quota::secondary(&error.to_string(), now)
        {
            scope.save(
                "secondary",
                &crate::gh_quota::Observation { remaining: Some(0), reset, limit: None },
            )?;
            bail!(
                "GitHub secondary rate limit: both APIs wait until {reset} (Unix seconds); {error}"
            );
        }
        match made {
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
                    &crate::gh_quota::Observation { remaining: Some(0), reset, limit: None },
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

    /// One issue of the tracker, open or closed. An error when it cannot be
    /// read: `Gone` when GitHub says it was deleted or transferred, or has no
    /// such issue.
    fn read_issue(&self, number: u64) -> Result<IssueRow> {
        let endpoint = endpoint(&self.tracker, &format!("issues/{number}"));
        let out = answered(number, self.read(&["api", &endpoint, "--jq", ISSUE_JQ]))?;
        let issue: IssueRow = serde_json::from_str(&out)
            .with_context(|| format!("gh api issue {number}: unexpected answer"))?;
        if let Some(to) = self.moved(number, issue.url.as_deref()) {
            return Err(Gone { issue: number, how: Went::Transferred { to: Some(to) } }.into());
        }
        let mut seen = self.seen();
        if issue.read.is_closed() {
            seen.closed.insert(number);
        }
        seen.ids.insert(number, issue.id);
        drop(seen);
        Ok(issue)
    }

    /// Where issue `number` is, `owner/repo#n`, when its read, GitHub's API
    /// `url` of it, is of another repo or number: GitHub redirected the read
    /// because it moved.
    fn moved(&self, number: u64, url: Option<&str>) -> Option<String> {
        let (repo, at) = located(url?)?;
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
            Some(404) => match self.read_issue(number) {
                Err(read) if gone(&read).is_some() => read,
                _ => error,
            },
            _ => error,
        })
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

    /// One PR's description, open or closed.
    pub fn pr_body(&self, number: u64) -> Result<String> {
        self.read(&["pr", "view", &number.to_string(), "--json", "body", "--jq", ".body"])
    }

    /// One PR's title, open or closed.
    pub fn pr_title(&self, number: u64) -> Result<String> {
        self.read(&["pr", "view", &number.to_string(), "--json", "title", "--jq", ".title"])
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
    fn a_read_that_said_any_marker_of_its_list_failed_in_transit() {
        let said = [
            ("unexpected end of JSON input", "unexpected end of JSON input"),
            ("unexpected EOF", "Post \"https://api.github.com/graphql\": unexpected EOF"),
            (": EOF", "Post \"https://api.github.com/graphql\": EOF"),
            (
                "connection reset",
                "Get \"https://api.github.com/repos/a/b\": read tcp: connection reset by peer",
            ),
            ("timeout", "Post \"https://api.github.com/graphql\": net/http: TLS handshake timeout"),
            ("timed out", "Get \"https://api.github.com/repos/a/b\": dial tcp: i/o timed out"),
        ];
        let markers: Vec<&str> = crate::transient::GH.iter().map(|marker| marker.said).collect();
        let named: Vec<&str> = said.iter().map(|(marker, _)| *marker).collect();
        assert_eq!(named, markers, "a marker with no example, or one out of order");
        for (marker, said) in said {
            assert!(in_transit(said), "{marker}: {said}");
        }
    }

    #[test]
    fn the_help_names_every_marker_of_a_read_in_transit_from_its_list() {
        let help = in_transit_help();
        assert!(help.contains(&crate::transient::listed(crate::transient::GH)), "{help}");
        for marker in crate::transient::GH {
            assert!(help.contains(marker.said), "{}: {help}", marker.said);
        }
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
        let here = Some("https://api.github.com/repos/sunfmin/hkj/issues/22");
        assert_eq!(gh.moved(22, here), None);
        assert_eq!(gh.moved(22, None), None);
        let moved = Some("https://api.github.com/repos/sunfmin/control/issues/2");
        assert_eq!(gh.moved(22, moved), Some("sunfmin/control#2".to_string()));
        let same_number = Some("https://api.github.com/repos/sunfmin/control/issues/22");
        assert_eq!(gh.moved(22, same_number), Some("sunfmin/control#22".to_string()));
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

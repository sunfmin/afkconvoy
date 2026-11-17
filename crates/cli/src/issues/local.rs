//! The local adapter of [`Issues`]: the host's local tracker (docs/adr/0091),
//! one SQLite database in the host's data directory (`store_path`), read and
//! written through `afkconvoy_issues::Store`. The fleet's issues there are
//! those whose repo — a field of the issue, not a label — is its checkout's:
//! every read is of that repo alone, and every issue the fleet opens is of
//! it. Nothing here asks GitHub, so no read spends its quota.
//!
//! An issue is opened only for a repo the host has a checkout of (`open`):
//! no fleet of the host would work one of any other. A blocker is an edge
//! between two of the host's own issues. A label needs no making: an issue
//! carries any it is given. A write is stamped with the time its command acts
//! at (`crate::acting_now`), so a command told `--now` writes as of then.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use afkconvoy_core::config::RepoName;
use afkconvoy_core::plan::Issue;
use afkconvoy_core::tracker::Tracker;
use afkconvoy_issues::{Error, Filter, State, Store, View, Which};
use anyhow::{Context, Result, bail};

use super::{
    AccountIssue, BlockedBy, Comment, Gone, IssueRead, Issues, OpenIssues, OpenRead, WatchIssue,
    Went,
};
use crate::checkout_list::{List, named_repo};
use crate::gh::GitHub;
use crate::git::Repo;

/// The database's name in the data directory.
const FILE: &str = "issues.db";

/// Where this host keeps its local tracker: `issues.db` in afkconvoy's data
/// directory — not its cache: it holds what nothing else can give back.
pub fn store_path() -> Result<PathBuf> {
    Ok(data_home()?.join("afkconvoy").join(FILE))
}

/// The directory the data directory is in: this user's, as XDG_DATA_HOME or
/// HOME says it.
#[cfg(not(test))]
fn data_home() -> Result<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    Ok(match (set("XDG_DATA_HOME"), set("HOME")) {
        (Some(data), _) => PathBuf::from(data),
        (None, Some(home)) => PathBuf::from(home).join(".local/share"),
        (None, None) => {
            bail!("neither XDG_DATA_HOME nor HOME says where this host's local tracker is kept")
        }
    })
}

/// The same, of a unit test: one of its own, made as it first asks and gone
/// once it ends — no test reads or writes the tracker of whoever runs it.
#[cfg(test)]
fn data_home() -> Result<PathBuf> {
    thread_local! {
        static OWN: tempfile::TempDir = tempfile::tempdir().expect("a unit test's own data");
    }
    Ok(OWN.with(|own| own.path().to_path_buf()))
}

/// The host's local tracker at `path`, made with its directory where there
/// is none.
pub fn open_store(path: &Path) -> Result<Store> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("cannot make the local tracker's {}", dir.display()))?;
    }
    Store::open(path).with_context(|| format!("the local tracker {}", path.display()))
}

/// Opens an issue of `repo` in `store` at `at` → its number. Refused,
/// naming the repo, where the host has no checkout of it: neither `here`, a
/// checkout of the host, nor any on the host's checkout list `list` — no
/// fleet of the host would ever work it (docs/adr/0091).
#[allow(clippy::too_many_arguments)]
pub fn open(
    store: &mut Store,
    list: &List,
    here: Option<&Repo>,
    repo: &RepoName,
    title: &str,
    body: &str,
    labels: &[&str],
    at: u64,
) -> Result<u64> {
    if !checked_out(list, here, repo)? {
        bail!(
            "this host has no checkout of {repo}: an issue of it in this host's local tracker \
             is one no fleet of this host would ever work. Open it on a host that works {repo}, \
             or `afkconvoy run` a checkout of it here first"
        );
    }
    stored(store.create(repo.as_str(), title, body, labels, at))
}

/// Whether the host has a checkout of `repo`: `here`, or one on `list`.
fn checked_out(list: &List, here: Option<&Repo>, repo: &RepoName) -> Result<bool> {
    let of = |checkout: &Repo| {
        named_repo(checkout).is_ok_and(|named| named.as_str().eq_ignore_ascii_case(repo.as_str()))
    };
    if here.is_some_and(of) {
        return Ok(true);
    }
    let listed = list.read()?.checkouts;
    Ok(listed
        .iter()
        .filter_map(|entry| Repo::at(&entry.checkout).ok())
        .any(|checkout| of(&checkout)))
}

/// What the store answered, its refusals said as the fleet tells them: an
/// issue it has no such number of is `Gone`, as one GitHub has not.
fn stored<T>(answer: afkconvoy_issues::Result<T>) -> Result<T> {
    answer.map_err(|error| match error {
        Error::NoSuchIssue(issue) => Gone { issue, how: Went::Missing }.into(),
        error => anyhow::Error::new(error).context("the local tracker"),
    })
}

/// The issues of the host's local tracker, of the repo of the checkout `gh`
/// is of.
pub(crate) struct LocalIssues {
    gh: GitHub,
    /// The database, opened as it is first asked.
    store: Mutex<Option<Store>>,
}

impl LocalIssues {
    pub(crate) fn new(gh: GitHub) -> LocalIssues {
        LocalIssues { gh, store: Mutex::new(None) }
    }

    /// The checkout's repo, whose issues these are.
    fn repo(&self) -> &RepoName {
        match &self.gh.tracker {
            Tracker::Local { repo, .. } => repo,
            _ => unreachable!("a local tracker's issues are of a local tracker"),
        }
    }

    /// The store, opened where it is not yet.
    fn store(&self) -> Result<MutexGuard<'_, Option<Store>>> {
        let mut store = self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if store.is_none() {
            *store = Some(open_store(&store_path()?)?);
        }
        Ok(store)
    }

    /// What `ask` answers of the store.
    fn with<T>(&self, ask: impl FnOnce(&mut Store) -> afkconvoy_issues::Result<T>) -> Result<T> {
        let mut store = self.store()?;
        stored(ask(store.as_mut().expect("opened")))
    }

    /// The repo's issues `state` and `filter` hold, each with its comments
    /// and blockers.
    fn views(&self, filter: Filter<'_>) -> Result<Vec<View>> {
        let repo = self.repo().as_str();
        self.with(|store| store.views(&Filter { repo: Some(repo), ..filter }))
    }

    /// The numbers of the repo's issues of `state` carrying `label`, with
    /// when each closed.
    fn carrying_in(&self, state: Which, label: &str) -> Result<Vec<(u64, Option<u64>)>> {
        let repo = self.repo().as_str();
        let labels = [label];
        let filter = Filter { repo: Some(repo), state, labels: &labels, updated_since: None };
        let listed = self.with(|store| store.list(&filter))?;
        Ok(listed.into_iter().map(|issue| (issue.number, issue.closed)).collect())
    }

    /// Where issue `number` is, as a human is told it: `local/<host>#7`.
    fn url(&self, number: u64) -> String {
        self.gh.tracker.issue(number)
    }

    fn comment_of(&self, comment: afkconvoy_issues::Comment) -> Comment {
        Comment {
            id: comment.id,
            url: format!("{}/comments/{}", self.url(comment.issue), comment.id),
            body: comment.body,
            at: Some(comment.created),
        }
    }

    fn comments_of(&self, view: &View) -> Vec<Comment> {
        view.comments.iter().cloned().map(|comment| self.comment_of(comment)).collect()
    }

    /// The edges of `view`, each of the tracker it is in — this one — and
    /// carrying the labels of its blocker; the repo it is the work of is its
    /// field (`BlockedBy::of`).
    fn blockers_of(&self, blockers: &[afkconvoy_issues::Blocker]) -> Vec<BlockedBy> {
        let tracker = self.gh.tracker.named().map(ToString::to_string);
        blockers
            .iter()
            .map(|blocker| BlockedBy {
                number: blocker.number,
                state: said_state(blocker.state()).to_string(),
                repo: tracker.clone(),
                labels: Some(blocker.labels.clone()),
                of: Some(blocker.repo.clone()),
            })
            .collect()
    }

    /// `(patrol, closed at)` of each patrol's issue closed in the day ending
    /// at `now`, among `closed`.
    fn landings(closed: &[View], now: u64) -> Vec<(String, u64)> {
        closed
            .iter()
            .filter_map(|view| {
                let at = view.issue.closed?;
                if now.saturating_sub(at) >= afkconvoy_core::patrol::DAY {
                    return None;
                }
                let bodies = view.comments.iter().map(|comment| comment.body.as_str());
                afkconvoy_core::intake::opened_by(bodies).map(|patrol| (patrol, at))
            })
            .collect()
    }

    /// The repo's issues closed in the day ending at `now`, with their
    /// comments: a close changes an issue, so none closed since is older.
    fn closed_since_day(&self, now: u64) -> Result<Vec<View>> {
        let since = now.saturating_sub(afkconvoy_core::patrol::DAY);
        self.views(Filter { state: Which::Closed, updated_since: Some(since), ..Filter::default() })
    }
}

/// A state as GitHub spells it, and as the fleet reads it from any tracker.
fn said_state(state: State) -> &'static str {
    match state {
        State::Open => "open",
        State::Closed => "closed",
    }
}

impl Issues for LocalIssues {
    fn tracker(&self) -> &Tracker {
        &self.gh.tracker
    }

    /// The landings of the day are read with them whenever asked: the store
    /// can say all of a day.
    fn open_issues(&self, day_ending: Option<u64>) -> Result<OpenRead> {
        let mut open = OpenIssues::default();
        for view in self.views(Filter::default())? {
            let number = view.issue.number;
            let blockers = self.blockers_of(&view.blockers);
            open.comments.insert(number, self.comments_of(&view));
            if let Some(edited) = view.issue.body_edited {
                open.edited.insert(number, edited);
            }
            open.reports.insert(
                number,
                Comment {
                    id: 0,
                    body: view.issue.body,
                    url: self.url(number),
                    at: Some(view.issue.created),
                },
            );
            open.rows.push(Issue {
                number,
                title: view.issue.title,
                labels: view.issue.labels,
                open_blockers: self.open_of(blockers.clone()),
            });
            open.blockers.insert(number, blockers);
        }
        let landings = match day_ending {
            Some(now) => Some(Self::landings(&self.closed_since_day(now)?, now)),
            None => None,
        };
        Ok(OpenRead { issues: open, landings })
    }

    /// Of whatever repo it is, as a GitHub tracker's issue is read whatever
    /// its repo label: the repo it is of says whose it is (`IssueRead::of`,
    /// `Tracker::to_take`). `Gone` of a number the tracker never gave.
    fn issue(&self, number: u64) -> Result<IssueRead> {
        let view = self.with(|store| store.view(number))?;
        let closed = view.issue.state() == State::Closed;
        if closed {
            self.gh.saw_closed(number);
        }
        Ok(IssueRead {
            state: said_state(view.issue.state()).to_string(),
            title: view.issue.title,
            // Every close the fleet makes is of one completed.
            state_reason: closed.then(|| "completed".to_string()),
            labels: view.issue.labels,
            pull_request: false,
            closed_by: None,
            body: Some(view.issue.body),
            of: Some(view.issue.repo),
        })
    }

    fn comments(&self, number: u64) -> Result<Vec<Comment>> {
        let view = self.with(|store| store.view(number))?;
        Ok(self.comments_of(&view))
    }

    fn blocked_by(&self, number: u64) -> Result<Vec<BlockedBy>> {
        let blockers = self.with(|store| store.blockers(number))?;
        Ok(self.blockers_of(&blockers))
    }

    fn open_of(&self, edges: Vec<BlockedBy>) -> Vec<u64> {
        for edge in &edges {
            if edge.state.eq_ignore_ascii_case("closed") {
                self.gh.saw_closed(edge.number);
            }
        }
        super::open_blockers(edges)
    }

    fn open_with(&self, label: &str) -> Result<Vec<u64>> {
        Ok(self.carrying_in(Which::Open, label)?.into_iter().map(|(number, _)| number).collect())
    }

    fn closed_with(&self, label: &str) -> Result<Vec<(u64, u64)>> {
        let closed = self.carrying_in(Which::Closed, label)?;
        Ok(closed.into_iter().filter_map(|(number, at)| Some((number, at?))).collect())
    }

    fn carrying(&self, label: &str) -> Result<Vec<(u64, Option<Option<u64>>)>> {
        let carrying = self.carrying_in(Which::All, label)?;
        Ok(carrying.into_iter().map(|(number, closed)| (number, closed.map(Some))).collect())
    }

    fn watching_issues(&self, since: u64) -> Result<Vec<WatchIssue>> {
        let filter = Filter { state: Which::All, updated_since: Some(since), ..Filter::default() };
        Ok(self
            .views(filter)?
            .into_iter()
            .map(|view| WatchIssue {
                comments: self.comments_of(&view),
                number: view.issue.number,
                title: view.issue.title,
                closed: view.issue.closed,
                labels: view.issue.labels,
                more: false,
            })
            .collect())
    }

    fn patrol_landings(&self, now: u64) -> Result<Vec<(String, u64)>> {
        Ok(Self::landings(&self.closed_since_day(now)?, now))
    }

    fn accounts(&self) -> Result<Vec<afkconvoy_core::accounts::Issue>> {
        let all = self.views(Filter { state: Which::All, ..Filter::default() })?;
        Ok(all
            .into_iter()
            .map(|view| afkconvoy_core::accounts::Issue {
                number: view.issue.number,
                closed: view.issue.state() == State::Closed,
                url: self.url(view.issue.number),
                account: super::work_account(self.comments_of(&view)),
            })
            .collect())
    }

    /// Of every repo, as a GitHub tracker's are. Whether a merged PR closed
    /// one is not kept: none is said landed.
    fn account_issues(&self) -> Result<Vec<AccountIssue>> {
        let all = Filter { state: Which::All, ..Filter::default() };
        let views = self.with(|store| store.views(&all))?;
        Ok(views
            .into_iter()
            .map(|view| AccountIssue {
                comments: self.comments_of(&view),
                number: view.issue.number,
                url: self.url(view.issue.number),
                closed: view.issue.state() == State::Closed,
                landed: false,
                labels: view.issue.labels,
                more: false,
            })
            .collect())
    }

    /// Those some issue carries: a label is made by being put on one.
    fn labels(&self) -> Result<Vec<String>> {
        self.with(|store| store.labels(None))
    }

    fn seen_closed(&self) -> Vec<u64> {
        self.gh.seen_closed()
    }

    /// Of the checkout's repo, which this host has a checkout of: this one.
    fn open_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64> {
        let list = List::of_host()?;
        let here = Repo::new(self.gh.dir.clone());
        let at = crate::acting_now();
        let mut store = self.store()?;
        let store = store.as_mut().expect("opened");
        open(store, &list, Some(&here), self.repo(), title, body, labels, at)
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.comment(number, body, at)).map(drop)
    }

    fn edit_comment(&self, id: u64, body: &str) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.edit_comment(id, body, at))
    }

    fn close_issue(&self, number: u64) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.close(number, at))?;
        self.gh.saw_closed(number);
        Ok(())
    }

    fn edit_issue(&self, number: u64, title: Option<&str>, body: Option<&str>) -> Result<()> {
        let at = crate::acting_now();
        if let Some(title) = title {
            self.with(|store| store.edit_title(number, title, at))?;
        }
        if let Some(body) = body {
            self.with(|store| store.edit_body(number, body, at))?;
        }
        Ok(())
    }

    fn reopen_issue(&self, number: u64) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.reopen(number, at))
    }

    fn add_blocker(&self, number: u64, blocker: u64) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.add_blocker(number, blocker, at))
    }

    fn remove_blocker(&self, number: u64, blocker: u64) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.remove_blocker(number, blocker, at))
    }

    /// Nothing to make: an issue carries any label it is given.
    fn create_label(&self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    fn add_label(&self, number: u64, label: &str) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.add_labels(number, &[label], at))
    }

    fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let at = crate::acting_now();
        self.with(|store| store.remove_labels(number, &[label], at))
    }
}

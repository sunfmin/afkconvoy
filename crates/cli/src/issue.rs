//! `afkconvoy issue`: what a worker or a human reads and writes of one of the
//! fleet's issues — opens one, views it, lists them, comments on one, edits,
//! closes, reopens, blocks or unblocks one — in the tracker the checkout's
//! host names for it, through the one seam every read and write of a
//! tracker's issues crosses (`crate::issues`). It is how the host's local
//! tracker is read and written by hand (docs/adr/0091), and it works on a
//! GitHub tracker alike, so a brief gives one line whatever the tracker.
//!
//! Its flags are `gh issue`'s where they mean the same, so a line of one
//! reads as a line of the other.

use std::io::Read;
use std::path::{Path, PathBuf};

use afkconvoy_core::config::RepoName;
use afkconvoy_core::tracker::Tracker;
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::checkout_list::List;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::issues::{self, Issues};

/// What one `afkconvoy issue` does.
#[derive(clap::Subcommand)]
pub(crate) enum Verb {
    /// Open an issue
    #[command(verbatim_doc_comment, hide = true)]
    Create {
        /// Its title
        #[arg(long, short = 't')]
        title: String,
        #[command(flatten)]
        body: Body,
        /// A label to put on it, once for each or separated by commas
        #[arg(long = "label", short = 'l', value_delimiter = ',')]
        labels: Vec<String>,
        /// The repo it is of, owner/repo, where it is not this checkout's: one this host has a checkout of. Of a local tracker only
        #[arg(long, short = 'R', value_parser = crate::tracker::repo_named)]
        repo: Option<RepoName>,
    },
    /// Read an issue
    #[command(verbatim_doc_comment, hide = true)]
    View {
        /// The issue's number in the tracker
        number: u64,
        /// With every comment on it, oldest first
        #[arg(long, short = 'c')]
        comments: bool,
        /// Only these fields, separated by commas: number, title, body, state, labels, comments, blockedBy, repo, url
        #[arg(long, value_delimiter = ',')]
        json: Option<Vec<String>>,
    },
    /// List the repo's issues
    #[command(verbatim_doc_comment, hide = true)]
    List {
        /// Which, by their state
        #[arg(long, short = 's', value_enum, default_value_t = State::Open)]
        state: State,
        /// Only those carrying this label, once for each or separated by commas: every one named
        #[arg(long = "label", short = 'l', value_delimiter = ',')]
        labels: Vec<String>,
        /// Only these fields of each, separated by commas: number, title, state, labels
        #[arg(long, value_delimiter = ',')]
        json: Option<Vec<String>>,
    },
    /// Comment on an issue
    #[command(verbatim_doc_comment, hide = true)]
    Comment {
        /// The issue's number in the tracker
        number: u64,
        #[command(flatten)]
        body: Body,
    },
    /// Edit an issue's title, body or labels
    #[command(verbatim_doc_comment, hide = true)]
    Edit {
        /// The issue's number in the tracker
        number: u64,
        /// Its new title
        #[arg(long, short = 't')]
        title: Option<String>,
        #[command(flatten)]
        body: NewBody,
        /// A label to put on it, once for each or separated by commas
        #[arg(long, value_delimiter = ',')]
        add_label: Vec<String>,
        /// A label to take off it, once for each or separated by commas
        #[arg(long, value_delimiter = ',')]
        remove_label: Vec<String>,
    },
    /// Close an issue
    #[command(verbatim_doc_comment, hide = true)]
    Close {
        /// The issue's number in the tracker
        number: u64,
        /// A comment to post on it first
        #[arg(long, short = 'c')]
        comment: Option<String>,
    },
    /// Reopen an issue
    #[command(verbatim_doc_comment, hide = true)]
    Reopen {
        /// The issue's number in the tracker
        number: u64,
    },
    /// Record what blocks an issue
    #[command(verbatim_doc_comment, hide = true)]
    Block {
        /// The issue's number in the tracker
        number: u64,
        /// The issues it is blocked by, once for each or separated by commas
        #[arg(long, required = true, value_delimiter = ',')]
        by: Vec<u64>,
    },
    /// Remove what blocks an issue
    #[command(verbatim_doc_comment, hide = true)]
    Unblock {
        /// The issue's number in the tracker
        number: u64,
        /// The issues it is no longer blocked by, once for each or separated by commas
        #[arg(long, required = true, value_delimiter = ',')]
        by: Vec<u64>,
    },
}

/// Which issues a listing holds, by their state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum State {
    Open,
    Closed,
    All,
}

/// The text a comment or a new issue carries.
#[derive(clap::Args)]
pub(crate) struct Body {
    /// The text
    #[arg(long, short = 'b', required_unless_present = "body_file", conflicts_with = "body_file")]
    body: Option<String>,
    /// A file holding the text; - for standard input
    #[arg(long, short = 'F')]
    body_file: Option<PathBuf>,
}

/// An issue's new body, where an edit gives one.
#[derive(clap::Args)]
pub(crate) struct NewBody {
    /// Its new body
    #[arg(long, short = 'b', conflicts_with = "body_file")]
    body: Option<String>,
    /// A file holding its new body; - for standard input
    #[arg(long, short = 'F')]
    body_file: Option<PathBuf>,
}

impl Body {
    fn text(&self) -> Result<String> {
        match (&self.body, &self.body_file) {
            (Some(body), _) => Ok(body.clone()),
            (None, Some(file)) => read(file),
            (None, None) => bail!("a text is given with --body or --body-file"),
        }
    }
}

impl NewBody {
    fn text(&self) -> Result<Option<String>> {
        match (&self.body, &self.body_file) {
            (Some(body), _) => Ok(Some(body.clone())),
            (None, Some(file)) => read(file).map(Some),
            (None, None) => Ok(None),
        }
    }
}

/// What `file` holds, standard input for `-`.
fn read(file: &Path) -> Result<String> {
    if file == Path::new("-") {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).context("cannot read standard input")?;
        return Ok(text);
    }
    std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))
}

/// Every field `view --json` names, as `gh issue view --json` names those it
/// has too.
const VIEW_FIELDS: [&str; 9] =
    ["number", "title", "body", "state", "labels", "comments", "blockedBy", "repo", "url"];
/// Every field `list --json` names.
const LIST_FIELDS: [&str; 4] = ["number", "title", "state", "labels"];

impl Verb {
    /// The issue it acts on, where it names one: what its acting log says.
    pub(crate) fn number(&self) -> Option<u64> {
        match self {
            Verb::Create { .. } | Verb::List { .. } => None,
            Verb::View { number, .. }
            | Verb::Comment { number, .. }
            | Verb::Edit { number, .. }
            | Verb::Close { number, .. }
            | Verb::Reopen { number }
            | Verb::Block { number, .. }
            | Verb::Unblock { number, .. } => Some(*number),
        }
    }

    /// Whether it writes: what it reads acts for nobody.
    pub(crate) fn writes(&self) -> bool {
        !matches!(self, Verb::View { .. } | Verb::List { .. })
    }
}

/// Does `verb` on the issues of the tracker the checkout `repo`'s host names.
pub(crate) fn issue(repo: &Repo, verb: Verb) -> Result<Value> {
    let gh = GitHub::of(repo)?;
    let issues = issues::of(&gh);
    let tracker = issues.tracker().clone();
    let named = |number: u64| tracker.issue(number);
    match verb {
        Verb::Create { title, body, labels, repo: of } => {
            let body = body.text()?;
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let number = match of {
                Some(of) if !is_the_repo(&tracker, &of) => {
                    create_of(repo, &tracker, &of, &title, &body, &labels)?
                }
                _ => issues.open_issue(&title, &body, &labels)?,
            };
            Ok(json!({"action": "created", "number": number, "issue": named(number)}))
        }
        Verb::View { number, comments, json } => {
            let fields = chosen(json.as_deref(), &VIEW_FIELDS)?;
            let comments =
                comments || fields.as_ref().is_some_and(|fields| fields.contains(&"comments"));
            let viewed = view(issues.as_ref(), number, comments)?;
            Ok(only(viewed, fields.as_deref()))
        }
        Verb::List { state, labels, json } => {
            let fields = chosen(json.as_deref(), &LIST_FIELDS)?;
            let listed = list(issues.as_ref(), state, &labels)?;
            let listed = listed.into_iter().map(|row| only(row, fields.as_deref())).collect();
            Ok(json!({ "issues": Value::Array(listed) }))
        }
        Verb::Comment { number, body } => {
            issues.comment(number, &body.text()?)?;
            Ok(json!({"action": "commented", "number": number, "issue": named(number)}))
        }
        Verb::Edit { number, title, body, add_label, remove_label } => {
            let body = body.text()?;
            if title.is_none() && body.is_none() && add_label.is_empty() && remove_label.is_empty()
            {
                bail!(
                    "nothing to edit: give --title, --body, --body-file, --add-label or --remove-label"
                );
            }
            issues.issue(number)?;
            issues.edit_issue(number, title.as_deref(), body.as_deref())?;
            for label in &add_label {
                issues.add_label(number, label)?;
            }
            for label in &remove_label {
                issues.remove_label(number, label)?;
            }
            Ok(json!({"action": "edited", "number": number, "issue": named(number)}))
        }
        Verb::Close { number, comment } => {
            if let Some(comment) = comment {
                issues.comment(number, &comment)?;
            }
            issues.close_issue(number)?;
            Ok(json!({"action": "closed", "number": number, "issue": named(number)}))
        }
        Verb::Reopen { number } => {
            issues.reopen_issue(number)?;
            Ok(json!({"action": "reopened", "number": number, "issue": named(number)}))
        }
        Verb::Block { number, by } => {
            for blocker in &by {
                issues.add_blocker(number, *blocker)?;
            }
            let open = issues.open_blockers(number)?;
            Ok(
                json!({"action": "blocked", "number": number, "issue": named(number), "open_blockers": open}),
            )
        }
        Verb::Unblock { number, by } => {
            for blocker in &by {
                issues.remove_blocker(number, *blocker)?;
            }
            let open = issues.open_blockers(number)?;
            Ok(
                json!({"action": "unblocked", "number": number, "issue": named(number), "open_blockers": open}),
            )
        }
    }
}

/// Whether `of` is the repo whose issues the tracker's fleet works here.
fn is_the_repo(tracker: &Tracker, of: &RepoName) -> bool {
    match tracker {
        Tracker::Own => false,
        Tracker::Other { repo, .. } | Tracker::Local { repo, .. } => {
            repo.as_str().eq_ignore_ascii_case(of.as_str())
        }
    }
}

/// Opens an issue of another repo, `of`, in the host's local tracker, which
/// keeps an issue's repo as a field of it; refused of a GitHub tracker,
/// where the repo is a label of the issue another repo's checkout puts on.
fn create_of(
    here: &Repo,
    tracker: &Tracker,
    of: &RepoName,
    title: &str,
    body: &str,
    labels: &[&str],
) -> Result<u64> {
    if !matches!(tracker, Tracker::Local { .. }) {
        bail!(
            "--repo names the repo of an issue in this host's local tracker, and the tracker of \
             this checkout is on GitHub: open an issue of {of} with `afkconvoy issue create` in \
             a checkout of {of}"
        );
    }
    let mut store = issues::local::open_store(&issues::local::store_path()?)?;
    let at = crate::acting_now();
    issues::local::open(&mut store, &List::of_host()?, Some(here), of, title, body, labels, at)
}

/// The fields `json` names, each one of `known`; `None` where it names none.
fn chosen<'a>(json: Option<&[String]>, known: &[&'a str]) -> Result<Option<Vec<&'a str>>> {
    let Some(json) = json else { return Ok(None) };
    let mut fields = vec![];
    for field in json.iter().map(|field| field.trim()).filter(|field| !field.is_empty()) {
        match known.iter().find(|known| **known == field) {
            Some(known) => fields.push(*known),
            None => bail!("no field `{field}`: --json takes {}", known.join(", ")),
        }
    }
    Ok(Some(fields))
}

/// `object` with only `fields` of it, where some are named.
fn only(object: Value, fields: Option<&[&str]>) -> Value {
    let Some(fields) = fields else { return object };
    let Value::Object(all) = object else { return object };
    let kept: Map<String, Value> =
        all.into_iter().filter(|(name, _)| fields.contains(&name.as_str())).collect();
    Value::Object(kept)
}

/// A state as `gh issue --json` says it.
fn said_state(closed: bool) -> &'static str {
    if closed { "CLOSED" } else { "OPEN" }
}

/// Labels as `gh issue --json` says them: each an object with its name.
fn said_labels(labels: &[String]) -> Value {
    labels.iter().map(|name| json!({ "name": name })).collect()
}

/// Issue `number` as `view` answers it.
fn view(issues: &dyn Issues, number: u64, with_comments: bool) -> Result<Value> {
    let read = issues.issue(number)?;
    let tracker = issues.tracker();
    let blockers: Vec<Value> = issues
        .blocked_by(number)?
        .into_iter()
        .map(|edge| json!({"number": edge.number, "state": edge.state.to_ascii_uppercase()}))
        .collect();
    let repo = read.of.clone().or_else(|| match tracker {
        Tracker::Own => None,
        Tracker::Other { repo, .. } | Tracker::Local { repo, .. } => Some(repo.to_string()),
    });
    let closed = read.is_closed();
    let mut viewed = json!({
        "number": number,
        "title": read.title,
        "body": read.body.unwrap_or_default(),
        "state": said_state(closed),
        "labels": said_labels(&read.labels),
        "blockedBy": blockers,
        "repo": repo,
        "url": tracker.issue(number),
    });
    if with_comments {
        let comments: Vec<Value> = issues
            .comments(number)?
            .into_iter()
            .map(|comment| {
                json!({
                    "id": comment.id,
                    "body": comment.body,
                    "url": comment.url,
                    "createdAt": comment.at.map(crate::log::stamp),
                })
            })
            .collect();
        viewed["comments"] = Value::Array(comments);
    }
    Ok(viewed)
}

/// The repo's issues of `state` carrying every one of `labels`, as `list`
/// answers them.
fn list(issues: &dyn Issues, state: State, labels: &[String]) -> Result<Vec<Value>> {
    let carries = |carried: &[String]| {
        labels.iter().all(|label| carried.iter().any(|has| has.eq_ignore_ascii_case(label)))
    };
    let mut rows: Vec<(u64, String, bool, Vec<String>)> = vec![];
    if state != State::Closed {
        let open = issues.open_issues(None)?.issues.rows;
        rows.extend(open.into_iter().map(|row| (row.number, row.title, false, row.labels)));
    }
    if state != State::Open {
        let all = issues.watching_issues(0)?;
        let closed = all.into_iter().filter(|issue| issue.closed.is_some());
        rows.extend(closed.map(|issue| (issue.number, issue.title, true, issue.labels)));
    }
    rows.retain(|(_, _, _, carried)| carries(carried));
    rows.sort_by_key(|(number, ..)| *number);
    Ok(rows
        .into_iter()
        .map(|(number, title, closed, labels)| {
            json!({
                "number": number,
                "title": title,
                "state": said_state(closed),
                "labels": said_labels(&labels),
            })
        })
        .collect())
}

pub(crate) const HELP: &str = r#"## When to run it

To read or write one of the fleet's issues by hand: to open one for it, read one with its comments, comment on one, change its title, body or labels, close or reopen one, or record what blocks it. Where this checkout's tracker is its host's local tracker (`afkconvoy run --tracker local`), this is the only way to: the issues are in a database on this host, and `gh issue` reads and writes the GitHub repo's own issues, not the fleet's. Where the tracker is on GitHub, it acts there as `gh issue` would, through the same calls the fleet makes.

A worker's brief, `doctor`'s fixes and the lines a human answers an escalation with give the line to run, with `afkconvoy issue` where the tracker is local.

## Who runs it

Whoever hands the fleet work — a human or their agent — in a checkout of the repo on the fleet's host, or with `-C <checkout>` from anywhere; and a worker, in its own worktree. A local tracker is its host's alone: run it on the host whose fleets work the issue.

## What it reads and writes

The tracker this host's checkout list names for the checkout (`afkconvoy checkouts`), or the one the fleet that started a worker kept in its worktree: the host's local tracker, the checkout's own repo, or another repo. Every verb but `view` and `list` writes there, then wakes this host's `run` of the repo, as `afkconvoy wake` does: the next tick reads what was written.

- `create`: an issue of this checkout's repo. `--repo <owner>/<repo>` opens one of another repo instead, in a local tracker only: a repo this host has a checkout of — refused, naming it, of any other: no fleet of this host would work it. In a tracker that is another repo, it carries this repo's repo label.
- `view <n>`: one issue, of whatever repo, open or closed.
- `list`: this repo's issues, open ones unless `--state` says otherwise, each with every label `--label` names.
- `comment <n>`, `edit <n>`, `close <n>`, `reopen <n>`: as `gh issue`'s of the same names. `close --comment` posts the comment first.
- `block <n> --by <m>`: records issue `n` as blocked by `m`: the fleet does not dispatch `n` while `m` is open. `unblock <n> --by <m>` takes that edge away. An edge is between two issues of the one tracker; a local tracker refuses one that would have an issue wait on itself, through any edges.

## What it answers

`create`: {"action": "created", "number", "issue"} — `issue` is how the repo names it: `#7`, `<owner>/<tracker>#7`, or `local/<host>#7` of a local tracker, which a PR closes with `Closes local/<host>#7`.

`view`: {"number", "title", "body", "state", "labels", "blockedBy", "repo", "url"}, and "comments" with `--comments` — `state` is `OPEN` or `CLOSED`, `labels` [{"name"}], `blockedBy` [{"number", "state"}], each comment {"id", "body", "url", "createdAt"}. `--json <fields>` keeps only the fields it names, as `gh issue view --json` does: `--json number,title,body,labels,comments,state` answers what gh's would.

`list`: {"issues": [{"number", "title", "state", "labels"}]}, lowest number first; `--json` keeps only the fields it names of each.

`comment`, `edit`, `close`, `reopen`: {"action", "number", "issue"}. `block`, `unblock`: the same and "open_blockers", the issues that still block it.

Every verb that writes also answers "woke": whether a `run` heard the wake.

Errors, as {"error": …} with a non-zero exit: "no such issue" of a number the tracker never gave; a `--repo` this host has no checkout of, or of a GitHub tracker; a label or edge the tracker refused; "is not in a git checkout", or git's own words for a checkout it refuses.

## What trips you up

- `gh issue` in a checkout whose tracker is local acts on the GitHub repo's own issue of that number — another issue, or none. A worker's guard refuses `gh issue …` and `gh api …/issues/…` there, naming this command.
- A flow label, a `stage:` label, the escalate label and the lock label are the fleet's: a worker's guard refuses them on `edit --add-label`, `--remove-label` and `create --label`, as on `gh issue`.
- `close` closes it as completed; the fleet leaves a closed issue alone. To hand an escalated issue back, use the line `afkconvoy doctor` names for it."#;

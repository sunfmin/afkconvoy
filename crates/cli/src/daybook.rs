//! `afkconvoy daybook`: one entry appended to the **daybook**, the operator's
//! one place to read, after the fact, what agents did and decided outside any
//! issue. Its home is the tracker's GitHub Discussions, category `Daybook`:
//! one discussion a day, titled with the host's local date, one comment an
//! entry. It is not fleet state: nothing reads it to decide what the fleet
//! does.
//!
//! Two agents writing the first entry of a day at once may each find no
//! discussion and each create one. Discussion numbers only grow, so the
//! day's discussion is the one of its title with the lowest number: every
//! entry goes there, and a creator that finds an older one deletes its own,
//! which no entry went to — an entry is only ever written to the lowest
//! numbered discussion its writer saw after any creation of its own.

use std::path::PathBuf;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::host_watch::WATCH_ENV;
use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::role::{ISSUE_ENV, ROLE_ENV};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::gh::{GitHub, Tracker};
use crate::git::Repo;

/// The discussion category every day's discussion is in.
pub const CATEGORY: &str = "Daybook";

/// Marks the asking session's processes, beside the watch's own marker
/// (`AFKCONVOY_WATCH=host`) the two share.
pub const ASKING_ENV: &str = "AFKCONVOY_ASKING";

/// What the caller says of one entry. Each heading holds what was said for
/// it; one said of nothing is left out of the entry.
#[derive(Debug, Default, Clone)]
pub struct Said {
    pub did: Option<String>,
    pub why: Option<String>,
    pub undo: Option<String>,
    pub links: Vec<String>,
}

/// The kind of session that writes an entry, as its environment tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Interactive,
    Watch,
    Asking,
    Worker,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Interactive => "interactive",
            Kind::Watch => "watch",
            Kind::Asking => "asking session",
            Kind::Worker => "worker",
        }
    }

    /// The session `var` describes: the asking session by its own marker,
    /// the watch by the marker it shares with it, a worker by what a fleet
    /// starts one with — a role, an issue or a patrol, or the name of the
    /// worker a landing turn runs for — and else a human's interactive one.
    pub fn of(var: impl Fn(&str) -> Option<String>) -> Kind {
        if var(ASKING_ENV).is_some() {
            return Kind::Asking;
        }
        match var(WATCH_ENV).as_deref() {
            Some("host") => return Kind::Watch,
            Some(_) => return Kind::Worker,
            None => {}
        }
        if [ROLE_ENV, ISSUE_ENV, PATROL_ENV].iter().any(|name| var(name).is_some()) {
            Kind::Worker
        } else {
            Kind::Interactive
        }
    }
}

/// What was said for a heading, where it says anything.
fn some(said: &Option<String>) -> Option<&str> {
    said.as_deref().filter(|said| !said.trim().is_empty())
}

/// The one shape of an entry: who wrote it, then each heading said.
pub fn entry(repo: &str, kind: Kind, said: &Said) -> String {
    let mut text = format!("**Repo** `{repo}` · **Session** {}\n", kind.word());
    let mut heading = |name: &str, body: &str| {
        text.push_str(&format!("\n### {name}\n\n{}\n", body.trim()));
    };
    if let Some(did) = some(&said.did) {
        heading("Did", did);
    }
    if let Some(why) = some(&said.why) {
        heading("Why", why);
    }
    if let Some(undo) = some(&said.undo) {
        heading("Undo", undo);
    }
    let links: Vec<String> = said
        .links
        .iter()
        .filter(|link| !link.trim().is_empty())
        .map(|link| format!("- {}", link.trim()))
        .collect();
    if !links.is_empty() {
        heading("Links", &links.join("\n"));
    }
    text
}

/// Today on this host, `YYYY-MM-DD` in its local time.
pub fn today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs()) as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: `localtime_r` writes only the `tm` it is handed.
    let local = unsafe {
        libc::localtime_r(&now, local.as_mut_ptr());
        local.assume_init()
    };
    format!("{:04}-{:02}-{:02}", local.tm_year + 1900, local.tm_mon + 1, local.tm_mday)
}

/// Where the entry goes, and of which repo it is.
pub struct Asked {
    pub said: Said,
    pub repo: Option<RepoName>,
    pub tracker: Option<RepoName>,
}

/// Appends one entry, said in `asked`, to today's discussion of the tracker
/// → {url, discussion, day, created}.
pub fn daybook(checkout: Result<Repo>, asked: Asked) -> Result<Value> {
    let Asked { said, repo, tracker } = asked;
    if said.did.is_none() && said.why.is_none() && said.undo.is_none() && said.links.is_empty() {
        bail!(
            "an entry says nothing: give its text, or one of `--did`, `--why`, `--undo`, `--link`"
        );
    }
    // Why it is about no checkout, where it is about none.
    let (checkout, none) = match checkout {
        Ok(checkout) => (Some(checkout), String::new()),
        Err(error) => (None, format!("{error:#}")),
    };
    let repo = match (repo, &checkout) {
        (Some(repo), _) => repo,
        (None, Some(checkout)) => crate::checkout_list::named_repo(checkout)?,
        (None, None) => {
            bail!("{none}: name the repo the entry is of with `--repo`")
        }
    };
    let tracker = match (tracker, &checkout) {
        (Some(tracker), _) => tracker,
        (None, Some(checkout)) => {
            let list = crate::checkout_list::List::of_host()?;
            crate::checkout_list::said_tracker(&list, checkout)?.unwrap_or_else(|| repo.clone())
        }
        (None, None) => bail!("{none}: name the tracker whose daybook it is with `--tracker`"),
    };
    let dir = checkout.map_or_else(std::env::temp_dir, |checkout| checkout.dir);
    let kind = Kind::of(|name| std::env::var(name).ok());
    let body = entry(repo.as_str(), kind, &said);
    append(&Discussions::new(dir, &tracker), &today(), &body)
}

/// One day's discussion, as a listing reads it.
#[derive(Debug, Clone, Deserialize)]
struct Day {
    id: String,
    number: u64,
    title: String,
    url: String,
}

/// The tracker's discussions, through `gh api graphql`.
struct Discussions {
    gh: GitHub,
    owner: String,
    name: String,
}

const PLACE: &str = "query ConvoyDaybookPlace($owner: String!, $repo: String!) { \
    repository(owner: $owner, name: $repo) { id hasDiscussionsEnabled \
    discussionCategories(first: 100) { nodes { id name } } } }";

/// The listing of a category's latest discussions, under the operation
/// name `name`: the first look, or the look after a creation.
fn days_query(name: &str) -> String {
    format!(
        "query {name}($owner: String!, $repo: String!, $category: ID!) {{ \
         repository(owner: $owner, name: $repo) {{ discussions(first: 100, categoryId: $category, \
         orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ id number title url }} }} }} }}"
    )
}

const OPEN: &str = "mutation ConvoyDaybookOpen($repository: ID!, $category: ID!, $title: String!, \
    $body: String!) { createDiscussion(input: {repositoryId: $repository, categoryId: $category, \
    title: $title, body: $body}) { discussion { id number title url } } }";

const DROP: &str = "mutation ConvoyDaybookDrop($id: ID!) { \
    deleteDiscussion(input: {id: $id}) { discussion { id } } }";

const ENTRY: &str = "mutation ConvoyDaybookEntry($discussion: ID!, $body: String!) { \
    addDiscussionComment(input: {discussionId: $discussion, body: $body}) { comment { url } } }";

impl Discussions {
    fn new(dir: PathBuf, tracker: &RepoName) -> Discussions {
        let (owner, name) = tracker.as_str().split_once('/').expect("owner/repo");
        Discussions {
            gh: GitHub::new(dir, Tracker::Own),
            owner: owner.to_string(),
            name: name.to_string(),
        }
    }

    fn tracker(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// `query` with `fields`, answered as JSON: read again on a failure in
    /// transit where it only reads, made once where it writes.
    fn ask(&self, query: &str, fields: &[(&str, &str)], writes: bool) -> Result<Value> {
        let owned: Vec<String> = [("owner", self.owner.as_str()), ("repo", self.name.as_str())]
            .iter()
            .filter(|_| !writes)
            .chain(fields)
            .map(|(key, value)| format!("{key}={value}"))
            .chain([format!("query={query}")])
            .collect();
        let mut args = vec!["api", "graphql"];
        for field in &owned {
            args.extend(["-f", field.as_str()]);
        }
        let out = if writes { self.gh.gh(&args)? } else { self.gh.read(&args)? };
        let answer: Value = serde_json::from_str(&out).context("GitHub's answer is no JSON")?;
        if let Some(errors) = answer.get("errors").filter(|errors| !errors.is_null()) {
            bail!("GitHub refused it: {errors}");
        }
        Ok(answer)
    }

    /// The tracker's id and its `Daybook` category's; an error naming what
    /// makes them where it has no such category.
    fn place(&self) -> Result<(String, String)> {
        let answer = self.ask(PLACE, &[], false)?;
        let repository = &answer["data"]["repository"];
        let tracker = self.tracker();
        let Some(id) = repository["id"].as_str() else {
            bail!("GitHub answered no repository {tracker}");
        };
        let create = format!(
            "create the category at https://github.com/{tracker}/discussions/categories/new, \
             named `{CATEGORY}` (GitHub creates none through its API); nothing was written"
        );
        if repository["hasDiscussionsEnabled"] == json!(false) {
            bail!(
                "the tracker {tracker} has no discussions, so no `{CATEGORY}` category: turn them \
                 on with `gh repo edit {tracker} --enable-discussions`, then {create}"
            );
        }
        let categories = repository["discussionCategories"]["nodes"].as_array();
        let category = categories.into_iter().flatten().find(|category| {
            category["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(CATEGORY))
        });
        match category.and_then(|category| category["id"].as_str()) {
            Some(category) => Ok((id.to_string(), category.to_string())),
            None => {
                bail!("the tracker {tracker} has no discussion category `{CATEGORY}`: {create}")
            }
        }
    }

    /// The discussion of `day` in `category` with the lowest number, of
    /// those the look `name` sees.
    fn day(&self, name: &str, category: &str, day: &str) -> Result<Option<Day>> {
        let answer = self.ask(&days_query(name), &[("category", category)], false)?;
        let nodes = answer["data"]["repository"]["discussions"]["nodes"].clone();
        let days: Vec<Day> =
            serde_json::from_value(nodes).context("GitHub listed no discussions")?;
        Ok(days.into_iter().filter(|found| found.title == day).min_by_key(|found| found.number))
    }

    fn open(&self, repository: &str, category: &str, day: &str) -> Result<Day> {
        let body = format!(
            "What agents did and decided outside any issue on {day}, one comment an entry. \
             Written by `afkconvoy daybook`; nothing reads it to decide what a fleet does."
        );
        let fields =
            [("repository", repository), ("category", category), ("title", day), ("body", &body)];
        let answer = self.ask(OPEN, &fields, true)?;
        serde_json::from_value(answer["data"]["createDiscussion"]["discussion"].clone())
            .context("GitHub answered no discussion it created")
    }

    fn drop(&self, id: &str) -> Result<()> {
        self.ask(DROP, &[("id", id)], true).map(|_| ())
    }

    fn comment(&self, discussion: &str, body: &str) -> Result<String> {
        let answer = self.ask(ENTRY, &[("discussion", discussion), ("body", body)], true)?;
        answer["data"]["addDiscussionComment"]["comment"]["url"]
            .as_str()
            .map(str::to_string)
            .context("GitHub answered no comment it added")
    }
}

/// Appends `body` to the discussion of `day`, creating it where there is
/// none, and settling a creation that raced another (the module's doc).
fn append(discussions: &Discussions, day: &str, body: &str) -> Result<Value> {
    let (repository, category) = discussions.place()?;
    let (discussion, created) = match discussions.day("ConvoyDaybookDay", &category, day)? {
        Some(found) => (found, false),
        None => {
            let mine = discussions.open(&repository, &category, day)?;
            let oldest = discussions.day("ConvoyDaybookSettle", &category, day)?;
            match oldest.filter(|oldest| oldest.number < mine.number) {
                Some(oldest) => {
                    discussions.drop(&mine.id).with_context(|| {
                        format!("{} is a second discussion of {day}; delete it", mine.url)
                    })?;
                    (oldest, false)
                }
                None => (mine, true),
            }
        }
    };
    let url = discussions.comment(&discussion.id, body)?;
    Ok(json!({"url": url, "discussion": discussion.url, "day": day, "created": created}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(did: &str) -> Said {
        Said { did: Some(did.to_string()), ..Said::default() }
    }

    #[test]
    fn an_entry_says_its_repo_and_session_and_only_the_headings_said() {
        let entry = entry("acme/dev", Kind::Watch, &said("installed jq"));
        assert_eq!(entry, "**Repo** `acme/dev` · **Session** watch\n\n### Did\n\ninstalled jq\n");
        let all = Said {
            did: Some("ruled".into()),
            why: Some("asked".into()),
            undo: Some("rule again".into()),
            links: vec!["https://x/1".into(), "https://x/2".into()],
        };
        let entry = super::entry("acme/dev", Kind::Worker, &all);
        assert!(entry.contains("### Did\n\nruled\n\n### Why\n\nasked\n\n### Undo\n\nrule again\n"));
        assert!(entry.ends_with("### Links\n\n- https://x/1\n- https://x/2\n"), "{entry}");
        let blank = Said { why: Some("  ".into()), ..said("x") };
        assert!(!super::entry("acme/dev", Kind::Interactive, &blank).contains("Why"));
    }

    #[test]
    fn the_session_kind_is_told_by_what_its_environment_holds() {
        let of = |set: &[(&str, &str)]| {
            let set: Vec<(String, String)> =
                set.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            Kind::of(|name| set.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
        };
        assert_eq!(of(&[]), Kind::Interactive);
        assert_eq!(of(&[(WATCH_ENV, "host")]), Kind::Watch);
        assert_eq!(of(&[(WATCH_ENV, "host"), (ASKING_ENV, "1")]), Kind::Asking);
        assert_eq!(of(&[(WATCH_ENV, "worker-7")]), Kind::Worker);
        assert_eq!(of(&[(ISSUE_ENV, "520")]), Kind::Worker);
    }

    #[test]
    fn today_is_a_date() {
        let day = today();
        let bytes = day.as_bytes();
        assert_eq!(bytes.len(), 10, "{day}");
        assert!(bytes[4] == b'-' && bytes[7] == b'-', "{day}");
    }
}

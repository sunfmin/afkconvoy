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
//!
//! An entry can carry a **lesson**: one sentence its writer thinks should
//! outlast the day, raw material for the weekly distillation. The entries
//! are the lessons' one home; the day's discussion body holds an index of
//! them, derived from its entries — rebuilt whole by each run that writes a
//! lesson, and checked after it is written — so the lessons of a span of
//! days are listed from the days' bodies alone, without fetching an entry.
//! Two lesson writers at once can each write an index the other's lesson is
//! missing from; each checks its own, and the next lesson's rebuild restores
//! any other.

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
    pub lesson: Option<String>,
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
    if let Some(lesson) = some(&said.lesson) {
        heading(LESSON, &one_line(lesson));
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

/// The heading a lesson is under.
const LESSON: &str = "Lesson";

/// A lesson is one sentence: its text on one line, however it was said.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The lesson an entry carries, as its `Lesson` heading holds it.
pub fn lesson_of(entry: &str) -> Option<String> {
    let (_, after) = entry.split_once(&format!("\n### {LESSON}\n\n"))?;
    let lesson = after.split("\n### ").next().unwrap_or_default();
    Some(one_line(lesson)).filter(|lesson| !lesson.is_empty())
}

/// Begins the lesson index of a day's discussion body; all after it is
/// the index.
const INDEX: &str = "<!--convoy:lessons-->";

/// One lesson in a day's index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    pub url: String,
    pub lesson: String,
}

/// `body` with its lesson index made of `lessons`, all that came before
/// the index kept; with none, no index.
pub fn indexed(body: &str, lessons: &[Indexed]) -> String {
    let kept = body.split(INDEX).next().unwrap_or_default().trim_end();
    if lessons.is_empty() {
        return kept.to_string();
    }
    let mut text = format!("{kept}\n\n{INDEX}\n## Lessons\n\n");
    for Indexed { url, lesson } in lessons {
        text.push_str(&format!("- [entry]({url}): {lesson}\n"));
    }
    text
}

/// The lessons a day's discussion body indexes, in its order.
pub fn index_of(body: &str) -> Vec<Indexed> {
    let Some((_, index)) = body.split_once(INDEX) else { return Vec::new() };
    index
        .lines()
        .filter_map(|line| {
            let (url, lesson) = line.strip_prefix("- [entry](")?.split_once("): ")?;
            Some(Indexed { url: url.to_string(), lesson: lesson.trim().to_string() })
        })
        .collect()
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
    let nothing =
        [&said.did, &said.why, &said.undo, &said.lesson].iter().all(|said| said.is_none());
    if nothing && said.links.is_empty() {
        bail!(
            "an entry says nothing: give its text, or one of `--did`, `--why`, `--undo`, \
             `--lesson`, `--link`"
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

/// A day, `YYYY-MM-DD`, as a flag says it.
pub fn day_named(said: &str) -> Result<String, String> {
    let bytes = said.as_bytes();
    let digits =
        [0, 1, 2, 3, 5, 6, 8, 9].iter().all(|at| bytes.get(*at).is_some_and(u8::is_ascii_digit));
    match bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-' && digits {
        true => Ok(said.to_string()),
        false => Err(format!("`{said}` is no day: say it `YYYY-MM-DD`")),
    }
}

/// The lessons of the days `since` to `until`, both included, from the
/// days' indexes of the tracker → {lessons: [{day, url, lesson}], since,
/// until}.
pub fn lessons(
    checkout: Result<Repo>,
    tracker: Option<RepoName>,
    since: &str,
    until: Option<&str>,
) -> Result<Value> {
    let (dir, tracker) = match (tracker, checkout) {
        (Some(tracker), checkout) => {
            (checkout.map_or_else(|_| std::env::temp_dir(), |checkout| checkout.dir), tracker)
        }
        (None, Ok(checkout)) => {
            let list = crate::checkout_list::List::of_host()?;
            let tracker = match crate::checkout_list::said_tracker(&list, &checkout)? {
                Some(tracker) => tracker,
                None => crate::checkout_list::named_repo(&checkout)?,
            };
            (checkout.dir, tracker)
        }
        (None, Err(error)) => {
            bail!("{error:#}: name the tracker whose daybook it is with `--tracker`")
        }
    };
    let discussions = Discussions::new(dir, &tracker);
    let (_, category) = discussions.place()?;
    let lessons: Vec<Value> = discussions
        .days_since(&category, since)?
        .into_iter()
        .filter(|day| until.is_none_or(|until| day.title.as_str() <= until))
        .flat_map(|day| {
            index_of(&day.body).into_iter().map(move |Indexed { url, lesson }| {
                json!({"day": day.title, "url": url, "lesson": lesson})
            })
        })
        .collect();
    Ok(json!({"lessons": lessons, "since": since, "until": until}))
}

/// One day's discussion, as a listing reads it.
#[derive(Debug, Clone, Deserialize)]
struct Day {
    id: String,
    number: u64,
    title: String,
    url: String,
}

/// One day's discussion as the lessons' listing reads it.
#[derive(Debug, Clone, Deserialize)]
struct Listed {
    number: u64,
    title: String,
    body: String,
}

/// The cursor of the page after the one `connection` holds, where there is one.
fn next_page(connection: &Value) -> Option<String> {
    let info = &connection["pageInfo"];
    info["hasNextPage"].as_bool().filter(|more| *more)?;
    info["endCursor"].as_str().map(str::to_string)
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

/// A day's discussion's body and a page of its entries.
const READ: &str = "query ConvoyDaybookRead($owner: String!, $repo: String!, $number: Int!, \
    $after: String) { repository(owner: $owner, name: $repo) { discussion(number: $number) { \
    body comments(first: 100, after: $after) { nodes { body url } \
    pageInfo { hasNextPage endCursor } } } } }";

const INDEXES: &str = "mutation ConvoyDaybookIndex($discussion: ID!, $body: String!) { \
    updateDiscussion(input: {discussionId: $discussion, body: $body}) { discussion { id } } }";

/// A page of a category's latest discussions with their bodies.
const LESSONS: &str = "query ConvoyDaybookLessons($owner: String!, $repo: String!, \
    $category: ID!, $after: String) { repository(owner: $owner, name: $repo) { \
    discussions(first: 100, after: $after, categoryId: $category, \
    orderBy: {field: CREATED_AT, direction: DESC}) { nodes { number title body } \
    pageInfo { hasNextPage endCursor } } } }";

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
    /// transit where it only reads, made once where it writes. A `number`
    /// is sent typed, as the Int it is.
    fn ask(&self, query: &str, fields: &[(&str, &str)], writes: bool) -> Result<Value> {
        let owned: Vec<(&str, String)> =
            [("owner", self.owner.as_str()), ("repo", self.name.as_str())]
                .iter()
                .filter(|_| !writes)
                .chain(fields)
                .map(|(key, value)| {
                    (if *key == "number" { "-F" } else { "-f" }, format!("{key}={value}"))
                })
                .chain([("-f", format!("query={query}"))])
                .collect();
        let mut args = vec!["api", "graphql"];
        for (flag, field) in &owned {
            args.extend([*flag, field.as_str()]);
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

    /// The body of the day's discussion `number` and the lessons its
    /// entries carry, in their order.
    fn read(&self, number: u64) -> Result<(String, Vec<Indexed>)> {
        let number = number.to_string();
        let (mut lessons, mut after) = (Vec::new(), None::<String>);
        loop {
            let mut fields = vec![("number", number.as_str())];
            if let Some(after) = &after {
                fields.push(("after", after.as_str()));
            }
            let answer = self.ask(READ, &fields, false)?;
            let discussion = &answer["data"]["repository"]["discussion"];
            let body = discussion["body"].as_str().context("GitHub answered no discussion")?;
            let comments = &discussion["comments"];
            for comment in comments["nodes"].as_array().into_iter().flatten() {
                let (Some(url), Some(lesson)) =
                    (comment["url"].as_str(), comment["body"].as_str().and_then(lesson_of))
                else {
                    continue;
                };
                lessons.push(Indexed { url: url.to_string(), lesson });
            }
            match next_page(comments) {
                Some(cursor) => after = Some(cursor),
                None => return Ok((body.to_string(), lessons)),
            }
        }
    }

    fn index(&self, discussion: &str, body: &str) -> Result<()> {
        self.ask(INDEXES, &[("discussion", discussion), ("body", body)], true).map(|_| ())
    }

    /// The discussions of `category` titled `since` or later, the lowest
    /// numbered of each title, oldest day first.
    fn days_since(&self, category: &str, since: &str) -> Result<Vec<Listed>> {
        let mut days: Vec<Listed> = Vec::new();
        let mut after = None::<String>;
        loop {
            let mut fields = vec![("category", category)];
            if let Some(after) = &after {
                fields.push(("after", after.as_str()));
            }
            let answer = self.ask(LESSONS, &fields, false)?;
            let found = &answer["data"]["repository"]["discussions"];
            let page: Vec<Listed> = serde_json::from_value(found["nodes"].clone())
                .context("GitHub listed no discussions")?;
            // Newest first: a page wholly before `since` ends the look.
            let older = page.iter().all(|day| day.title.as_str() < since);
            days.extend(page.into_iter().filter(|day| day.title.as_str() >= since));
            match next_page(found).filter(|_| !older) {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        days.sort_by(|a, b| (&a.title, a.number).cmp(&(&b.title, b.number)));
        days.dedup_by(|later, first| later.title == first.title);
        Ok(days)
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
    if lesson_of(body).is_some() {
        index(discussions, &discussion, &url).with_context(|| {
            format!(
                "the entry {url} was written, but the day's lesson index lacks it; the next \
                 lesson of the day indexes it"
            )
        })?;
    }
    Ok(json!({"url": url, "discussion": discussion.url, "day": day, "created": created}))
}

/// Rebuilds the lesson index of the day's `discussion` from its entries
/// until it holds the entry `url`, a few times at most (the module's doc).
fn index(discussions: &Discussions, discussion: &Day, url: &str) -> Result<()> {
    for _ in 0..3 {
        let (body, lessons) = discussions.read(discussion.number)?;
        if index_of(&body).iter().any(|indexed| indexed.url == url) {
            return Ok(());
        }
        discussions.index(&discussion.id, &indexed(&body, &lessons))?;
    }
    let (body, _) = discussions.read(discussion.number)?;
    match index_of(&body).iter().any(|indexed| indexed.url == url) {
        true => Ok(()),
        false => bail!("three rebuilds of the index each lost it to another writer"),
    }
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
            lesson: None,
            links: vec!["https://x/1".into(), "https://x/2".into()],
        };
        let entry = super::entry("acme/dev", Kind::Worker, &all);
        assert!(entry.contains("### Did\n\nruled\n\n### Why\n\nasked\n\n### Undo\n\nrule again\n"));
        assert!(entry.ends_with("### Links\n\n- https://x/1\n- https://x/2\n"), "{entry}");
        let lesson = Said { lesson: Some("the gate\n needs  jq".into()), ..all.clone() };
        let entry = super::entry("acme/dev", Kind::Worker, &lesson);
        assert!(
            entry
                .contains("### Undo\n\nrule again\n\n### Lesson\n\nthe gate needs jq\n\n### Links")
        );
        assert_eq!(lesson_of(&entry).as_deref(), Some("the gate needs jq"));
        assert_eq!(lesson_of(&super::entry("acme/dev", Kind::Worker, &all)), None);
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
    fn a_body_s_index_is_rebuilt_whole_and_read_back() {
        let intro = "What agents did on 2026-11-03.";
        let one = Indexed { url: "https://x#c-1".into(), lesson: "a: b".into() };
        let two = Indexed { url: "https://x#c-2".into(), lesson: "c".into() };
        let body = indexed(intro, std::slice::from_ref(&one));
        assert_eq!(index_of(intro), vec![]);
        assert_eq!(index_of(&body), vec![one.clone()]);
        let again = indexed(&body, &[one.clone(), two.clone()]);
        assert!(again.starts_with(intro) && again.matches(INDEX).count() == 1, "{again}");
        assert_eq!(index_of(&again), vec![one, two]);
        assert_eq!(indexed(&again, &[]), intro);
    }

    #[test]
    fn a_day_is_said_as_a_date() {
        assert!(day_named("2026-11-03").is_ok());
        for wrong in ["2026-1-03", "20261103", "2026/11/03", "yesterday"] {
            assert!(day_named(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn today_is_a_date() {
        let day = today();
        let bytes = day.as_bytes();
        assert_eq!(bytes.len(), 10, "{day}");
        assert!(bytes[4] == b'-' && bytes[7] == b'-', "{day}");
    }
}

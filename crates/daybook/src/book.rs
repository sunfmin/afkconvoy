//! A daybook's discussions, through `GraphQl`: appending an entry to the
//! day's, settling two creations of it at once, rebuilding and checking its
//! lesson index, and reading a span of days.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::CATEGORY;
use crate::entry::{Indexed, index_of, indexed, lesson_of};

/// One variable of a GraphQL call: text, or an `Int`, sent typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variable<'a> {
    Text(&'a str),
    Int(u64),
}

/// GitHub's GraphQL API, as a program reaches it: its own client, `gh`
/// (`Gh`), or a fake in its tests.
pub trait GraphQl {
    /// `query` with `variables`, answered as GitHub's whole JSON answer,
    /// `errors` included. `writes` says whether it changes anything: a call
    /// that only reads may be made again on a failure in transit, one that
    /// writes is made once.
    fn graphql(&self, query: &str, variables: &[(&str, Variable)], writes: bool) -> Result<Value>;
}

impl<G: GraphQl + ?Sized> GraphQl for &G {
    fn graphql(&self, query: &str, variables: &[(&str, Variable)], writes: bool) -> Result<Value> {
        (**self).graphql(query, variables, writes)
    }
}

/// The arguments of `gh` that make the call: `api graphql`, each variable
/// as a field (`-f`, an `Int` typed with `-F`), in their order, then the
/// query.
pub fn gh_args(query: &str, variables: &[(&str, Variable)]) -> Vec<String> {
    let mut args = vec!["api".to_string(), "graphql".to_string()];
    for (name, value) in variables {
        let (flag, value) = match value {
            Variable::Text(text) => ("-f", text.to_string()),
            Variable::Int(int) => ("-F", int.to_string()),
        };
        args.extend([flag.to_string(), format!("{name}={value}")]);
    }
    args.extend(["-f".to_string(), format!("query={query}")]);
    args
}

/// `gh api graphql`, run once a call in `dir` with the user's own login.
#[derive(Debug, Clone)]
pub struct Gh {
    pub dir: PathBuf,
}

impl GraphQl for Gh {
    fn graphql(&self, query: &str, variables: &[(&str, Variable)], _writes: bool) -> Result<Value> {
        let out = Command::new("gh")
            .args(gh_args(query, variables))
            .current_dir(&self.dir)
            .output()
            .context("cannot run gh")?;
        let answer = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() && answer.trim().is_empty() {
            bail!("gh api graphql failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        serde_json::from_str(&answer).context("GitHub's answer is no JSON")
    }
}

/// An appended entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended {
    /// The entry's comment.
    pub url: String,
    /// The day's discussion.
    pub discussion: String,
    /// Its title.
    pub day: String,
    /// Whether this append created it.
    pub created: bool,
}

/// One lesson of a span, and the day it is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    pub day: String,
    pub url: String,
    pub lesson: String,
}

/// What a span of a daybook holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub lessons: usize,
    /// Its entries that carry no lesson.
    pub unlessoned: usize,
}

/// The daybook in the Discussions of the repo `home`, `owner/name`.
pub struct Daybook<G> {
    github: G,
    owner: String,
    name: String,
    /// The program a day's discussion says wrote it.
    written_by: String,
}

/// One day's discussion, as a listing reads it.
#[derive(Debug, Clone, Deserialize)]
struct Day {
    id: String,
    number: u64,
    title: String,
    url: String,
}

/// One day's discussion as a span's listing reads it.
#[derive(Debug, Clone, Deserialize)]
struct Listed {
    number: u64,
    title: String,
    body: String,
    /// How many entries it holds, where the listing counted them.
    #[serde(default)]
    comments: Option<Counted>,
}

#[derive(Debug, Clone, Deserialize)]
struct Counted {
    #[serde(rename = "totalCount")]
    total: usize,
}

/// The cursor of the page after the one `connection` holds, where there is one.
fn next_page(connection: &Value) -> Option<String> {
    let info = &connection["pageInfo"];
    info["hasNextPage"].as_bool().filter(|more| *more)?;
    info["endCursor"].as_str().map(str::to_string)
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

/// A page of a category's latest discussions with their bodies and how many
/// entries each holds.
const SPAN: &str = "query ConvoyDaybookSpan($owner: String!, $repo: String!, \
    $category: ID!, $after: String) { repository(owner: $owner, name: $repo) { \
    discussions(first: 100, after: $after, categoryId: $category, \
    orderBy: {field: CREATED_AT, direction: DESC}) { nodes { number title body \
    comments { totalCount } } pageInfo { hasNextPage endCursor } } } }";

const ENTRY: &str = "mutation ConvoyDaybookEntry($discussion: ID!, $body: String!) { \
    addDiscussionComment(input: {discussionId: $discussion, body: $body}) { comment { url } } }";

impl<G: GraphQl> Daybook<G> {
    /// The daybook of the repo `home`, `owner/name`, reached through
    /// `github`; a day's discussion it creates says `written_by` wrote it.
    pub fn new(github: G, home: &str, written_by: &str) -> Result<Daybook<G>> {
        let Some((owner, name)) =
            home.split_once('/').filter(|(o, n)| !o.is_empty() && !n.is_empty())
        else {
            bail!("`{home}` names no repo: say it `owner/name`");
        };
        Ok(Daybook {
            github,
            owner: owner.to_string(),
            name: name.to_string(),
            written_by: written_by.to_string(),
        })
    }

    fn home(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// `query` with `fields`, answered as JSON. A read names the repo
    /// first; GitHub's `errors` are an error.
    fn ask(&self, query: &str, fields: &[(&str, Variable)], writes: bool) -> Result<Value> {
        let named = [("owner", Variable::Text(&self.owner)), ("repo", Variable::Text(&self.name))];
        let variables: Vec<(&str, Variable)> =
            named.iter().filter(|_| !writes).chain(fields).copied().collect();
        let answer = self.github.graphql(query, &variables, writes)?;
        if let Some(errors) = answer.get("errors").filter(|errors| !errors.is_null()) {
            bail!("GitHub refused it: {errors}");
        }
        Ok(answer)
    }

    /// The repo's id and its `Daybook` category's; an error naming what
    /// makes them where it has no such category.
    fn place(&self) -> Result<(String, String)> {
        let answer = self.ask(PLACE, &[], false)?;
        let repository = &answer["data"]["repository"];
        let home = self.home();
        let Some(id) = repository["id"].as_str() else {
            bail!("GitHub answered no repository {home}");
        };
        let create = format!(
            "create the category at https://github.com/{home}/discussions/categories/new, \
             named `{CATEGORY}` (GitHub creates none through its API); nothing was written"
        );
        if repository["hasDiscussionsEnabled"] == json!(false) {
            bail!(
                "the tracker {home} has no discussions, so no `{CATEGORY}` category: turn them \
                 on with `gh repo edit {home} --enable-discussions`, then {create}"
            );
        }
        let categories = repository["discussionCategories"]["nodes"].as_array();
        let category = categories.into_iter().flatten().find(|category| {
            category["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(CATEGORY))
        });
        match category.and_then(|category| category["id"].as_str()) {
            Some(category) => Ok((id.to_string(), category.to_string())),
            None => {
                bail!("the tracker {home} has no discussion category `{CATEGORY}`: {create}")
            }
        }
    }

    /// The discussion of `day` in `category` with the lowest number, of
    /// those the look `name` sees.
    fn day(&self, name: &str, category: &str, day: &str) -> Result<Option<Day>> {
        let answer =
            self.ask(&days_query(name), &[("category", Variable::Text(category))], false)?;
        let nodes = answer["data"]["repository"]["discussions"]["nodes"].clone();
        let days: Vec<Day> =
            serde_json::from_value(nodes).context("GitHub listed no discussions")?;
        Ok(days.into_iter().filter(|found| found.title == day).min_by_key(|found| found.number))
    }

    fn open(&self, repository: &str, category: &str, day: &str) -> Result<Day> {
        let body = format!(
            "What agents did and decided outside any issue on {day}, one comment an entry. \
             Written by `{}`; nothing reads it to decide what a fleet does.",
            self.written_by
        );
        let fields = [
            ("repository", Variable::Text(repository)),
            ("category", Variable::Text(category)),
            ("title", Variable::Text(day)),
            ("body", Variable::Text(&body)),
        ];
        let answer = self.ask(OPEN, &fields, true)?;
        serde_json::from_value(answer["data"]["createDiscussion"]["discussion"].clone())
            .context("GitHub answered no discussion it created")
    }

    fn drop(&self, id: &str) -> Result<()> {
        self.ask(DROP, &[("id", Variable::Text(id))], true).map(|_| ())
    }

    /// The body of the day's discussion `number` and the lessons its
    /// entries carry, in their order.
    fn read(&self, number: u64) -> Result<(String, Vec<Indexed>)> {
        let (mut lessons, mut after) = (Vec::new(), None::<String>);
        loop {
            let mut fields = vec![("number", Variable::Int(number))];
            if let Some(after) = &after {
                fields.push(("after", Variable::Text(after)));
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

    fn write_index(&self, discussion: &str, body: &str) -> Result<()> {
        let fields = [("discussion", Variable::Text(discussion)), ("body", Variable::Text(body))];
        self.ask(INDEXES, &fields, true).map(|_| ())
    }

    /// The discussions of `category` titled `since` or later, the lowest
    /// numbered of each title, oldest day first.
    fn days_since(&self, query: &str, category: &str, since: &str) -> Result<Vec<Listed>> {
        let mut days: Vec<Listed> = Vec::new();
        let mut after = None::<String>;
        loop {
            let mut fields = vec![("category", Variable::Text(category))];
            if let Some(after) = &after {
                fields.push(("after", Variable::Text(after)));
            }
            let answer = self.ask(query, &fields, false)?;
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

    /// The days' discussions of the span `since` to `until`, both included,
    /// as `query` lists them.
    fn span(&self, query: &str, since: &str, until: Option<&str>) -> Result<Vec<Listed>> {
        let (_, category) = self.place()?;
        Ok(self
            .days_since(query, &category, since)?
            .into_iter()
            .filter(|day| until.is_none_or(|until| day.title.as_str() <= until))
            .collect())
    }

    fn comment(&self, discussion: &str, body: &str) -> Result<String> {
        let fields = [("discussion", Variable::Text(discussion)), ("body", Variable::Text(body))];
        let answer = self.ask(ENTRY, &fields, true)?;
        answer["data"]["addDiscussionComment"]["comment"]["url"]
            .as_str()
            .map(str::to_string)
            .context("GitHub answered no comment it added")
    }

    /// Appends `entry` to the discussion of `day`, creating it where there
    /// is none and settling a creation that raced another (the crate's doc);
    /// an entry with a lesson then has the day's lesson index rebuilt until
    /// it holds it.
    pub fn append(&self, day: &str, entry: &str) -> Result<Appended> {
        let (repository, category) = self.place()?;
        let (discussion, created) = match self.day("ConvoyDaybookDay", &category, day)? {
            Some(found) => (found, false),
            None => {
                let mine = self.open(&repository, &category, day)?;
                let oldest = self.day("ConvoyDaybookSettle", &category, day)?;
                match oldest.filter(|oldest| oldest.number < mine.number) {
                    Some(oldest) => {
                        self.drop(&mine.id).with_context(|| {
                            format!("{} is a second discussion of {day}; delete it", mine.url)
                        })?;
                        (oldest, false)
                    }
                    None => (mine, true),
                }
            }
        };
        let url = self.comment(&discussion.id, entry)?;
        if lesson_of(entry).is_some() {
            self.index(&discussion, &url).with_context(|| {
                format!(
                    "the entry {url} was written, but the day's lesson index lacks it; the next \
                     lesson of the day indexes it"
                )
            })?;
        }
        Ok(Appended { url, discussion: discussion.url, day: day.to_string(), created })
    }

    /// Rebuilds the lesson index of the day's `discussion` from its entries
    /// until it holds the entry `url`, a few times at most (the crate's doc).
    fn index(&self, discussion: &Day, url: &str) -> Result<()> {
        for _ in 0..3 {
            let (body, lessons) = self.read(discussion.number)?;
            if index_of(&body).iter().any(|indexed| indexed.url == url) {
                return Ok(());
            }
            self.write_index(&discussion.id, &indexed(&body, &lessons))?;
        }
        let (body, _) = self.read(discussion.number)?;
        match index_of(&body).iter().any(|indexed| indexed.url == url) {
            true => Ok(()),
            false => bail!("three rebuilds of the index each lost it to another writer"),
        }
    }

    /// The lessons of the days `since` to `until`, both included (every day
    /// from `since` where `until` is none), from the days' indexes, oldest
    /// day first, with no entry fetched.
    pub fn lessons(&self, since: &str, until: Option<&str>) -> Result<Vec<Lesson>> {
        Ok(self
            .span(LESSONS, since, until)?
            .into_iter()
            .flat_map(|day| {
                index_of(&day.body).into_iter().map(move |Indexed { url, lesson }| Lesson {
                    day: day.title.clone(),
                    url,
                    lesson,
                })
            })
            .collect())
    }

    /// What the days `since` to `until` hold: the days' indexes and how many
    /// entries each day holds, with no entry fetched.
    pub fn held(&self, since: &str, until: &str) -> Result<Held> {
        let mut held = Held { lessons: 0, unlessoned: 0 };
        for day in self.span(SPAN, since, Some(until))? {
            let lessons = index_of(&day.body).len();
            let entries = day.comments.map_or(0, |counted| counted.total);
            held.lessons += lessons;
            held.unlessoned += entries.saturating_sub(lessons);
        }
        Ok(held)
    }
}

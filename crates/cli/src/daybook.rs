//! `afkconvoy daybook`: one entry appended to the **daybook**, the operator's
//! one place to read, after the fact, what agents did and decided outside any
//! issue. Its home is the tracker's GitHub Discussions, category `Daybook`:
//! one discussion a day, titled with the host's local date, one comment an
//! entry. It is not fleet state: nothing reads it to decide what the fleet
//! does.
//!
//! What a daybook is — the entry's shape and its lesson, the day's
//! discussion and the lowest numbered winning two created at once, the
//! lesson index — is the daybook crate's (`afkconvoy-daybook`, ADR-0071),
//! which another program uses too. What is afkconvoy's own is here: the
//! kind of session its environment tells, the repo and tracker a checkout
//! and this host's checkout list name, `gh` as the fleet runs it, and the
//! weekly distillation's file.

use std::path::PathBuf;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::host_watch::WATCH_ENV;
use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::role::{ISSUE_ENV, ROLE_ENV};
pub use afkconvoy_daybook::{
    CATEGORY, Held, Indexed, Said, day_at, day_named, index_of, lesson_of, next_day,
};
use afkconvoy_daybook::{Daybook, GraphQl, Variable, gh_args, one_line};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::gh::{GitHub, Tracker};
use crate::git::Repo;

/// Marks the asking session's processes, beside the watch's own marker
/// (`AFKCONVOY_WATCH=host`) the two share.
pub const ASKING_ENV: &str = "AFKCONVOY_ASKING";

/// What a day's discussion says wrote it.
const WRITTEN_BY: &str = "afkconvoy daybook";

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

/// The one shape of an entry: who wrote it, then each heading said.
pub fn entry(repo: &str, kind: Kind, said: &Said) -> String {
    afkconvoy_daybook::entry(repo, kind.word(), said)
}

/// Today on this host, `YYYY-MM-DD` in its local time, by the fleet's clock.
pub fn today() -> String {
    day_at(crate::clock())
}

/// `gh api graphql` as the fleet runs it in a checkout: a read made again on
/// a failure in transit, a write made once.
struct Gh(GitHub);

impl GraphQl for Gh {
    fn graphql(&self, query: &str, variables: &[(&str, Variable)], writes: bool) -> Result<Value> {
        let args = gh_args(query, variables);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = if writes { self.0.gh(&args)? } else { self.0.read(&args)? };
        serde_json::from_str(&out).context("GitHub's answer is no JSON")
    }
}

/// The daybook of `tracker`, read and written with `gh` in `dir`.
fn book(dir: PathBuf, tracker: &RepoName) -> Result<Daybook<Gh>> {
    Daybook::new(Gh(GitHub::new(dir, Tracker::Own)), tracker.as_str(), WRITTEN_BY)
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
    if said.is_empty() {
        bail!(
            "an entry says nothing: give its text, or one of `--did`, `--why`, `--undo`, \
             `--lesson`, `--link`"
        );
    }
    let whose = Whose::of(checkout, repo, tracker)?;
    let kind = Kind::of(|name| std::env::var(name).ok());
    whose.append(&today(), kind, &said)
}

/// Whose daybook an entry goes to, and of which repo it is.
pub struct Whose {
    /// Where `gh` runs.
    pub dir: PathBuf,
    pub repo: RepoName,
    pub tracker: RepoName,
}

impl Whose {
    /// The repo `repo` names, else the checkout's; the tracker `tracker`
    /// names, else the one this host's checkout list says for the checkout,
    /// else its own repo.
    pub fn of(
        checkout: Result<Repo>,
        repo: Option<RepoName>,
        tracker: Option<RepoName>,
    ) -> Result<Whose> {
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
            (None, None) => {
                bail!("{none}: name the tracker whose daybook it is with `--tracker`")
            }
        };
        let dir = checkout.map_or_else(std::env::temp_dir, |checkout| checkout.dir);
        Ok(Whose { dir, repo, tracker })
    }

    /// Appends the entry `said` by a session of `kind` to the discussion of
    /// `day` → {url, discussion, day, created}.
    pub fn append(&self, day: &str, kind: Kind, said: &Said) -> Result<Value> {
        let body = entry(self.repo.as_str(), kind, said);
        let appended = book(self.dir.clone(), &self.tracker)?.append(day, &body)?;
        Ok(json!({
            "url": appended.url,
            "discussion": appended.discussion,
            "day": appended.day,
            "created": appended.created,
        }))
    }
}

/// The checkout's directory, where `gh` runs, and the tracker whose daybook
/// it is: `tracker`, or the one this host's checkout list says for the
/// checkout, its own repo where it says none.
fn whose(checkout: Result<Repo>, tracker: Option<RepoName>) -> Result<(PathBuf, RepoName)> {
    match (tracker, checkout) {
        (Some(tracker), checkout) => {
            Ok((checkout.map_or_else(|_| std::env::temp_dir(), |checkout| checkout.dir), tracker))
        }
        (None, Ok(checkout)) => {
            let list = crate::checkout_list::List::of_host()?;
            let tracker = match crate::checkout_list::said_tracker(&list, &checkout)? {
                Some(tracker) => tracker,
                None => crate::checkout_list::named_repo(&checkout)?,
            };
            Ok((checkout.dir, tracker))
        }
        (None, Err(error)) => {
            bail!("{error:#}: name the tracker whose daybook it is with `--tracker`")
        }
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
    let (dir, tracker) = whose(checkout, tracker)?;
    let lessons: Vec<Value> = book(dir, &tracker)?
        .lessons(since, until)?
        .into_iter()
        .map(|found| json!({"day": found.day, "url": found.url, "lesson": found.lesson}))
        .collect();
    Ok(json!({"lessons": lessons, "since": since, "until": until}))
}

/// What the days `since` to `until` of `tracker`'s daybook hold, read with
/// `gh` in `dir`: the days' indexes and how many entries each day holds,
/// with no entry fetched.
pub fn held(dir: PathBuf, tracker: &RepoName, since: &str, until: &str) -> Result<Held> {
    book(dir, tracker)?.held(since, until)
}

/// Where a distillation carried one lesson, or one pattern recurring across
/// entries that carry none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Home {
    /// A ticket to the repo that owns it: a mechanism, a project's ADR or
    /// GLOSSARY, a tool's own skill.
    Ticket,
    /// A change to the control repo's own ADR or GLOSSARY.
    Doc,
    /// A draft PR on the operator's global principles, for them to read.
    Draft,
    /// Its home holds it already.
    Covered,
}

impl Home {
    fn said(self) -> &'static str {
        match self {
            Home::Ticket => "ticket",
            Home::Doc => "control doc change",
            Home::Draft => "draft PR, for the operator to read",
            Home::Covered => "already covered",
        }
    }
}

/// One lesson, or one recurring pattern, and where it went.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Carried {
    /// The entries it was read from: a lesson's own, or a pattern's.
    entries: Vec<String>,
    /// What it says; a lesson's is its index's where this says none.
    #[serde(default)]
    lesson: Option<String>,
    home: Home,
    /// The ticket, change, draft PR, or where its home already holds it.
    link: String,
}

/// A distillation's file: its span and where each of its lessons went.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Distillation {
    since: String,
    until: String,
    carried: Vec<Carried>,
}

/// Checks a distillation's file against the lessons of its span, the
/// lesson index of each day from `since` to `until`: every lesson carried
/// once, and a pattern carried from two entries at least → the entry's
/// `Did`, or what is wrong.
fn carried(distillation: &Distillation, lessons: &[(String, Indexed)]) -> Result<String> {
    let Distillation { since, until, carried } = distillation;
    let mut lines = Vec::new();
    for (at, carried) in carried.iter().enumerate() {
        let Carried { entries, lesson, home, link } = carried;
        let at = at + 1;
        let web = |url: &str| {
            url.strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .is_some_and(|tail| !tail.is_empty() && !tail.chars().any(char::is_whitespace))
        };
        if !web(link) {
            bail!("carried {at}: its link must be an HTTP or HTTPS URL: {link:?}");
        }
        if entries.is_empty() || !entries.iter().all(|entry| web(entry)) {
            bail!("carried {at}: name the entries it was read from, each by its URL");
        }
        let indexed: Vec<&Indexed> = lessons
            .iter()
            .filter(|(_, indexed)| entries.contains(&indexed.url))
            .map(|(_, indexed)| indexed)
            .collect();
        let said = lesson.as_deref().map(one_line).filter(|said| !said.is_empty());
        let said = match (said, indexed.first()) {
            (Some(said), _) => said,
            (None, Some(indexed)) => indexed.lesson.clone(),
            (None, None) => bail!("carried {at}: a pattern says its `lesson`"),
        };
        if indexed.is_empty() && entries.len() < 2 {
            bail!(
                "carried {at}: it names no lesson of {since} to {until}, so it is a pattern, \
                 which recurs across two entries at least"
            );
        }
        let read: Vec<String> = entries.iter().map(|entry| format!("[entry]({entry})")).collect();
        lines.push(format!("- {said} ({}) → {}: {link}", read.join(", "), home.said()));
    }
    for (day, Indexed { url, lesson }) in lessons {
        let times = carried.iter().filter(|carried| carried.entries.contains(url)).count();
        match times {
            1 => {}
            0 => bail!("the lesson of {day} {url} is carried nowhere: {lesson}"),
            _ => bail!("the lesson of {day} {url} is carried {times} times: carry it once"),
        }
    }
    let did = match lines.is_empty() {
        true => format!("Distilled the daybook of {since} to {until}: it held nothing to carry."),
        false => format!(
            "Distilled the daybook of {since} to {until}: each lesson, and where it went.\n\n{}",
            lines.join("\n")
        ),
    };
    Ok(did)
}

/// Records a distillation, said in the file `file`: checks it carries each
/// lesson of its span exactly once, then appends one entry to today's
/// discussion mapping each lesson to where it went → the entry's answer, with
/// the span and how many were carried.
pub fn distilled(
    checkout: Result<Repo>,
    repo: Option<RepoName>,
    tracker: Option<RepoName>,
    file: &std::path::Path,
) -> Result<Value> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read the distillation {}", file.display()))?;
    let said: Distillation = serde_json::from_str(&text).context(
        "a distillation is one JSON object: {\"since\", \"until\", \"carried\": [{\"entries\", \
         \"lesson\", \"home\", \"link\"}]}",
    )?;
    let copy = checkout.as_ref().map(Repo::clone).map_err(|error| anyhow::anyhow!("{error:#}"));
    let (dir, tracker) = whose(copy, tracker)?;
    for day in [&said.since, &said.until] {
        day_named(day).map_err(anyhow::Error::msg)?;
    }
    let lessons: Vec<(String, Indexed)> = book(dir, &tracker)?
        .lessons(&said.since, Some(&said.until))?
        .into_iter()
        .map(|found| (found.day, Indexed { url: found.url, lesson: found.lesson }))
        .collect();
    let did = carried(&said, &lessons)?;
    let entry = Said { did: Some(did), ..Said::default() };
    let mut answer = daybook(checkout, Asked { said: entry, repo, tracker: Some(tracker) })?;
    answer["since"] = json!(said.since);
    answer["until"] = json!(said.until);
    answer["carried"] = json!(said.carried.len());
    answer["lessons"] = json!(lessons.len());
    Ok(answer)
}

/// What `afkconvoy daybook --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When an agent — an interactive session, the watch, the asking session or a worker — did or decided something no issue holds: a host fix or install, a ruling made without a ticket, a change made at the operator's word. Record it and carry on: the operator reads the daybook when they choose, and nobody waits on it. What belongs to an issue goes on the issue, not here.

## Who runs it

The operator, or any agent acting on the host for them: the agent a human is talking to, the watch, the asking session, or a worker, in its own worktree. Each says what it did in its own entry; none reads the daybook to decide what to do next.

## What it reads and writes

It reads the tracker's discussion categories and its latest discussions of `Daybook`, and this host's checkout list for the tracker.

One comment on today's discussion of the tracker, in its discussion category `Daybook`: one discussion a day, titled with the host's local date, `YYYY-MM-DD`, created by the first entry of the day. Two entries that both find no discussion of the day both still go to one: the discussion of the day with the lowest number is the day's, and a run that created a second deletes its own.

Every entry has one shape: the repo it is of and the kind of session that wrote it — `interactive`, `watch`, `asking session` or `worker`, told by its environment — then the headings `Did`, `Why`, `Undo`, `Lesson` and `Links`, each holding what was said for it. A heading said of nothing is left out. The text given alone goes under `Did`.

A **lesson** (`--lesson`) is one sentence the agent, at the moment, thinks should outlast the day: "the gate on this host needs X", "this kind of edit breaks Y". Writing it does not interrupt the work: it is raw material for the weekly distillation, which carries each lesson to its lasting home — a check, a hook, a rule, a doc. An entry with a lesson also rebuilds the lesson index at the foot of the day's discussion body — each lesson of the day and its entry's link — from the day's entries, so `--lessons` lists a span's lessons from the days' bodies without fetching an entry.

`--lessons --since <YYYY-MM-DD> [--until <YYYY-MM-DD>]` writes nothing: it lists the lessons of the days from `--since` to `--until`, both included (to today where `--until` is not given), from the days' indexes.

The **distillation** is the host's watch's: at most once a week, when a span of days holds a lesson or two entries without one — enough for a fault to recur — a watch round reads the span's lessons and entries, decides each lesson's lasting home and proposes it there through that home's normal path: a repeated mistake first as a mechanism, a ticket to the repo that owns it; one project's decision or term, a ticket for its ADR or GLOSSARY; a cross-project one, a change to the control repo's; how to use a tool, a ticket for its own skill; the operator's global principles, a draft PR they read and no fleet merges. It keeps nothing of its own: the proposal is the record, and the entries are left to age out. `--distill` asks the watch for one at its next look without waiting out the week; it still runs only on a span that holds something, from the day after the last distillation's span to today.

`--distilled <file>` is how the round records it: the JSON file says {"since", "until", "carried": [{"entries": [entry URL…], "lesson", "home", "link"}]}, `home` one of `ticket`, `doc` (a control ADR or GLOSSARY change), `draft` (a draft PR on the global principles) or `covered` (its home holds it already, `link` saying where). Each lesson of the days `since` to `until` is carried exactly once, by its entry's URL; its `lesson` may be left out. A pattern recurring across entries without a lesson names two entries at least and says its `lesson`. The file is checked against the span's lesson indexes before anything is written, then one entry is written whose `Did` maps each lesson to where it went.

The repo is the one the checkout's remote names, or `--repo`. The tracker is the one this host's checkout list says for the checkout, its own repo where it says none, or `--tracker`. Run in no checkout, both flags are needed.

It is not fleet state: nothing reads it to decide what a fleet does.

## What it answers

{"url", "discussion", "day", "created"}

- `url`: the entry's comment
- `discussion`: the day's discussion
- `day`: its title, the host's local date
- `created`: whether this run created it

With `--lessons`: {"lessons": [{"day", "url", "lesson"}], "since", "until"}, oldest day first.

With `--distilled`: the entry's answer, and "since", "until", "lessons" (how many the span holds) and "carried" (how many lessons and patterns the file carried). With `--distill`: {"asked", "at"}, the epoch second the watch was asked at.

Errors, as {"error": …} with a non-zero exit, each before anything is written but the last: an entry that says nothing; a distillation file that carries a lesson of its span nowhere or twice, or a pattern with no `lesson` or fewer than two entries; no repo or tracker to name, run in no checkout; the tracker has no `Daybook` category — the error names the page that creates one, as GitHub creates no category through its API, and the line that turns its discussions on where they are off; GitHub refused to read or create the discussion, or to add the comment.

## What trips you up

- The category is created by hand, once, in the tracker: https://github.com/<owner>/<tracker>/discussions/categories/new, named `Daybook`.
- An entry is written once: a second run writes a second entry. To correct one, edit its comment on GitHub; to correct its lesson in the index too, edit the index line in the day's discussion body, or write the day's next lesson, which rebuilds the index from the entries.
- Two lessons written at once can each rebuild an index the other's is missing from. Each run checks the index holds its own lesson and rebuilds it until it does, and the day's next lesson restores any other; an index that still lacks a run's lesson is an error naming the entry, which was written."#;

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
    fn today_is_a_date() {
        let day = today();
        let bytes = day.as_bytes();
        assert_eq!(bytes.len(), 10, "{day}");
        assert!(bytes[4] == b'-' && bytes[7] == b'-', "{day}");
    }
}

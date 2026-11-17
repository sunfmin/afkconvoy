//! `afkconvoy daybook`: one entry appended to the **daybook**, the operator's
//! one place to read, after the fact, what agents did and decided outside any
//! issue. Its home is the host's own (docs/adr/0118): `daybook.db` in
//! afkconvoy's data directory, a row an entry. It is not fleet state:
//! nothing reads it to decide what the fleet does.
//!
//! What a daybook is — the entry's shape and its lesson, the day, the
//! database and its schema, reading a span — is the daybook crate's
//! (`afkconvoy-daybook`, ADR-0071), which another program uses too. What is
//! afkconvoy's own is here: the kind of session its environment tells, the
//! repo a checkout names, where the host's data directory is, and the weekly
//! distillation's file.

use std::path::PathBuf;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::host_watch::WATCH_ENV;
use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::role::{ISSUE_ENV, ROLE_ENV};
pub use afkconvoy_daybook::{Held, Said, day_at, day_named, lesson_of, next_day};
use afkconvoy_daybook::{Lesson, Local, Store, one_line};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::git::Repo;

/// Marks the asking session's processes, beside the watch's own marker
/// (`AFKCONVOY_WATCH=host`) the two share.
pub const ASKING_ENV: &str = "AFKCONVOY_ASKING";

/// The daybook's name in the data directory.
const FILE: &str = "daybook.db";

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

/// Where this host keeps its daybook: `daybook.db` in afkconvoy's data
/// directory, beside its local tracker.
pub fn store_path() -> Result<PathBuf> {
    Ok(crate::issues::local::data_dir()?.join(FILE))
}

/// This host's daybook, made where there is none.
fn book() -> Result<Local> {
    Local::open(&store_path()?)
}

/// What an entry says, and of which repo it is.
pub struct Asked {
    pub said: Said,
    pub repo: Option<RepoName>,
}

/// Appends one entry, said in `asked`, to today's entries of this host's
/// daybook → {url, day, at}.
pub fn daybook(checkout: Result<Repo>, asked: Asked) -> Result<Value> {
    let Asked { said, repo } = asked;
    if said.is_empty() {
        bail!(
            "an entry says nothing: give its text, or one of `--did`, `--why`, `--undo`, \
             `--lesson`, `--link`"
        );
    }
    let repo = repo_of(checkout, repo)?;
    let kind = Kind::of(|name| std::env::var(name).ok());
    let at = crate::acting_now();
    append(&day_at(at), at, &repo, kind, &said)
}

/// The repo an entry is of: the one `repo` names, else the checkout's.
pub fn repo_of(checkout: Result<Repo>, repo: Option<RepoName>) -> Result<RepoName> {
    match (repo, checkout) {
        (Some(repo), _) => Ok(repo),
        (None, Ok(checkout)) => crate::checkout_list::named_repo(&checkout),
        (None, Err(error)) => bail!("{error:#}: name the repo the entry is of with `--repo`"),
    }
}

/// Appends the entry `said`, of `repo` by a session of `kind`, to the day
/// `day`, written at `at` → {url, day, at}.
pub fn append(day: &str, at: u64, repo: &RepoName, kind: Kind, said: &Said) -> Result<Value> {
    let url = book()?.write(day, at, &entry(repo.as_str(), kind, said))?;
    Ok(json!({"url": url, "day": day, "at": at}))
}

/// The span `--read` reads: the day `day`, the days `since` to `until`
/// (to today where `until` is none), or today where neither is said.
pub fn read(day: Option<&str>, since: Option<&str>, until: Option<&str>) -> Result<Value> {
    let today = day_at(crate::acting_now());
    let since = day.or(since).unwrap_or(&today);
    let until = day.or(until).unwrap_or(&today);
    let entries: Vec<Value> = book()?
        .entries(since, until)?
        .into_iter()
        .map(|found| {
            json!({"url": found.url, "day": found.day, "at": found.at,
                "lesson": lesson_of(&found.text), "text": found.text})
        })
        .collect();
    Ok(json!({"entries": entries, "since": since, "until": until}))
}

/// The lessons of the days `since` to `until`, both included, read from
/// their entries → {lessons: [{day, url, lesson}], since, until}.
pub fn lessons(since: &str, until: Option<&str>) -> Result<Value> {
    let lessons: Vec<Value> = book()?
        .lessons(since, until)?
        .into_iter()
        .map(|found| json!({"day": found.day, "url": found.url, "lesson": found.lesson}))
        .collect();
    Ok(json!({"lessons": lessons, "since": since, "until": until}))
}

/// What the days `since` to `until` of this host's daybook hold: how many
/// lessons, and how many entries carry none.
pub fn held(since: &str, until: &str) -> Result<Held> {
    book()?.held(since, until)
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
    /// The entries it was read from, each by its `url`: a lesson's own, or
    /// a pattern's.
    entries: Vec<String>,
    /// What it says; a lesson's is its entry's where this says none.
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

/// Checks a distillation's file against its span: the `urls` of its
/// entries and their `lessons`. Every lesson carried once, every entry
/// named one of the span's, and a pattern carried from two entries at least
/// → the entry's `Did`, or what is wrong.
fn carried(distillation: &Distillation, urls: &[String], lessons: &[Lesson]) -> Result<String> {
    let Distillation { since, until, carried } = distillation;
    let mut lines = Vec::new();
    for (at, carried) in carried.iter().enumerate() {
        let Carried { entries, lesson, home, link } = carried;
        let at = at + 1;
        let web = link
            .strip_prefix("https://")
            .or_else(|| link.strip_prefix("http://"))
            .is_some_and(|tail| !tail.is_empty() && !tail.chars().any(char::is_whitespace));
        if !web {
            bail!("carried {at}: its link must be an HTTP or HTTPS URL: {link:?}");
        }
        if entries.is_empty() {
            bail!("carried {at}: name the entries it was read from, each by its `url`");
        }
        if let Some(stranger) = entries.iter().find(|entry| !urls.contains(entry)) {
            bail!(
                "carried {at}: {stranger} is no entry of {since} to {until}: name each by the \
                 `url` `afkconvoy daybook --read --since {since} --until {until}` gives it"
            );
        }
        let indexed: Vec<&Lesson> =
            lessons.iter().filter(|found| entries.contains(&found.url)).collect();
        let said = lesson.as_deref().map(one_line).filter(|said| !said.is_empty());
        let said = match (said, indexed.first()) {
            (Some(said), _) => said,
            (None, Some(found)) => found.lesson.clone(),
            (None, None) => bail!("carried {at}: a pattern says its `lesson`"),
        };
        if indexed.is_empty() && entries.len() < 2 {
            bail!(
                "carried {at}: it names no lesson of {since} to {until}, so it is a pattern, \
                 which recurs across two entries at least"
            );
        }
        let read: Vec<String> = entries.iter().map(|entry| format!("`{entry}`")).collect();
        lines.push(format!("- {said} ({}) → {}: {link}", read.join(", "), home.said()));
    }
    for Lesson { day, url, lesson } in lessons {
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
/// mapping each lesson to where it went → the entry's answer, with the span
/// and how many were carried.
pub fn distilled(
    checkout: Result<Repo>,
    repo: Option<RepoName>,
    file: &std::path::Path,
) -> Result<Value> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read the distillation {}", file.display()))?;
    let said: Distillation = serde_json::from_str(&text).context(
        "a distillation is one JSON object: {\"since\", \"until\", \"carried\": [{\"entries\", \
         \"lesson\", \"home\", \"link\"}]}",
    )?;
    for day in [&said.since, &said.until] {
        day_named(day).map_err(anyhow::Error::msg)?;
    }
    let book = book()?;
    let urls: Vec<String> =
        book.entries(&said.since, &said.until)?.into_iter().map(|found| found.url).collect();
    let lessons = book.lessons(&said.since, Some(&said.until))?;
    let did = carried(&said, &urls, &lessons)?;
    let entry = Said { did: Some(did), ..Said::default() };
    let mut answer = daybook(checkout, Asked { said: entry, repo })?;
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

The daybook is this host's own: `daybook.db`, one SQLite database in afkconvoy's data directory — `$XDG_DATA_HOME/afkconvoy`, else `~/.local/share/afkconvoy` — beside the host's local tracker, made by the first entry. It asks GitHub nothing. An entry is written at once, or the command fails and nothing is written; there is nothing to retry later. Another program of the host's user (Guild) may write the same file at once: neither loses an entry.

An entry is one row: its day — the host's local date, `YYYY-MM-DD` — when it was written, and its text. A day is the entries of that date, in the order they were written.

Every entry has one shape: the repo it is of and the kind of session that wrote it — `interactive`, `watch`, `asking session` or `worker`, told by its environment — then the headings `Did`, `Why`, `Undo`, `Lesson` and `Links`, each holding what was said for it. A heading said of nothing is left out. The text given alone goes under `Did`.

A **lesson** (`--lesson`) is one sentence the agent, at the moment, thinks should outlast the day: "the gate on this host needs X", "this kind of edit breaks Y". Writing it does not interrupt the work: it is raw material for the weekly distillation, which carries each lesson to its lasting home — a check, a hook, a rule, a doc. A day's lessons are read from its entries.

`--read` writes nothing: it prints today's entries; `--day <YYYY-MM-DD>` one day's; `--since <YYYY-MM-DD> [--until <YYYY-MM-DD>]` the days from `--since` to `--until`, both included (to today where `--until` is not given). This is how a human reads the daybook: on the host that wrote it.

`--lessons --since <YYYY-MM-DD> [--until <YYYY-MM-DD>]` writes nothing: it lists the lessons of the days from `--since` to `--until`, both included (to today where `--until` is not given).

The **distillation** is the host's watch's: at most once a week, when a span of days holds a lesson or two entries without one — enough for a fault to recur — a watch round reads the span's lessons and entries, decides each lesson's lasting home and proposes it there through that home's normal path: a repeated mistake first as a mechanism, a ticket to the repo that owns it; one project's decision or term, a ticket for its ADR or GLOSSARY; a cross-project one, a change to the control repo's; how to use a tool, a ticket for its own skill; the operator's global principles, a draft PR they read and no fleet merges. It keeps nothing of its own: the proposal is the record, and the entries are left to age out. `--distill` asks the watch for one at its next look without waiting out the week; it still runs only on a span that holds something, from the day after the last distillation's span to today.

`--distilled <file>` is how the round records it: the JSON file says {"since", "until", "carried": [{"entries": [entry url…], "lesson", "home", "link"}]}, `home` one of `ticket`, `doc` (a control ADR or GLOSSARY change), `draft` (a draft PR on the global principles) or `covered` (its home holds it already, `link` saying where). An entry is named by its `url`, as `--read` and `--lessons` give it (`daybook:<id>`), and must be one of the span's. Each lesson of the days `since` to `until` is carried exactly once, by its entry's `url`; its `lesson` may be left out. A pattern recurring across entries without a lesson names two entries at least and says its `lesson`. The file is checked against the span's entries before anything is written, then one entry is written whose `Did` maps each lesson to where it went.

The repo an entry is of is the one the checkout's remote names, or `--repo`; run in no checkout, `--repo` is needed to write one.

It is not fleet state: nothing reads it to decide what a fleet does.

## What it answers

{"url", "day", "at"}

- `url`: the entry's name, `daybook:<id>`
- `day`: the day it is of, the host's local date
- `at`: when it was written, epoch seconds

With `--read`: {"entries": [{"url", "day", "at", "lesson", "text"}], "since", "until"}, oldest day first; `lesson` is the entry's lesson, or null.

With `--lessons`: {"lessons": [{"day", "url", "lesson"}], "since", "until"}, oldest day first.

With `--distilled`: the entry's answer, and "since", "until", "lessons" (how many the span holds) and "carried" (how many lessons and patterns the file carried). With `--distill`: {"asked", "at"}, the epoch second the watch was asked at.

Errors, as {"error": …} with a non-zero exit, each before anything is written: an entry that says nothing; a day not said `YYYY-MM-DD`; a distillation file that carries a lesson of its span nowhere or twice, names an entry not of its span, or carries a pattern with no `lesson` or fewer than two entries; no repo to name, run in no checkout; the data directory cannot be made or the database written (its path is in the error).

## What trips you up

- An entry is written once: a second run writes a second entry. To correct one, write another that says what it corrects.
- The daybook is this host's: another host's entries are read on that host. Entries written to GitHub before the daybook moved to the host stay there, and are not distilled again."#;

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

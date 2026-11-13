//! The daybook crate as another program uses it (docs/adr/0071): through its
//! public API alone, with no afkconvoy beside it, against a fake GitHub that
//! keeps one repo's discussions in memory. A program appends entries to the
//! day's discussion, a lesson is indexed in the day's body and listed back,
//! and two writers creating the day's discussion at once both write to one.

use std::sync::{Barrier, Mutex};
use std::thread;

use afkconvoy_daybook::{Daybook, GraphQl, Held, Lesson, Said, Variable, entry, gh_args};
use anyhow::{Result, bail};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Discussion {
    number: u64,
    title: String,
    body: String,
    comments: Vec<(String, String)>,
}

impl Discussion {
    fn id(&self) -> String {
        format!("D_{}", self.number)
    }

    fn url(&self) -> String {
        format!("https://github.com/acme/control/discussions/{}", self.number)
    }

    fn node(&self) -> Value {
        json!({
            "id": self.id(), "number": self.number, "title": self.title, "url": self.url(),
            "body": self.body, "comments": {"totalCount": self.comments.len()},
        })
    }
}

/// One repo's Discussions, with a `Daybook` category. Where `together` is
/// set, the first look of each writer for the day's discussion waits for
/// that many writers to have looked, so that each finds none.
#[derive(Default)]
struct Fake {
    discussions: Mutex<Vec<Discussion>>,
    numbers: Mutex<u64>,
    together: Option<Barrier>,
    /// Each operation asked, in order.
    asked: Mutex<Vec<String>>,
}

impl Fake {
    fn racing(writers: usize) -> Fake {
        Fake { together: Some(Barrier::new(writers)), ..Fake::default() }
    }

    fn discussions(&self) -> Vec<Discussion> {
        self.discussions.lock().unwrap().clone()
    }
}

fn text<'a>(variables: &'a [(&str, Variable)], name: &str) -> &'a str {
    match variables.iter().find(|(named, _)| *named == name) {
        Some((_, Variable::Text(text))) => text,
        other => panic!("no text {name}: {other:?}"),
    }
}

impl GraphQl for Fake {
    fn graphql(&self, query: &str, variables: &[(&str, Variable)], writes: bool) -> Result<Value> {
        let operation = query.split(['(', ' ']).nth(1).unwrap_or_default().to_string();
        self.asked.lock().unwrap().push(operation.clone());
        assert_eq!(writes, query.starts_with("mutation"), "{operation}");
        if !writes {
            assert_eq!((text(variables, "owner"), text(variables, "repo")), ("acme", "control"));
        }
        let mut discussions = self.discussions.lock().unwrap();
        let by_id = |discussions: &mut Vec<Discussion>, id: &str| {
            discussions.iter().position(|d| d.id() == id).expect("a discussion of this id")
        };
        Ok(match operation.as_str() {
            "ConvoyDaybookPlace" => json!({"data": {"repository": {
                "id": "R_1", "hasDiscussionsEnabled": true,
                "discussionCategories": {"nodes": [
                    {"id": "C_general", "name": "General"}, {"id": "C_daybook", "name": "Daybook"},
                ]},
            }}}),
            "ConvoyDaybookDay"
            | "ConvoyDaybookSettle"
            | "ConvoyDaybookLessons"
            | "ConvoyDaybookSpan" => {
                assert_eq!(text(variables, "category"), "C_daybook");
                if operation == "ConvoyDaybookDay"
                    && let Some(together) = &self.together
                {
                    let seen: Vec<Value> = discussions.iter().map(Discussion::node).collect();
                    drop(discussions);
                    together.wait();
                    return Ok(json!({"data": {"repository": {"discussions": {"nodes": seen}}}}));
                }
                let nodes: Vec<Value> = discussions.iter().rev().map(Discussion::node).collect();
                json!({"data": {"repository": {"discussions": {
                    "nodes": nodes, "pageInfo": {"hasNextPage": false},
                }}}})
            }
            "ConvoyDaybookOpen" => {
                let mut numbers = self.numbers.lock().unwrap();
                *numbers += 1;
                let made = Discussion {
                    number: *numbers,
                    title: text(variables, "title").to_string(),
                    body: text(variables, "body").to_string(),
                    comments: Vec::new(),
                };
                let node = made.node();
                discussions.push(made);
                json!({"data": {"createDiscussion": {"discussion": node}}})
            }
            "ConvoyDaybookDrop" => {
                let at = by_id(&mut discussions, text(variables, "id"));
                discussions.remove(at);
                json!({"data": {"deleteDiscussion": {"discussion": {"id": "gone"}}}})
            }
            "ConvoyDaybookEntry" => {
                let at = by_id(&mut discussions, text(variables, "discussion"));
                let discussion = &mut discussions[at];
                let url = format!("{}#discussioncomment-{}", discussion.url(), {
                    discussion.comments.len() + 1
                });
                discussion.comments.push((text(variables, "body").to_string(), url.clone()));
                json!({"data": {"addDiscussionComment": {"comment": {"url": url}}}})
            }
            "ConvoyDaybookRead" => {
                let Some((_, Variable::Int(number))) =
                    variables.iter().find(|(n, _)| *n == "number")
                else {
                    bail!("a read names its discussion's number as an Int");
                };
                let found = discussions.iter().find(|d| d.number == *number).unwrap();
                let nodes: Vec<Value> = found
                    .comments
                    .iter()
                    .map(|(body, url)| json!({"body": body, "url": url}))
                    .collect();
                json!({"data": {"repository": {"discussion": {
                    "body": found.body,
                    "comments": {"nodes": nodes, "pageInfo": {"hasNextPage": false}},
                }}}})
            }
            "ConvoyDaybookIndex" => {
                let at = by_id(&mut discussions, text(variables, "discussion"));
                discussions[at].body = text(variables, "body").to_string();
                json!({"data": {"updateDiscussion": {"discussion": {"id": "ok"}}}})
            }
            other => bail!("the fake knows no {other}"),
        })
    }
}

fn did(what: &str) -> Said {
    Said { did: Some(what.to_string()), ..Said::default() }
}

#[test]
fn an_entry_opens_the_day_s_discussion_and_the_next_is_added_to_it() {
    let fake = Fake::default();
    let daybook = Daybook::new(&fake, "acme/control", "guildd").unwrap();
    let first =
        daybook.append("2026-11-13", &entry("acme/dev", "agent", &did("installed jq"))).unwrap();
    assert!(first.created, "{first:?}");
    let second = daybook.append("2026-11-13", &entry("acme/dev", "agent", &did("ruled"))).unwrap();
    assert!(!second.created);
    assert_eq!(
        (second.discussion.as_str(), second.day.as_str()),
        (first.discussion.as_str(), "2026-11-13")
    );
    assert_ne!(second.url, first.url);

    let found = fake.discussions();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "2026-11-13");
    assert!(found[0].body.contains("Written by `guildd`"), "{}", found[0].body);
    assert_eq!(
        found[0].comments[0].0,
        "**Repo** `acme/dev` · **Session** agent\n\n### Did\n\ninstalled jq\n"
    );
    let other =
        daybook.append("2026-11-14", &entry("acme/dev", "agent", &did("next day"))).unwrap();
    assert!(other.created && other.discussion != first.discussion);
}

#[test]
fn a_lesson_is_indexed_in_the_day_s_body_and_listed_back_with_the_span_it_is_in() {
    let fake = Fake::default();
    let daybook = Daybook::new(&fake, "acme/control", "guildd").unwrap();
    let plain = daybook.append("2026-11-12", &entry("acme/dev", "agent", &did("looked"))).unwrap();
    let lesson = Said { lesson: Some("the gate\n needs jq".into()), ..did("installed jq") };
    let taught = daybook.append("2026-11-13", &entry("acme/dev", "agent", &lesson)).unwrap();
    assert!(!fake.discussions()[0].body.contains("Lessons"), "no lesson, no index");
    assert!(fake.discussions()[1].body.contains(&taught.url));

    let lessons = daybook.lessons("2026-11-12", None).unwrap();
    let want = Lesson {
        day: "2026-11-13".into(),
        url: taught.url.clone(),
        lesson: "the gate needs jq".into(),
    };
    assert_eq!(lessons, vec![want]);
    assert_eq!(daybook.lessons("2026-11-12", Some("2026-11-12")).unwrap(), vec![]);
    assert_eq!(
        daybook.held("2026-11-12", "2026-11-13").unwrap(),
        Held { lessons: 1, unlessoned: 1 }
    );
    assert_ne!(plain.discussion, taught.discussion);

    // An index another writer lost is rebuilt from the entries by the next.
    fake.discussions.lock().unwrap()[1].body = "edited away".into();
    let again = Said { lesson: Some("jq is needed twice".into()), ..did("again") };
    let second = daybook.append("2026-11-13", &entry("acme/dev", "agent", &again)).unwrap();
    let urls: Vec<String> =
        daybook.lessons("2026-11-13", None).unwrap().into_iter().map(|l| l.url).collect();
    assert_eq!(urls, vec![taught.url, second.url]);
}

#[test]
fn two_writers_creating_the_day_s_discussion_at_once_both_write_to_the_lowest_numbered() {
    let fake = Fake::racing(2);
    let appended: Vec<_> = thread::scope(|scope| {
        let writers: Vec<_> = ["one", "two"]
            .map(|who| {
                let fake = &fake;
                scope.spawn(move || {
                    let daybook = Daybook::new(fake, "acme/control", "guildd").unwrap();
                    daybook.append("2026-11-13", &entry("acme/dev", who, &did(who))).unwrap()
                })
            })
            .into_iter()
            .collect();
        writers.into_iter().map(|writer| writer.join().unwrap()).collect()
    });
    let found = fake.discussions();
    assert_eq!(found.len(), 1, "the second discussion is deleted: {found:?}");
    assert_eq!(found[0].number, 1);
    assert_eq!(found[0].comments.len(), 2);
    assert!(appended.iter().all(|appended| appended.discussion == found[0].url()));
    assert_eq!(appended.iter().filter(|appended| appended.created).count(), 1);
    let asked = fake.asked.lock().unwrap();
    assert_eq!(asked.iter().filter(|op| *op == "ConvoyDaybookOpen").count(), 2);
    assert_eq!(asked.iter().filter(|op| *op == "ConvoyDaybookDrop").count(), 1);
}

#[test]
fn a_repo_said_wrongly_and_a_refusal_are_errors() {
    assert!(Daybook::new(Fake::default(), "control", "guildd").is_err());
    struct Refusing;
    impl GraphQl for Refusing {
        fn graphql(&self, _: &str, _: &[(&str, Variable)], _: bool) -> Result<Value> {
            Ok(json!({"errors": [{"message": "no"}]}))
        }
    }
    let daybook = Daybook::new(Refusing, "acme/control", "guildd").unwrap();
    let error = daybook.append("2026-11-13", "x").unwrap_err();
    assert!(format!("{error:#}").contains("GitHub refused it"), "{error:#}");
}

#[test]
fn gh_is_asked_each_variable_as_a_field_an_int_typed() {
    let args =
        gh_args("query Q", &[("owner", Variable::Text("acme")), ("number", Variable::Int(7))]);
    assert_eq!(
        args,
        ["api", "graphql", "-f", "owner=acme", "-F", "number=7", "-f", "query=query Q"]
    );
}

/// What the crate depends on: anyhow, libc and serde — never afkconvoy-core
/// or the cli, whose fleet concepts its API is to stay free of.
#[test]
fn the_crate_depends_on_nothing_of_the_fleet() {
    let manifest = include_str!("../Cargo.toml");
    let section = manifest.split("[dependencies]").nth(1).unwrap().split("\n[").next().unwrap();
    let mut names: Vec<&str> = section
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once(['=', '.']).map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names, ["anyhow", "libc", "serde", "serde_json"]);
}

/// The crate's public API names no fleet concept: none of its public items
/// and fields is named for a team, role, stage, issue, claim, patrol,
/// intake, worker or watch, nor reads an environment variable.
#[test]
fn its_public_api_names_no_fleet_concept() {
    const FLEET: [&str; 9] =
        ["team", "role", "stage", "issue", "claim", "patrol", "intake", "worker", "watch"];
    let sources = [
        include_str!("../src/lib.rs"),
        include_str!("../src/book.rs"),
        include_str!("../src/day.rs"),
        include_str!("../src/entry.rs"),
    ];
    for source in sources {
        let public = source.split("#[cfg(test)]").next().unwrap();
        assert!(!public.contains("env::var"), "the crate reads no environment");
        for line in public.lines().map(str::trim).filter(|line| line.starts_with("pub ")) {
            let lower = line.to_lowercase();
            let words: Vec<&str> = lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            for concept in FLEET {
                assert!(
                    !words.iter().any(|word| word.starts_with(concept)),
                    "`{line}` names a fleet concept: {concept}"
                );
            }
        }
    }
}

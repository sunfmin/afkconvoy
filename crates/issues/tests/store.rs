//! The local tracker as another program uses it: through its public API
//! alone, against a database in a directory of the test's own.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Barrier;
use std::thread;

use afkconvoy_issues::{Blocker, Error, Filter, Import, State, Store, Which};

/// An issue as `acme/tracker#<n>` held it: opened at 100, edited at 150, two
/// comments, blocked by `blockers`.
fn moved<'a>(source: &'a str, blockers: &'a [u64]) -> Import<'a> {
    Import {
        source,
        repo: REPO,
        title: "Moved",
        body: "as it was",
        labels: &["ready-for-agent", "bug"],
        created: 100,
        body_edited: Some(150),
        comments: &[("<!--convoy:claim-->\nfirst", 110), ("second", 120)],
        blockers,
    }
}

#[test]
fn an_imported_issue_keeps_the_times_and_comments_its_other_tracker_gave_it() {
    let (mut store, _) = store("import");
    let blocker = store.create(REPO, "Here first", "", &[], 50).unwrap();
    let number = store.import(&moved("acme/tracker#7", &[blocker]), 1000).unwrap();
    let view = store.view(number).unwrap();
    let issue = &view.issue;
    assert_eq!((issue.title.as_str(), issue.body.as_str()), ("Moved", "as it was"));
    assert_eq!(issue.labels, ["bug", "ready-for-agent"]);
    assert_eq!(
        (issue.created, issue.updated, issue.body_edited, issue.closed),
        (100, 1000, Some(150), None),
        "opened when the other tracker says; changed by the move"
    );
    let comments: Vec<(&str, u64)> =
        view.comments.iter().map(|c| (c.body.as_str(), c.created)).collect();
    assert_eq!(comments, [("<!--convoy:claim-->\nfirst", 110), ("second", 120)]);
    assert_eq!(view.blockers.iter().map(|b| b.number).collect::<Vec<_>>(), [blocker]);
    let added = store.labels_added(number).unwrap();
    assert_eq!(added, [("bug".to_string(), Some(1000)), ("ready-for-agent".into(), Some(1000))]);
    assert_eq!(store.imported("acme/tracker#7").unwrap(), Some(number));
    assert_eq!(store.imported("acme/tracker#8").unwrap(), None);
}

#[test]
fn an_import_run_again_keeps_no_second_copy_and_changes_nothing() {
    let (mut store, _) = store("import-again");
    let number = store.import(&moved("acme/tracker#7", &[]), 1000).unwrap();
    store.comment(number, "worked here since", 1100).unwrap();
    let before = store.view(number).unwrap();
    assert_eq!(store.import(&moved("acme/tracker#7", &[]), 2000).unwrap(), number);
    assert_eq!(store.view(number).unwrap(), before);
    let all = store.list(&Filter { state: Which::All, ..Filter::default() }).unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn an_import_blocked_by_no_issue_here_keeps_nothing() {
    let (mut store, _) = store("import-whole");
    let refused = store.import(&moved("acme/tracker#7", &[9]), 1000).unwrap_err();
    assert!(matches!(refused, Error::NoSuchIssue(9)), "{refused}");
    assert_eq!(store.imported("acme/tracker#7").unwrap(), None);
    let all = store.list(&Filter { state: Which::All, ..Filter::default() }).unwrap();
    assert!(all.is_empty(), "{all:?}");
    let bad = Import { repo: "widgets", ..moved("acme/tracker#7", &[]) };
    assert!(matches!(store.import(&bad, 1000), Err(Error::BadRepo(_))));
}

/// A directory no other test writes in, empty.
fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("afkconvoy-issues-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn store(name: &str) -> (Store, PathBuf) {
    let path = dir(name).join("issues.db");
    (Store::open(&path).unwrap(), path)
}

const REPO: &str = "acme/widgets";

#[test]
fn an_issue_reads_back_as_it_was_opened() {
    let (mut store, _) = store("create");
    let first = store.create(REPO, "First", "the body", &["bug", "ready", "bug"], 100).unwrap();
    let second = store.create("acme/gears", "Second", "", &[], 101).unwrap();
    assert_eq!((first, second), (1, 2));
    let view = store.view(first).unwrap();
    let issue = &view.issue;
    assert_eq!(issue.number, 1);
    assert_eq!(issue.repo, REPO);
    assert_eq!(issue.title, "First");
    assert_eq!(issue.body, "the body");
    assert_eq!(issue.labels, ["bug", "ready"]);
    assert_eq!(
        (issue.created, issue.updated, issue.body_edited, issue.closed),
        (100, 100, None, None)
    );
    assert_eq!(issue.state(), State::Open);
    assert!(view.comments.is_empty() && view.blockers.is_empty());
}

#[test]
fn a_repo_not_named_owner_slash_repo_is_refused() {
    let (mut store, _) = store("bad-repo");
    for repo in ["widgets", "acme/widgets/x", " /x", ""] {
        let refused = store.create(repo, "t", "b", &[], 1).unwrap_err();
        assert!(matches!(&refused, Error::BadRepo(named) if named == repo), "{refused}");
    }
    assert_eq!(store.create(REPO, "t", "b", &[], 1).unwrap(), 1, "no number was spent");
}

#[test]
fn a_missing_issue_or_comment_is_told_apart_from_a_failure() {
    let (mut store, _) = store("missing");
    store.create(REPO, "t", "b", &[], 1).unwrap();
    assert!(matches!(store.view(9), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.comment(9, "x", 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.edit_body(9, "x", 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.edit_title(9, "x", 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.add_labels(9, &["x"], 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.remove_labels(9, &["x"], 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.close(9, 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.reopen(9, 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.blockers(9), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.add_blocker(1, 9, 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.add_blocker(9, 1, 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.remove_blocker(1, 9, 2), Err(Error::NoSuchIssue(9))));
    assert!(matches!(store.edit_comment(5, "x", 2), Err(Error::NoSuchComment(5))));
    assert_eq!(store.view(1).unwrap().issue.updated, 1, "nothing refused wrote");
}

#[test]
fn comments_are_kept_in_order_and_an_edit_records_when() {
    let (mut store, _) = store("comments");
    let one = store.create(REPO, "One", "", &[], 10).unwrap();
    let two = store.create(REPO, "Two", "", &[], 10).unwrap();
    let a = store.comment(one, "first", 20).unwrap();
    let b = store.comment(two, "elsewhere", 21).unwrap();
    let c = store.comment(one, "second", 22).unwrap();
    assert_eq!((a, b, c), (1, 2, 3), "ids count across issues");
    store.edit_comment(a, "first, edited", 30).unwrap();
    let view = store.view(one).unwrap();
    let said: Vec<_> = view
        .comments
        .iter()
        .map(|c| (c.id, c.issue, c.body.as_str(), c.created, c.edited))
        .collect();
    assert_eq!(said, [(1, one, "first, edited", 20, Some(30)), (3, one, "second", 22, None)]);
    assert_eq!(view.issue.updated, 30);
    assert_eq!(store.view(two).unwrap().comments.len(), 1);
}

#[test]
fn a_body_edit_records_when() {
    let (mut store, _) = store("body");
    let n = store.create(REPO, "t", "old", &[], 10).unwrap();
    store.edit_body(n, "new", 40).unwrap();
    let issue = store.view(n).unwrap().issue;
    assert_eq!((issue.body.as_str(), issue.body_edited, issue.updated), ("new", Some(40), 40));
    assert_eq!(issue.created, 10);
}

#[test]
fn a_title_edit_changes_the_issue_and_leaves_its_body() {
    let (mut store, _) = store("title");
    let n = store.create(REPO, "old", "body", &[], 10).unwrap();
    store.edit_title(n, "new", 40).unwrap();
    let issue = store.view(n).unwrap().issue;
    assert_eq!((issue.title.as_str(), issue.body.as_str()), ("new", "body"));
    assert_eq!((issue.updated, issue.body_edited), (40, None));
}

#[test]
fn label_writes_are_idempotent() {
    let (mut store, _) = store("labels");
    let n = store.create(REPO, "t", "", &["a"], 10).unwrap();
    store.add_labels(n, &["b", "a"], 20).unwrap();
    store.add_labels(n, &["b"], 30).unwrap();
    let issue = store.view(n).unwrap().issue;
    assert_eq!(issue.labels, ["a", "b"]);
    assert_eq!(issue.updated, 20, "a label already there changed nothing");
    store.remove_labels(n, &["a", "zzz"], 40).unwrap();
    store.remove_labels(n, &["a"], 50).unwrap();
    let issue = store.view(n).unwrap().issue;
    assert_eq!(issue.labels, ["b"]);
    assert_eq!(issue.updated, 40, "a label already gone changed nothing");
}

#[test]
fn a_label_keeps_when_it_was_put_on_until_it_is_taken_off() {
    let (mut store, _) = store("label-times");
    let n = store.create(REPO, "t", "", &["a"], 10).unwrap();
    store.add_labels(n, &["b"], 20).unwrap();
    store.add_labels(n, &["a", "b"], 30).unwrap();
    let added = |store: &mut Store| store.labels_added(n).unwrap();
    assert_eq!(added(&mut store), [("a".into(), Some(10)), ("b".into(), Some(20))]);
    store.remove_labels(n, &["a"], 40).unwrap();
    store.add_labels(n, &["a"], 50).unwrap();
    assert_eq!(added(&mut store), [("a".into(), Some(50)), ("b".into(), Some(20))]);
    assert!(matches!(store.labels_added(99), Err(Error::NoSuchIssue(99))));
}

#[test]
fn close_and_reopen_record_when_and_repeat_as_no_change() {
    let (mut store, _) = store("close");
    let n = store.create(REPO, "t", "", &[], 10).unwrap();
    store.close(n, 20).unwrap();
    store.close(n, 30).unwrap();
    let issue = store.view(n).unwrap().issue;
    assert_eq!((issue.state(), issue.closed, issue.updated), (State::Closed, Some(20), 20));
    store.reopen(n, 40).unwrap();
    store.reopen(n, 50).unwrap();
    let issue = store.view(n).unwrap().issue;
    assert_eq!((issue.state(), issue.closed, issue.updated), (State::Open, None, 40));
    store.comment(n, "on a closed issue too", 60).unwrap();
}

#[test]
fn numbers_are_never_given_again() {
    let (mut store, path) = store("numbers");
    for at in 0..3 {
        let n = store.create(REPO, "t", "", &[], at).unwrap();
        store.close(n, at).unwrap();
    }
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.create(REPO, "after", "", &[], 9).unwrap(), 4);
}

#[test]
fn blockers_are_read_with_their_state_and_labels() {
    let (mut store, _) = store("blockers");
    let n = store.create(REPO, "blocked", "", &[], 1).unwrap();
    let a = store.create(REPO, "a", "", &["x"], 1).unwrap();
    let b = store.create("acme/gears", "b", "", &[], 1).unwrap();
    store.add_blocker(n, b, 5).unwrap();
    store.add_blocker(n, a, 6).unwrap();
    store.add_blocker(n, a, 7).unwrap();
    store.close(b, 8).unwrap();
    let blockers = store.blockers(n).unwrap();
    assert_eq!(
        blockers,
        [
            Blocker { number: a, repo: REPO.into(), closed: None, labels: vec!["x".into()] },
            Blocker { number: b, repo: "acme/gears".into(), closed: Some(8), labels: vec![] },
        ]
    );
    assert_eq!(
        blockers.iter().map(Blocker::state).collect::<Vec<_>>(),
        [State::Open, State::Closed]
    );
    assert_eq!(store.view(n).unwrap().blockers, blockers);
    assert_eq!(store.view(n).unwrap().issue.updated, 6, "an edge already there changed nothing");
    assert!(store.blockers(a).unwrap().is_empty(), "an edge has a direction");
    store.remove_blocker(n, b, 9).unwrap();
    store.remove_blocker(n, b, 10).unwrap();
    assert_eq!(store.blockers(n).unwrap().iter().map(|b| b.number).collect::<Vec<_>>(), [a]);
    assert_eq!(store.view(n).unwrap().issue.updated, 9);
}

#[test]
fn an_edge_onto_itself_or_round_a_cycle_is_refused() {
    let (mut store, _) = store("cycle");
    let [a, b, c] = [1, 2, 3].map(|_| store.create(REPO, "t", "", &[], 1).unwrap());
    assert!(matches!(store.add_blocker(a, a, 2), Err(Error::BlocksItself(1))));
    store.add_blocker(a, b, 2).unwrap();
    store.add_blocker(b, c, 2).unwrap();
    assert!(matches!(store.add_blocker(b, a, 3), Err(Error::Cycle { issue: 2, blocker: 1 })));
    assert!(matches!(store.add_blocker(c, a, 3), Err(Error::Cycle { issue: 3, blocker: 1 })));
    store.add_blocker(a, c, 3).unwrap();
    assert!(store.blockers(c).unwrap().is_empty());
    store.remove_blocker(b, c, 4).unwrap();
    store.add_blocker(c, b, 5).unwrap();
}

#[test]
fn a_listing_holds_what_its_filter_names() {
    let (mut store, _) = store("list");
    let open_bug = store.create(REPO, "1", "", &["bug", "ready"], 10).unwrap();
    let closed_bug = store.create(REPO, "2", "", &["bug"], 11).unwrap();
    let other = store.create("acme/gears", "3", "", &["bug", "ready"], 12).unwrap();
    let plain = store.create(REPO, "4", "", &[], 13).unwrap();
    store.close(closed_bug, 20).unwrap();
    let numbers = |store: &mut Store, filter: Filter<'_>| -> Vec<u64> {
        store.list(&filter).unwrap().iter().map(|issue| issue.number).collect()
    };
    assert_eq!(numbers(&mut store, Filter::default()), [open_bug, other, plain]);
    assert_eq!(
        numbers(&mut store, Filter { state: Which::All, ..Filter::default() }),
        [1, 2, 3, 4]
    );
    assert_eq!(numbers(&mut store, Filter { state: Which::Closed, ..Filter::default() }), [2]);
    let repo = Filter { repo: Some(REPO), state: Which::All, ..Filter::default() };
    assert_eq!(numbers(&mut store, repo), [open_bug, closed_bug, plain]);
    let both = Filter { labels: &["bug", "ready"], state: Which::All, ..Filter::default() };
    assert_eq!(numbers(&mut store, both), [open_bug, other], "every label named");
    let bugs =
        Filter { repo: Some(REPO), labels: &["bug"], state: Which::All, ..Filter::default() };
    assert_eq!(numbers(&mut store, bugs), [open_bug, closed_bug]);
    let since = Filter { updated_since: Some(12), state: Which::All, ..Filter::default() };
    assert_eq!(numbers(&mut store, since), [closed_bug, other, plain], "at or after");
    assert!(numbers(&mut store, Filter { repo: Some("x/y"), ..Filter::default() }).is_empty());
    let listed = store.list(&Filter::default()).unwrap();
    assert_eq!(listed[0], store.view(open_bug).unwrap().issue, "a listed issue is all of it");
}

#[test]
fn views_hold_each_issue_with_its_comments_and_blockers() {
    let (mut store, _) = store("views");
    let a = store.create(REPO, "a", "", &[], 1).unwrap();
    let b = store.create(REPO, "b", "", &[], 1).unwrap();
    store.comment(a, "on a", 2).unwrap();
    store.add_blocker(b, a, 3).unwrap();
    let views = store.views(&Filter::default()).unwrap();
    assert_eq!(views, [store.view(a).unwrap(), store.view(b).unwrap()]);
    assert_eq!(views[0].comments.len(), 1);
    assert_eq!(views[1].blockers[0].number, a);
}

#[test]
fn the_labels_in_use_are_named_once() {
    let (mut store, _) = store("label-names");
    store.create(REPO, "t", "", &["b", "a"], 1).unwrap();
    store.create(REPO, "t", "", &["a"], 1).unwrap();
    let gears = store.create("acme/gears", "t", "", &["c"], 1).unwrap();
    assert_eq!(store.labels(None).unwrap(), ["a", "b", "c"]);
    assert_eq!(store.labels(Some(REPO)).unwrap(), ["a", "b"]);
    store.remove_labels(gears, &["c"], 2).unwrap();
    assert_eq!(store.labels(None).unwrap(), ["a", "b"]);
}

#[test]
fn opening_again_converges_and_keeps_every_write() {
    let (mut store, path) = store("reopen");
    let n = store.create(REPO, "t", "", &["a"], 1).unwrap();
    store.comment(n, "kept", 2).unwrap();
    drop(store);
    for _ in 0..3 {
        Store::open(&path).unwrap();
    }
    let mut store = Store::open(&path).unwrap();
    let view = store.view(n).unwrap();
    assert_eq!((view.issue.labels.len(), view.comments.len()), (1, 1));
    assert_eq!(store.list(&Filter { state: Which::All, ..Filter::default() }).unwrap().len(), 1);
}

#[test]
fn a_store_failure_is_a_storage_error() {
    let missing = dir("no-dir").join("absent").join("issues.db");
    assert!(matches!(Store::open(&missing), Err(Error::Storage(_))));
}

/// Writers at once, in threads and in processes, each through a connection
/// of its own.
const WRITERS: u64 = 6;
const WRITES: u64 = 40;

/// What one writer does: opens issues, comments on #1 and labels it with a
/// label of its own, each write its own transaction.
fn write_as(path: &Path, writer: u64) -> Vec<u64> {
    let mut store = Store::open(path).unwrap();
    let mut opened = vec![];
    for write in 0..WRITES {
        opened.push(store.create(REPO, &format!("{writer}.{write}"), "", &[], write).unwrap());
        store.comment(1, &format!("{writer}.{write}"), write).unwrap();
        store.add_labels(1, &[&format!("w{writer}")], write).unwrap();
        let mut blocked = store.view(1).unwrap();
        blocked.issue.labels.retain(|label| label == &format!("w{writer}"));
        assert_eq!(blocked.issue.labels.len(), 1, "a writer reads its own write");
    }
    opened
}

/// Checks that every writer's every write is there, numbers given once.
fn every_write_is_there(path: &Path, mut opened: Vec<u64>) {
    let mut store = Store::open(path).unwrap();
    opened.sort_unstable();
    let expected: Vec<u64> = (2..=WRITERS * WRITES + 1).collect();
    assert_eq!(opened, expected, "every number given once, none skipped");
    let all = store.list(&Filter { state: Which::All, ..Filter::default() }).unwrap();
    assert_eq!(all.len() as u64, WRITERS * WRITES + 1);
    let one = store.view(1).unwrap();
    assert_eq!(one.comments.len() as u64, WRITERS * WRITES);
    let labels: Vec<String> = (0..WRITERS).map(|w| format!("w{w}")).collect();
    assert_eq!(one.issue.labels, labels, "no label write lost");
}

#[test]
fn threads_writing_at_once_lose_nothing() {
    let path = dir("threads").join("issues.db");
    // Every writer opens a database none made before: the schema converges.
    let barrier = Barrier::new(WRITERS as usize);
    thread::scope(|scope| {
        for _ in 0..WRITERS {
            scope.spawn(|| {
                barrier.wait();
                Store::open(&path).unwrap();
            });
        }
    });
    Store::open(&path).unwrap().create(REPO, "shared", "", &[], 0).unwrap();
    let opened = thread::scope(|scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let (path, barrier) = (&path, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    write_as(path, writer)
                })
            })
            .collect();
        writers.into_iter().flat_map(|writer| writer.join().unwrap()).collect()
    });
    every_write_is_there(&path, opened);
}

/// The database a child process writes to, where it is one.
const CHILD_DB: &str = "AFKCONVOY_ISSUES_CHILD_DB";
const CHILD_WRITER: &str = "AFKCONVOY_ISSUES_CHILD_WRITER";

/// A process's part of `processes_writing_at_once_lose_nothing`: does
/// nothing where it is not run as one of its children.
#[test]
fn child_writer() {
    let (Ok(path), Ok(writer)) = (std::env::var(CHILD_DB), std::env::var(CHILD_WRITER)) else {
        return;
    };
    let opened = write_as(Path::new(&path), writer.parse().unwrap());
    let opened: Vec<String> = opened.iter().map(u64::to_string).collect();
    println!("opened {}", opened.join(","));
}

#[test]
fn processes_writing_at_once_lose_nothing() {
    let path = dir("processes").join("issues.db");
    Store::open(&path).unwrap().create(REPO, "shared", "", &[], 0).unwrap();
    let test = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..WRITERS)
        .map(|writer| {
            Command::new(&test)
                .args(["child_writer", "--exact", "--nocapture", "--test-threads=1"])
                .env(CHILD_DB, &path)
                .env(CHILD_WRITER, writer.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut opened = vec![];
    for child in children {
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
        let line = stdout.lines().find_map(|line| Some(line.split_once("opened ")?.1)).unwrap();
        opened.extend(line.split(',').map(|n| n.parse::<u64>().unwrap()));
    }
    every_write_is_there(&path, opened);
}

//! `afkconvoy issue import` (docs/adr/0091): a GitHub tracker's open issues
//! of the repos this host has a checkout of, claimed by this host or by no
//! host, move into the host's local tracker whole, and are closed on GitHub
//! naming their new name; a run again moves nothing twice.

use crate::common;

use afkconvoy_issues::{Filter, Store, Which};
use common::{World, git};
use serde_json::{Value, json};

const TRACKER: &str = "acme/tracker";
const REPO: &str = "acme/dev";
/// When the tests' imports act: past the lease of a heartbeat at 1000.
const NOW: &str = "5000";

/// A checkout of `acme/dev` whose tracker is `acme/tracker`, on the host `h1`.
fn world() -> World {
    let world = World::new();
    world.name_origin(&world.clone, REPO);
    world.say_tracker(&world.clone, TRACKER);
    world
}

/// Opens issue `number` of `repo` in the tracker, carrying `labels` and its
/// repo label, opened at 100 + `number`.
fn issue_of(world: &World, repo: &str, number: u64, labels: &[&str]) {
    let mut labels: Vec<String> = labels.iter().map(ToString::to_string).collect();
    labels.push(format!("repo:{repo}"));
    world.with_tracker(|tracker| {
        let issue = json!({"number": number, "title": format!("Issue {number}"),
            "body": format!("body of {number}"), "labels": labels, "state": "open",
            "at": 100 + number});
        tracker["issues"].as_array_mut().unwrap().push(issue);
    });
}

fn issue(world: &World, number: u64, labels: &[&str]) {
    issue_of(world, REPO, number, labels);
}

/// Comments `body` on the tracker's issue `number`, written at `at`.
fn comment(world: &World, number: u64, body: &str, at: u64) {
    world.with_tracker(|tracker| {
        let every = tracker["comments"].as_object().unwrap().values();
        let id = 3000 + every.map(|rows| rows.as_array().unwrap().len()).sum::<usize>();
        let rows = &mut tracker["comments"][number.to_string()];
        if !rows.is_array() {
            *rows = json!([]);
        }
        rows.as_array_mut().unwrap().push(
            json!({"id": id, "body": body, "html_url": format!("https://gh/c/{id}"), "at": at}),
        );
    });
}

/// Records the tracker's issue `number` as blocked by `blocker`.
fn block(world: &World, number: u64, blocker: u64) {
    world.with_tracker(|tracker| {
        let edges = &mut tracker["edges"][number.to_string()];
        if !edges.is_array() {
            *edges = json!([]);
        }
        edges.as_array_mut().unwrap().push(json!(blocker));
    });
}

/// Puts the ref `refs/convoy/<key>` on the remote, its record `subject`.
fn record(world: &World, key: &str, subject: &str) {
    let repo = afkconvoy::git::Repo::at(&world.clone).unwrap();
    let sha = repo.record_commit(subject).unwrap();
    git(&world.clone, &["push", "-q", "--force", "origin", &format!("{sha}:refs/convoy/{key}")]);
}

/// The claim of `instance` on `host` on the tracker's issue `number`, its
/// instance's heartbeat at `beat`.
fn claim(world: &World, number: u64, instance: &str, host: &str, beat: u64) {
    let key = format!("claim/{TRACKER}/{number}");
    record(world, &key, &format!("convoy:claim instance={instance} host={host} ts=1000"));
    let beat = format!("convoy:heartbeat instance={instance} host={host} ts={beat}");
    record(world, &format!("heartbeat/{instance}"), &beat);
}

fn claim_refs(world: &World) -> Vec<String> {
    let refs = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/claim/"]);
    refs.lines().map(str::to_string).collect()
}

fn import(world: &World, more: &[&str]) -> Value {
    let line = [&["issue", "import", "--from", TRACKER, "--now", NOW][..], more].concat();
    world.afk(&line).ok()
}

/// `(from, to)` of each issue an answer says moved.
fn moved(answer: &Value) -> Vec<(String, Value)> {
    let rows = answer["moved"].as_array().unwrap_or_else(|| panic!("{answer}"));
    rows.iter().map(|row| (row["from"].as_str().unwrap().to_string(), row["to"].clone())).collect()
}

/// `from` → `why` of each issue an answer says stays.
fn skipped(answer: &Value) -> Vec<(String, String)> {
    let rows = answer["skipped"].as_array().unwrap_or_else(|| panic!("{answer}"));
    rows.iter()
        .map(|row| (row["from"].as_str().unwrap().into(), row["why"].as_str().unwrap().into()))
        .collect()
}

/// The tracker's issue `number` as the stand-in GitHub has it.
fn github(world: &World, number: u64) -> Value {
    let tracker = world.tracker();
    let rows = tracker["issues"].as_array().unwrap();
    rows.iter().find(|row| row["number"] == number).unwrap().clone()
}

fn github_comments(world: &World, number: u64) -> Vec<String> {
    let rows = world.tracker()["comments"][number.to_string()].clone();
    let rows = rows.as_array().cloned().unwrap_or_default();
    rows.iter().map(|row| row["body"].as_str().unwrap().to_string()).collect()
}

/// The local tracker's issues, all of them, as `(number, title)`.
fn local(store: &mut Store) -> Vec<(u64, String)> {
    let all = store.list(&Filter { state: Which::All, ..Filter::default() }).unwrap();
    all.into_iter().map(|issue| (issue.number, issue.title)).collect()
}

/// The world's issues: #1 blocks #2, which a closed #9 blocked too; #3 is
/// claimed by this host's instance whose heartbeat lapsed; #4 by host h2;
/// #5 by this host's live instance; #7 is blocked by #4; #6 is of a repo
/// this host has no checkout of.
fn tracker_of_seven(world: &World) {
    issue(world, 1, &["ready-for-agent"]);
    comment(world, 1, "first, by a human", 150);
    let account = "<!--convoy:account v=1-->\nWork account: none yet.";
    comment(world, 1, account, 160);
    issue(world, 2, &["ready-for-agent", "bug"]);
    block(world, 2, 1);
    issue(world, 9, &[]);
    world.with_tracker(|tracker| {
        let rows = tracker["issues"].as_array_mut().unwrap();
        rows.last_mut().unwrap()["state"] = json!("closed");
    });
    block(world, 2, 9);
    issue(world, 3, &["ready-for-agent"]);
    claim(world, 3, "alpha", "h1", 1000);
    issue(world, 4, &["ready-for-agent"]);
    claim(world, 4, "beta", "h2", 4990);
    issue(world, 5, &["ready-for-agent"]);
    claim(world, 5, "gamma", "h1", 4990);
    issue_of(world, "acme/web", 6, &["ready-for-agent"]);
    issue(world, 7, &["ready-for-agent"]);
    block(world, 7, 4);
}

#[test]
fn an_import_moves_unclaimed_and_own_lapsed_issues_whole_and_closes_them_on_github() {
    let world = world();
    tracker_of_seven(&world);
    let answer = import(&world, &[]);
    assert_eq!((&answer["from"], &answer["to"]), (&json!(TRACKER), &json!("local/h1")));
    assert_eq!(answer["errors"], json!([]), "{answer}");
    // Every blocker is kept before the issues it blocks.
    assert_eq!(
        moved(&answer),
        [
            ("acme/tracker#1".into(), json!("local/h1#1")),
            ("acme/tracker#3".into(), json!("local/h1#2")),
            ("acme/tracker#2".into(), json!("local/h1#3")),
        ],
        "{answer}"
    );
    let released = &answer["moved"][1]["released"];
    assert_eq!(released, "refs/convoy/claim/acme/tracker/3", "{answer}");
    let stays = skipped(&answer);
    let froms: Vec<&str> = stays.iter().map(|(from, _)| from.as_str()).collect();
    assert_eq!(froms, ["acme/tracker#4", "acme/tracker#5", "acme/tracker#7"], "{answer}");
    assert!(stays[0].1.contains("claimed by host h2 (instance beta)"), "{}", stays[0].1);
    assert!(stays[1].1.contains("instance gamma holds a live claim"), "{}", stays[1].1);
    assert!(stays[2].1.contains("blocked by acme/tracker#4, which stays"), "{}", stays[2].1);
    let next = format!("afkconvoy run {} --tracker local", world.clone.display());
    assert_eq!(answer["next"], json!([next]));

    // Kept whole: title, body, labels but the repo label, comments exactly,
    // GitHub's times, and the edge between two moved issues.
    let mut store = world.local_tracker();
    let one = store.view(1).unwrap();
    assert_eq!((one.issue.repo.as_str(), one.issue.title.as_str()), (REPO, "Issue 1"));
    assert_eq!((one.issue.body.as_str(), one.issue.created), ("body of 1", 101));
    assert_eq!(one.issue.labels, ["ready-for-agent"]);
    let said: Vec<(&str, u64)> =
        one.comments.iter().map(|comment| (comment.body.as_str(), comment.created)).collect();
    let account = "<!--convoy:account v=1-->\nWork account: none yet.";
    assert_eq!(said, [("first, by a human", 150), (account, 160)]);
    assert_eq!(store.imported("acme/tracker#1").unwrap(), Some(1));
    let two = store.view(3).unwrap();
    assert_eq!(two.issue.labels, ["bug", "ready-for-agent"]);
    assert_eq!(two.blockers.iter().map(|b| b.number).collect::<Vec<_>>(), [1]);
    let note = &two.comments.last().unwrap().body;
    assert!(note.contains("acme/tracker#9 (closed)"), "a closed blocker not moved: {note}");
    assert_eq!(local(&mut store).len(), 3);

    // On GitHub each moved one names its new name and is closed as not
    // planned; what stays is as it was.
    for (number, to) in [(1, "local/h1#1"), (3, "local/h1#2"), (2, "local/h1#3")] {
        let closed = github(&world, number);
        assert_eq!(
            (&closed["state"], &closed["state_reason"]),
            (&json!("closed"), &json!("not_planned"))
        );
        let said = github_comments(&world, number);
        let marker = said.last().unwrap();
        assert!(marker.starts_with(&format!("<!--convoy:moved to={to}-->")), "{marker}");
        assert!(marker.contains(&format!("Moved to {to}")), "{marker}");
    }
    for number in [4, 5, 6, 7] {
        assert_eq!(github(&world, number)["state"], "open", "#{number}");
    }
    assert_eq!(
        claim_refs(&world),
        ["refs/convoy/claim/acme/tracker/4", "refs/convoy/claim/acme/tracker/5"],
        "the lapsed claim of this host is released; the others are not"
    );
}

#[test]
fn an_import_run_again_moves_nothing_twice() {
    let world = world();
    tracker_of_seven(&world);
    import(&world, &[]);
    let before = world.local_tracker().views(&Filter { state: Which::All, ..Filter::default() });
    let again = import(&world, &[]);
    assert_eq!(moved(&again), [], "{again}");
    assert_eq!(again["errors"], json!([]), "{again}");
    let froms: Vec<String> = skipped(&again).into_iter().map(|(from, _)| from).collect();
    assert_eq!(froms, ["acme/tracker#4", "acme/tracker#5", "acme/tracker#7"]);
    let after = world.local_tracker().views(&Filter { state: Which::All, ..Filter::default() });
    assert_eq!(after.unwrap(), before.unwrap());
    assert_eq!(github_comments(&world, 1).len(), 3, "one marker comment, not two");
}

#[test]
fn an_import_cut_short_after_keeping_an_issue_converges_without_a_second_copy() {
    let world = world();
    issue(&world, 1, &["ready-for-agent"]);
    issue(&world, 2, &["ready-for-agent"]);
    // The comment naming #1's new name fails; #2's close fails after it.
    world.with_gh(|gh| {
        gh["fail"] = json!(["issues/1/comments", "issues/2 -f state=closed"]);
    });
    let cut = import(&world, &[]);
    assert_eq!(moved(&cut), [], "{cut}");
    let errors = cut["errors"].as_array().unwrap();
    let failed: Vec<(&str, &str)> = errors
        .iter()
        .map(|error| (error["from"].as_str().unwrap(), error["to"].as_str().unwrap()))
        .collect();
    assert_eq!(failed, [("acme/tracker#1", "local/h1#1"), ("acme/tracker#2", "local/h1#2")]);
    assert!(errors[0]["error"].as_str().unwrap().contains("not closed on GitHub"), "{cut}");
    assert_eq!(github(&world, 1)["state"], "open");
    assert_eq!(github_comments(&world, 1), [] as [String; 0]);
    assert_eq!(github(&world, 2)["state"], "open");
    assert_eq!(github_comments(&world, 2).len(), 1, "#2's marker was written");

    world.with_gh(|gh| gh["fail"] = json!([]));
    let again = import(&world, &[]);
    assert_eq!(again["errors"], json!([]), "{again}");
    assert_eq!(moved(&again), [("acme/tracker#1".into(), json!("local/h1#1"))], "{again}");
    let stays = skipped(&again);
    assert_eq!(stays.len(), 1, "{again}");
    assert_eq!(stays[0].0, "acme/tracker#2");
    assert!(stays[0].1.contains("moved already to local/h1#2; closed on GitHub now"), "{again}");
    assert_eq!(local(&mut world.local_tracker()), [(1, "Issue 1".into()), (2, "Issue 2".into())]);
    for number in [1, 2] {
        assert_eq!(github(&world, number)["state_reason"], "not_planned");
        assert_eq!(github_comments(&world, number).len(), 1, "one marker on #{number}");
    }
}

#[test]
fn a_dry_run_says_what_would_move_and_what_stays_and_changes_nothing() {
    let world = world();
    tracker_of_seven(&world);
    let (tracker, refs) = (world.tracker(), claim_refs(&world));
    let answer = import(&world, &["--dry-run"]);
    assert_eq!(answer["dry_run"], true);
    assert_eq!(
        moved(&answer),
        [
            ("acme/tracker#1".into(), Value::Null),
            ("acme/tracker#3".into(), Value::Null),
            ("acme/tracker#2".into(), Value::Null),
        ],
        "{answer}"
    );
    let froms: Vec<String> = skipped(&answer).into_iter().map(|(from, _)| from).collect();
    assert_eq!(froms, ["acme/tracker#4", "acme/tracker#5", "acme/tracker#7"]);
    let mut kept = world.tracker();
    kept["calls"] = tracker["calls"].clone();
    kept["envs"] = tracker["envs"].clone();
    assert_eq!(kept, tracker, "GitHub is as it was");
    assert_eq!(claim_refs(&world), refs);
    assert!(!world.data().join("afkconvoy/issues.db").exists(), "no local tracker is made");
}

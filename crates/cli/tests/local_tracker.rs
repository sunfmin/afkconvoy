//! A fleet whose tracker is its host's local tracker (docs/adr/0091): every
//! issue it reads and writes is in the host's own database, named
//! `local/<host>#<n>`, and every PR, branch and ref is the checkout's repo's
//! on GitHub, as of any tracker.

use crate::common;

use std::path::PathBuf;

use afkconvoy::checkout_list::List;
use afkconvoy_core::config::RepoName;
use afkconvoy_issues::{Filter, Store, Which};
use common::{World, commit, git};
use serde_json::{Value, json};

/// The world's host, `h1`, names its local tracker so.
const TRACKER: &str = "local/h1";
const REPO: &str = "acme/dev";

/// An unbound repo, `acme/dev`, whose checkout says its host's local tracker
/// is its tracker, and whose gate passes.
fn world() -> World {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, REPO);
    world.say_local_tracker(&world.clone);
    world
}

/// Opens an issue of `repo` carrying `labels` in the world's local tracker →
/// its number.
fn open_of(world: &World, repo: &str, title: &str, labels: &[&str]) -> u64 {
    world.local_tracker().create(repo, title, "", labels, 900).unwrap()
}

fn open(world: &World, title: &str, labels: &[&str]) -> u64 {
    open_of(world, REPO, title, labels)
}

fn comments(store: &mut Store, number: u64) -> Vec<String> {
    store.view(number).unwrap().comments.into_iter().map(|comment| comment.body).collect()
}

fn tick(world: &World, now: &str) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let steps =
        steps.filter(|step| !["board", "heartbeat"].contains(&step["do"].as_str().unwrap()));
    let steps = steps.filter(|step| step["outcome"] != "nothing");
    steps
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

fn frontier(plan: &Value) -> Vec<u64> {
    let rows = plan["frontier"].as_array().unwrap_or_else(|| panic!("{plan}"));
    rows.iter().map(|row| row["number"].as_u64().unwrap()).collect()
}

/// Every `gh` call the world's commands made of an issue or a label: none
/// is made of a local tracker.
fn issue_calls(world: &World) -> Vec<Vec<String>> {
    let calls: Vec<Vec<String>> = serde_json::from_value(world.gh()["calls"].clone()).unwrap();
    calls
        .into_iter()
        .filter(|call| {
            call.iter().any(|arg| {
                arg.contains("/issues") || arg.contains("/labels") || arg.contains(".visibility")
            }) || call.first().is_some_and(|word| word == "issue" || word == "label")
                || (call.starts_with(&["api".into(), "graphql".into()])
                    && call.iter().any(|arg| arg.contains("issues(")))
        })
        .collect()
}

#[test]
fn an_issue_opened_locally_is_claimed_worked_landed_and_closed_by_its_local_name() {
    let world = world();
    let number = open(&world, "Add plan", &["ready-for-agent"]);
    assert_eq!(number, 1);
    // The repo's own GitHub issue of the same number is no issue of the fleet's.
    world.issue(1, &["ready-for-agent"]);
    let own = world.gh()["issues"].clone();

    assert_eq!(did(&tick(&world, "1000")), ["dispatch 1 done"]);
    let claims = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/claim/"]);
    assert_eq!(claims, "refs/convoy/claim/local/h1/1", "named for the local tracker's issue");
    assert_eq!(world.claims(), [(1, "alpha".to_string())]);
    let said = comments(&mut world.local_tracker(), 1).join("\n");
    assert!(said.contains("afkconvoy works this issue on the branch"), "{said}");
    let claimed = world.local_tracker().view(1).unwrap().comments[0].created;
    assert_eq!(claimed, 1000, "a write of the tick is of the time it acts at");

    // Its worker, on a branch named for the local tracker's issue, opens the
    // PR that closes it by the name the repo gives it.
    let worktree = PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap());
    let branch = git(&worktree, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(branch, "tester/local-h1-1-add-plan");
    commit(&worktree, "plan.txt", "1\n");
    git(&worktree, &["push", "-q", "origin", "HEAD"]);
    world.pr_closing(70, "local/h1#1", &branch);
    let gated = world.afk_in(&worktree, &["gate"]).ok();
    assert_eq!(gated["status"], "green", "{gated}");
    let joined = world.afk_in(&worktree, &["land", "--issue", "1", "--now", "1050"]).ok();
    assert_eq!(joined["outcome"], "merged", "{joined}");

    let landed = world.local_tracker().view(1).unwrap().issue;
    assert_eq!(landed.closed, Some(1050), "the turn that landed it closed it");
    assert_eq!(did(&tick(&world, "1090")), ["settle 1 done"]);
    assert_eq!(world.claims(), []);
    let joined = world.remote(&["log", "--format=%s%n%b", "--merges", "-1", "main"]);
    assert_eq!(
        joined, "pr 70 (#70)\nCloses local/h1#1\nLanded on h1",
        "the join names the local issue"
    );

    // GitHub was asked of no issue, label or visibility: only PRs and branches.
    assert_eq!(issue_calls(&world), [] as [Vec<String>; 0]);
    assert_eq!(world.gh()["issues"], own, "the repo's own #1 is as it was");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1100"]).ok();
    assert_eq!(plan["tracker"], TRACKER);
    assert_eq!(world.afk(&["checkouts"]).ok()["checkouts"][0]["tracker"], TRACKER);
}

#[test]
fn a_comment_the_fleet_writes_is_read_back_from_the_local_tracker() {
    let world = world();
    open(&world, "Target", &["ready-for-agent"]);
    let other = open_of(&world, "acme/web", "Theirs", &["ready-for-agent"]);
    git(&world.clone, &["push", "-q", "origin", "HEAD:delivery/local"]);
    world.afk(&["target", "--issue", "1", "--branch", "delivery/local"]).ok();
    let said = comments(&mut world.local_tracker(), 1).join("\n");
    assert!(said.contains("convoy:target branch=delivery/local"), "{said}");
    assert_eq!(world.afk(&["target", "--issue", "1"]).ok()["target_branch"], "delivery/local");
    // Another repo's issue of the host's tracker is that repo's to work.
    let error =
        world.afk(&["target", "--issue", &other.to_string(), "--branch", "delivery/local"]).error();
    assert!(error.contains("is of acme/web"), "{error}");
}

#[test]
fn an_issue_is_held_back_while_one_it_is_blocked_by_is_open() {
    let world = world();
    let blocked = open(&world, "Blocked", &["ready-for-agent"]);
    let blocker = open(&world, "Blocker", &[]);
    world.local_tracker().add_blocker(blocked, blocker, 950).unwrap();
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(frontier(&plan), [] as [u64; 0], "{plan}");
    assert_eq!(plan["excluded"][0]["number"], blocked, "{plan}");
    assert_eq!(plan["excluded"][0]["reason"], "open blockers: #2", "{plan}");
    assert_eq!(plan["findings"], json!([]), "a blocker of the repo's own is worked: {plan}");

    world.local_tracker().close(blocker, 990).unwrap();
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(frontier(&plan), [blocked], "{plan}");
}

#[test]
fn a_fleet_lock_of_the_local_tracker_holds_its_fleets_of_that_repo_alone() {
    let world = world();
    open(&world, "Work", &["ready-for-agent"]);
    // A checkout of the same repo whose tracker is its own GitHub repo.
    let github = world.clone_again("github");
    world.name_origin(&github, REPO);
    world.say_said_tracker(&github, REPO);
    world.with_orca(|orca| {
        orca["repos"].as_array_mut().unwrap().push(json!({"id": "repo-2", "path": github}));
    });
    world.issue(5, &["ready-for-agent"]);
    let plan_in =
        |dir: &PathBuf| world.afk_in(dir, &["plan", "--instance", "alpha", "--now", "1000"]).ok();

    // Another repo's lock in the host's tracker holds no fleet of this one.
    open_of(&world, "acme/web", "Their lock", &["fleet-lock"]);
    assert_eq!(frontier(&plan_in(&world.clone)), [1]);

    // Its own lock holds the fleet of the local tracker, and not the fleet of
    // the same repo that works another tracker.
    open(&world, "Lock", &["fleet-lock"]);
    assert_eq!(frontier(&plan_in(&world.clone)), [] as [u64; 0]);
    assert_eq!(frontier(&plan_in(&github)), [5]);
}

#[test]
fn an_issue_of_a_repo_the_host_has_no_checkout_of_is_refused_naming_it() {
    let world = world();
    let list = List::at(world.host_db());
    let mut store = world.local_tracker();
    let open = |store: &mut Store, repo: &str| {
        let repo = RepoName::read(repo).unwrap();
        afkconvoy::issues::local::open(store, &list, None, &repo, "t", "", &[], 1000)
    };
    let refused = format!("{:#}", open(&mut store, "acme/web").unwrap_err());
    assert!(refused.contains("this host has no checkout of acme/web"), "{refused}");
    assert_eq!(open(&mut store, REPO).unwrap(), 1, "the listed checkout's repo is taken");
    let all = store.list(&Filter { state: Which::All, ..Filter::default() }).unwrap();
    assert_eq!(all.iter().map(|issue| issue.repo.as_str()).collect::<Vec<_>>(), [REPO]);
}

/// A `run` on the host `h1`: a second of its sleeps takes 10ms.
const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "10"];

#[test]
fn run_tracker_local_keeps_it_on_the_checkout_list_and_asks_github_nothing_of_it() {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, REPO);

    let first = world.afk(&[&RUN[..], &["--tracker", "local"]].concat()).ok();
    assert_eq!(first["tracker"], json!({"from": null, "to": "local"}), "{first}");
    assert_eq!(world.checkout_list()["checkouts"][0]["tracker"], "local");
    assert_eq!(world.afk(&["plan"]).ok()["tracker"], TRACKER);
    assert_eq!(world.afk(&["doctor", "--host", "h1"]).json["fleet"]["tracker"], TRACKER);

    // its next tick reads the local tracker
    open(&world, "Run", &["ready-for-agent"]);
    let until = afkconvoy_patience::deadline();
    while world.claims().is_empty() {
        assert!(std::time::Instant::now() < until, "no tick dispatched local/h1#1");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let claims = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/claim/"]);
    assert_eq!(claims, "refs/convoy/claim/local/h1/1");
    // No label is made, and no visibility is asked: it is no GitHub repo.
    assert_eq!(issue_calls(&world), [] as [Vec<String>; 0]);
    world.afk(&["stop", "--all"]).ok();
}

const ROLES: &str = "roles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    escalate: { to: human }
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    escalate: { to: human }
    on: { pass: done }
```
";

/// `world`, its repo bound to a team with the workflow `feature`.
fn bound() -> World {
    let world = world();
    for (repo, skill) in [("acme/dev", "implement"), ("acme/reviewer", "review")] {
        let file = format!(".claude/skills/{skill}/SKILL.md");
        let text = format!("---\nname: {skill}\ndescription: {skill}\n---\n");
        world.repo(repo, &[(file.as_str(), text.as_str())]);
    }
    let team = format!("```yaml\n{ROLES}```\n");
    world.repo("acme/team", &[("team.md", &team), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: \"true\"\nteam: acme/team\n```\n";
    common::write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// One tick of a bound repo at `now`, every worker started being at work.
fn tick_at_work(world: &World, now: u64) -> Value {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
                terminal["lastOutputAt"] = json!(now * 1000);
            }
        }
    });
    tick(world, &now.to_string())
}

#[test]
fn a_stage_record_is_written_to_the_local_tracker_and_read_back_to_advance_the_issue() {
    let world = bound();
    let number = open(&world, "Feature", &["flow:feature"]);

    let ticked = tick_at_work(&world, 1000);
    assert_eq!(did(&ticked), ["dispatch 1 done", "stage_label 1 done"], "{ticked}");
    let line = ["verdict", "pass", "--issue", "1", "--stage", "build", "--text", "Built."];
    let declared =
        world.afk(&[&line[..], &["--role-commit", "r1", "--team-commit", "t1"]].concat());
    assert_eq!(declared.ok()["declared"], true);
    let said = comments(&mut world.local_tracker(), number).join("\n");
    assert!(said.contains("Built."), "the stage record is on the local issue: {said}");

    let ticked = tick_at_work(&world, 1090);
    assert_eq!(
        did(&ticked),
        ["advance 1 done", "dispatch 1 done", "stage_label 1 done"],
        "{ticked}"
    );
    let labels = world.local_tracker().view(number).unwrap().issue.labels;
    assert_eq!(labels, ["flow:feature", "stage:review"]);
    assert_eq!(issue_calls(&world), [] as [Vec<String>; 0]);
}

#[test]
fn doctor_finds_a_ready_local_issue_no_fleet_has_taken_by_when_its_label_was_put_on() {
    let world = world();
    let config = "```yaml\nbase_branch: main\ngate_command: 'true'\n```\n";
    world.repo(REPO, &[("docs/agents/afkconvoy.md", config)]);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut store = world.local_tracker();
    let number = store.create(REPO, "Waits", "", &[], now - 10_000).unwrap();
    let observed = |world: &World| {
        let answer = world.afk(&["doctor", "--all"]).json;
        let rows =
            answer["host"]["unworked_repos"].as_array().unwrap_or_else(|| panic!("{answer}"));
        rows.iter().find(|row| row["repo"] == REPO).unwrap_or_else(|| panic!("{answer}")).clone()
    };

    // Made ready under an hour ago: young.
    store.add_labels(number, &["ready-for-agent"], now - 3540).unwrap();
    let young = observed(&world);
    assert_eq!((&young["tracker"], &young["local"]), (&json!(TRACKER), &json!(true)), "{young}");
    assert_eq!(young["waiting"], json!([]), "{young}");
    assert!(young["error"].is_null(), "{young}");

    // Made ready over an hour ago, or taken off and put on again since.
    let mut ready_at = |at: u64| {
        store.remove_labels(number, &["ready-for-agent"], at).unwrap();
        store.add_labels(number, &["ready-for-agent"], at).unwrap();
    };
    ready_at(now - 3700);
    let old = observed(&world);
    assert_eq!(old["waiting"][0]["issue"], number, "{old}");
    ready_at(now - 3540);
    assert_eq!(observed(&world)["waiting"], json!([]));
    ready_at(now - 3700);

    // A blocker closed under an hour ago holds it young; a claim excludes it.
    let blocker = store.create(REPO, "Blocker", "", &[], now - 10_000).unwrap();
    store.add_blocker(number, blocker, now - 3700).unwrap();
    assert_eq!(observed(&world)["waiting"], json!([]));
    store.close(blocker, now - 3540).unwrap();
    assert_eq!(observed(&world)["waiting"], json!([]));
    store.reopen(blocker, now).unwrap();
    store.close(blocker, now - 3700).unwrap();
    assert_eq!(observed(&world)["waiting"][0]["issue"], number);
    git(&world.bare, &["update-ref", "refs/convoy/claim/local/h1/1", "refs/heads/main"]);
    let claimed = observed(&world);
    assert_eq!(claimed["waiting"], json!([]), "{claimed}");
}

/// `afkconvoy issue` in the world's checkout, as of `now`.
fn issue(world: &World, now: &str, args: &[&str]) -> common::Answer {
    let mut all = vec!["issue"];
    all.extend_from_slice(args);
    all.extend(["--now", now]);
    world.afk(&all)
}

#[test]
fn afkconvoy_issue_opens_reads_and_lists_the_local_trackers_issues() {
    let world = world();
    let created = issue(
        &world,
        "100",
        &["create", "-t", "Add plan", "-b", "why", "-l", "bug,ready-for-agent"],
    )
    .ok();
    assert_eq!(
        (created["action"].as_str(), created["number"].as_u64()),
        (Some("created"), Some(1))
    );
    assert_eq!(created["issue"], "local/h1#1");
    let body = world.root.join("body.md");
    std::fs::write(&body, "from a file").unwrap();
    let second = issue(
        &world,
        "110",
        &["create", "--title", "Second", "--body-file", body.to_str().unwrap()],
    )
    .ok();
    assert_eq!(second["number"], 2);
    let stored = world.local_tracker().view(1).unwrap().issue;
    assert_eq!(
        (stored.repo.as_str(), stored.title.as_str(), stored.created),
        (REPO, "Add plan", 100)
    );
    assert_eq!(stored.labels, ["bug", "ready-for-agent"]);

    let viewed = issue(&world, "120", &["view", "1"]).ok();
    assert_eq!(viewed["title"], "Add plan");
    assert_eq!(viewed["body"], "why");
    assert_eq!(viewed["state"], "OPEN");
    assert_eq!(viewed["repo"], REPO);
    assert_eq!(viewed["url"], "local/h1#1");
    assert_eq!(viewed["labels"], json!([{"name": "bug"}, {"name": "ready-for-agent"}]));
    assert!(viewed.get("comments").is_none(), "comments only when asked: {viewed}");
    let fields =
        issue(&world, "120", &["view", "2", "--json", "number,title,body,labels,comments,state"])
            .ok();
    assert_eq!(
        fields,
        json!({"number": 2, "title": "Second", "body": "from a file", "labels": [], "comments": [], "state": "OPEN"})
    );
    let refused = issue(&world, "120", &["view", "1", "--json", "assignees"]).error();
    assert!(refused.contains("no field `assignees`"), "{refused}");
    let missing = issue(&world, "120", &["view", "9"]).error();
    assert!(missing.contains("no such issue"), "{missing}");

    let listed = issue(&world, "130", &["list"]).ok();
    let numbers: Vec<u64> = listed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [1, 2]);
    let labelled = issue(&world, "130", &["list", "--label", "bug", "--json", "number"]).ok();
    assert_eq!(labelled, json!({"issues": [{"number": 1}]}));
    // Nothing reached GitHub's issues.
    assert_eq!(issue_calls(&world), [] as [Vec<String>; 0]);
}

#[test]
fn afkconvoy_issue_comments_edits_closes_reopens_and_blocks_a_local_issue() {
    let world = world();
    let one = open(&world, "One", &["ready-for-agent"]);
    let two = open(&world, "Two", &[]);
    let (one_s, two_s) = (one.to_string(), two.to_string());

    let said = issue(&world, "200", &["comment", &one_s, "--body", "looked"]).ok();
    assert_eq!(said["action"], "commented");
    let viewed = issue(&world, "201", &["view", &one_s, "--comments"]).ok();
    let comment = &viewed["comments"][0];
    assert_eq!(
        (comment["body"].as_str(), comment["createdAt"].as_str()),
        (Some("looked"), Some("1970-01-01T00:03:20Z"))
    );
    assert_eq!(comment["url"], format!("local/h1#{one}/comments/{}", comment["id"]));

    issue(
        &world,
        "210",
        &[
            "edit",
            &one_s,
            "--title",
            "One, renamed",
            "--body",
            "new",
            "--add-label",
            "bug",
            "--remove-label",
            "ready-for-agent",
        ],
    )
    .ok();
    let edited = world.local_tracker().view(one).unwrap().issue;
    assert_eq!(
        (edited.title.as_str(), edited.body.as_str(), edited.body_edited),
        ("One, renamed", "new", Some(210))
    );
    assert_eq!(edited.labels, ["bug"]);
    let nothing = issue(&world, "211", &["edit", &one_s]).error();
    assert!(nothing.contains("nothing to edit"), "{nothing}");

    let blocked = issue(&world, "220", &["block", &one_s, "--by", &two_s]).ok();
    assert_eq!(blocked["open_blockers"], json!([two]));
    let viewed = issue(&world, "221", &["view", &one_s, "--json", "blockedBy"]).ok();
    assert_eq!(viewed, json!({"blockedBy": [{"number": two, "state": "OPEN"}]}));
    let cycle = issue(&world, "222", &["block", &two_s, "--by", &one_s]).error();
    assert!(cycle.contains("cycle"), "{cycle}");

    let closed = issue(&world, "230", &["close", &two_s, "--comment", "done elsewhere"]).ok();
    assert_eq!(closed["action"], "closed");
    let two_now = world.local_tracker().view(two).unwrap();
    assert_eq!(two_now.issue.closed, Some(230));
    assert_eq!(two_now.comments[0].body, "done elsewhere");
    let listed =
        issue(&world, "231", &["list", "--state", "closed", "--json", "number,state"]).ok();
    assert_eq!(listed, json!({"issues": [{"number": two, "state": "CLOSED"}]}));
    assert_eq!(
        issue(&world, "232", &["view", &one_s, "--json", "blockedBy"]).ok()["blockedBy"][0]["state"],
        "CLOSED"
    );

    issue(&world, "240", &["reopen", &two_s]).ok();
    assert_eq!(world.local_tracker().view(two).unwrap().issue.closed, None);
    let unblocked = issue(&world, "250", &["unblock", &one_s, "--by", &two_s]).ok();
    assert_eq!(unblocked["open_blockers"], json!([]));
    assert!(world.local_tracker().blockers(one).unwrap().is_empty());
    assert_eq!(issue_calls(&world), [] as [Vec<String>; 0]);
}

#[test]
fn afkconvoy_issue_opens_one_of_another_repo_only_where_the_host_has_a_checkout_of_it() {
    let world = world();
    let refused =
        issue(&world, "100", &["create", "-t", "Theirs", "-b", "", "--repo", "acme/web"]).error();
    assert!(refused.contains("this host has no checkout of acme/web"), "{refused}");
    assert!(
        world
            .local_tracker()
            .list(&Filter { state: Which::All, ..Filter::default() })
            .unwrap()
            .is_empty()
    );
    let web = world.second_repo();
    world.name_origin(&web, "acme/web");
    world.say_local_tracker(&web);
    let opened =
        issue(&world, "110", &["create", "-t", "Theirs", "-b", "", "--repo", "acme/web"]).ok();
    let stored = world.local_tracker().view(opened["number"].as_u64().unwrap()).unwrap().issue;
    assert_eq!(stored.repo, "acme/web");
    // Of this checkout's own repo, as named, it is this checkout's.
    let own = issue(&world, "120", &["create", "-t", "Ours", "-b", "", "--repo", REPO]).ok();
    assert_eq!(
        world.local_tracker().view(own["number"].as_u64().unwrap()).unwrap().issue.repo,
        REPO
    );
}

#[test]
fn the_flows_line_of_a_local_tracker_opens_a_local_issue_from_anywhere_on_the_host() {
    let world = world();
    let flows = world.afk(&["flows"]).ok();
    let line = flows["line"].as_str().unwrap();
    assert_eq!(
        line,
        "afkconvoy issue create --repo acme/dev --label 'ready-for-agent' --title '<title>' --body-file <file>"
    );
    let body = world.root.join("body.md");
    std::fs::write(&body, "the body").unwrap();
    let line = line.replace("'<title>'", "Flowed").replace("<file>", body.to_str().unwrap());
    // As a shell reads it: its quotes are no part of a word.
    let line = line.replace('\'', "");
    let words: Vec<&str> = line.split_whitespace().skip(1).collect();
    let opened = world.afk(&words).ok();
    let stored = world.local_tracker().view(opened["number"].as_u64().unwrap()).unwrap().issue;
    assert_eq!(
        (stored.title.as_str(), stored.body.as_str(), stored.repo.as_str()),
        ("Flowed", "the body", REPO)
    );
    assert_eq!(stored.labels, ["ready-for-agent"]);
    assert_eq!(issue_calls(&world), [] as [Vec<String>; 0]);
}

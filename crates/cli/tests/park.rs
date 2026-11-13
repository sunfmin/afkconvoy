use crate::common;

use std::path::PathBuf;

use common::{World, commit};
use serde_json::{Value, json};

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        // the status boards a tick brings up to date are `board.rs`'s to test
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world with a worker dispatched on issue 7 at 1000.
fn fleet() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done", "heartbeat done"]);
    world
}

fn worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().find(|row| row["linkedIssue"] == 7).expect("the worktree of issue 7");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The worker on issue 7 declares itself blocked by `blockers`.
fn blocked_by(world: &World, blockers: &str) -> String {
    world.comment(7, &format!("<!--convoy:verdict phase=blocked blocked_by={blockers}-->\nwhy"))
}

/// An open issue that is not on the frontier, but that an open PR closes.
fn in_flight(world: &World, number: u64) {
    world.issue(number, &["bug"]);
    world.pr(number * 10, number, &format!("issue-{number}"));
}

fn park(world: &World, instance: &str) -> common::Answer {
    world.afk(&["park", "--issue", "7", "--instance", instance, "--now", "1100"])
}

fn excluded(world: &World, number: u64) -> Option<String> {
    let plan = world.afk(&["plan", "--now", "1100"]).ok();
    let rows = plan["excluded"].as_array().unwrap();
    let row = rows.iter().find(|row| row["number"] == number)?;
    Some(row["reason"].as_str().unwrap().to_string())
}

fn escalation(world: &World) -> String {
    let comments = world.comments(7);
    assert_eq!(
        comments.iter().filter(|body| body.starts_with("<!--convoy:account ")).count(),
        1,
        "an escalation leaves one work account: {comments:?}"
    );
    comments
        .into_iter()
        .find(|body| body.starts_with("<!--convoy:escalation"))
        .expect("an escalation comment")
}

#[test]
fn a_claim_waiting_on_open_blockers_is_parked_edges_first() {
    let world = fleet();
    in_flight(&world, 41);
    in_flight(&world, 43);
    world.issue(42, &[]);
    world.close(42, "completed");
    blocked_by(&world, "41,42,43");
    let dir = worktree(&world);

    // cut between the edges and the release: the issue is held back already
    world.watch_pushes(Some("claim/7"));
    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked), ["judge 7 failed", "heartbeat done"]);
    assert_eq!(world.edges(7), [41, 43], "an edge to each blocker still open");
    assert_eq!(world.claims().len(), 1);

    // picked up again: no edge is written twice
    world.watch_pushes(None);
    world.gh_calls();
    assert_eq!(did(&tick(&world, 1180)), ["judge 7 done"]);
    let posts = world.gh_calls().iter().filter(|call| call.contains(&"POST".to_string())).count();
    assert_eq!(posts, 0);
    assert_eq!(world.edges(7), [41, 43]);
    assert_eq!(world.claims(), []);
    assert_eq!(world.state(7), "open");
    assert_eq!(world.labels(7), ["ready-for-agent"], "the ready label is kept");
    assert!(!dir.exists(), "a worktree with no work is removed");
    let comments = world.comments(7);
    assert_eq!(comments.len(), 3, "the branch, the worker, and the verdict: {comments:?}");
    assert!(comments.iter().any(|body| body.contains("<!--convoy:branch ")), "{comments:?}");
    assert!(comments.iter().any(|body| body.starts_with("<!--convoy:worker ")), "{comments:?}");
    assert!(comments.iter().any(|body| body.contains("<!--convoy:verdict ")), "{comments:?}");
    assert!(
        !comments.iter().any(|body| body.starts_with("<!--convoy:account ")),
        "parking is not an end: {comments:?}"
    );

    // off the frontier exactly while a blocker it named is open
    assert_eq!(excluded(&world, 7).as_deref(), Some("open blockers: #41, #43"));
    assert_eq!(did(&tick(&world, 1270)), [] as [String; 0]);
    world.close(41, "completed");
    assert_eq!(excluded(&world, 7).as_deref(), Some("open blockers: #43"));
    world.close(43, "completed");
    assert_eq!(excluded(&world, 7), None);
    assert_eq!(did(&tick(&world, 1360)), ["dispatch 7 done", "heartbeat done"]);
}

#[test]
fn a_parked_worktree_with_work_in_it_is_kept() {
    for dirty in [false, true] {
        let world = fleet();
        let dir = worktree(&world);
        if dirty {
            std::fs::write(dir.join("scratch.txt"), "x").unwrap();
        } else {
            commit(&dir, "work.txt", "x\n");
        }
        // a blocker another fleet holds, and one that is ready for one
        world.issue(41, &["bug"]);
        world.afk(&["claim", "--issue", "41", "--instance", "beta", "--now", "1000"]).ok();
        blocked_by(&world, "#41");
        assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
        assert_eq!(world.edges(7), [41]);
        assert_eq!(world.claims(), [(41, "beta".to_string())]);
        assert!(dir.exists(), "what a later continuation starts from");
        assert_eq!(world.open_terminals(), Vec::<String>::new(), "its worker is ended");
    }
}

#[test]
fn a_blocker_nothing_will_resolve_escalates() {
    type Arrange<'a> = &'a dyn Fn(&World);
    let cases: [(&str, Arrange, &str); 8] = [
        ("99", &|_| {}, "#99 could not be read"),
        ("41", &|world| world.close(41, "not_planned"), "#41 was closed as not planned"),
        ("41", &|world| world.close(41, "duplicate"), "#41 was closed as a duplicate"),
        (
            "41",
            &|world| {
                world.with_gh(|gh| {
                    let issues = gh["issues"].as_array_mut().unwrap();
                    issues.iter_mut().find(|i| i["number"] == 41).unwrap()["pull_request"] =
                        json!(true);
                })
            },
            "#41 is a pull request",
        ),
        ("44", &|_| {}, "#44 will never be dispatched: epic label (epic)"),
        ("45", &|_| {}, "#45 is open but no fleet will work it"),
        ("7", &|_| {}, "#7 is the issue itself"),
        ("", &|_| {}, "named no blocker"),
    ];
    for (named, arrange, says) in cases {
        let world = fleet();
        world.issue(41, &["bug"]);
        // an epic an open PR closes: still never dispatched
        world.issue(44, &["epic", "ready-for-agent"]);
        world.pr(440, 44, "issue-44");
        world.issue(45, &["bug"]);
        // one more that is waiting: a single unmet blocker is enough
        in_flight(&world, 46);
        arrange(&world);
        let said = blocked_by(&world, &format!("46,{named}"));
        let verdict = if named.is_empty() {
            world.comment(7, "<!--convoy:verdict phase=blocked-->\nwhy")
        } else {
            said
        };

        assert_eq!(did(&tick(&world, 1090))[0], "judge 7 done", "{named}");
        assert_eq!(world.claims(), [], "{named}");
        assert_eq!(world.labels(7), ["ready-for-human"]);
        assert_eq!(world.edges(7), [] as [u64; 0], "nothing is recorded for a human's case");
        let escalation = escalation(&world);
        assert!(escalation.contains(says), "{escalation}");
        assert!(escalation.contains(&verdict), "{escalation}");
    }
}

#[test]
fn a_blocker_that_already_depends_on_the_issue_escalates() {
    for chain in [json!({"41": [7]}), json!({"41": [50], "50": [51, 7], "51": []})] {
        let world = fleet();
        in_flight(&world, 41);
        in_flight(&world, 50);
        in_flight(&world, 51);
        world.with_gh(|gh| gh["edges"] = chain.clone());
        blocked_by(&world, "41");
        assert_eq!(did(&tick(&world, 1090))[0], "judge 7 done");
        let escalation = escalation(&world);
        assert!(escalation.contains("#41 already depends on #7"), "{escalation}");
        assert!(escalation.contains("close a cycle"));
        assert_eq!(world.edges(7), [] as [u64; 0]);
    }

    // a chain behind a blocker that cannot be read is an error, never "no cycle"
    let world = fleet();
    in_flight(&world, 41);
    blocked_by(&world, "41");
    world.with_gh(|gh| gh["fail"] = json!(["issues/41/dependencies"]));
    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked)[0], "judge 7 failed");
    assert_eq!(world.claims().len(), 1);
    assert_eq!(world.edges(7), [] as [u64; 0]);
}

#[test]
fn every_blocker_closed_starts_a_worker_on_the_issue_again() {
    let world = fleet();
    let dir = worktree(&world);
    commit(&dir, "work.txt", "x\n");
    world.issue(41, &[]);
    world.close(41, "completed");
    blocked_by(&world, "41");
    let first = world.orca()["terminals"].as_array().unwrap().len();

    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked), ["judge 7 continued", "heartbeat done"]);
    assert_eq!(world.claims().len(), 1, "the claim is held throughout");
    assert!(dir.exists());
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let open: Vec<&Value> = terminals.iter().filter(|t| t["open"] == true).collect();
    assert_eq!((terminals.len(), open.len()), (first + 1, 1), "a new worker, the only one there");
    let sent = open[0]["sent"].as_array().unwrap();
    assert!(sent[0]["text"].as_str().unwrap().contains("convoy-worker-prompt.md"));

    // The answered verdict is not the new worker's. Unknown silence must
    // be diagnosed rather than consuming a nudge/escalation ladder.
    for now in [1180, 1390, 1690] {
        assert_eq!(did(&tick(&world, now))[0], "judge 7 nothing");
    }
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn blockers_closed_before_parking_continue_the_same_worktree_after_a_fleet_lock() {
    for locked in [false, true] {
        let world = fleet();
        world.list_checkout(&world.clone);
        let dir = worktree(&world);
        let tip = commit(&dir, "work.txt", "kept work\n");
        std::fs::write(dir.join("scratch.txt"), "uncommitted work\n").unwrap();
        in_flight(&world, 41);
        blocked_by(&world, "41");
        world.with_orca(|orca| {
            let terminal = &mut orca["terminals"][0];
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": 1000000});
            terminal["lastOutputAt"] = json!(1000000);
        });

        if locked {
            world.afk(&["lock", "--issue", "41"]).ok();
            assert_eq!(did(&tick(&world, 1390)), ["heartbeat done"]);
            assert_eq!(world.claims(), [(7, "alpha".to_string())]);
            assert_eq!(world.edges(7), [] as [u64; 0]);
        }
        world.close(41, "completed");
        let ticked = tick(&world, 1480);
        assert_eq!(did(&ticked), ["judge 7 continued", "heartbeat done"], "{ticked}");
        assert_eq!(world.claims(), [(7, "alpha".to_string())]);
        assert_eq!(worktree(&world), dir);
        assert_eq!(common::git(&dir, &["rev-parse", "HEAD"]), tip);
        assert_eq!(std::fs::read_to_string(dir.join("scratch.txt")).unwrap(), "uncommitted work\n");
        assert_eq!(world.open_terminals().len(), 1);
        assert_eq!(world.orca()["terminals"].as_array().unwrap().len(), 2);
        assert_eq!(did(&tick(&world, 1570))[0], "judge 7 nothing");

        // Only the answered comment is retired. The successor's own verdict
        // still stands, even when it names the same already-closed blocker.
        blocked_by(&world, "41");
        assert_eq!(did(&tick(&world, 1660))[0], "judge 7 continued");
        assert_eq!(did(&tick(&world, 1750))[0], "judge 7 nothing");
    }
}

#[test]
fn a_failed_continuation_of_a_cleared_blocked_verdict_is_completed_at_the_next_tick() {
    let world = fleet();
    let dir = worktree(&world);
    commit(&dir, "work.txt", "kept work\n");
    world.issue(41, &[]);
    world.close_issue(41);
    blocked_by(&world, "41");
    world.with_orca(|orca| orca["send_refused"] = json!(true));
    let refused = world.afk(&["dispatch", "--issue", "7", "--instance", "alpha", "--now", "1090"]);
    assert!(refused.error().contains("did not accept"));
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    world.with_orca(|orca| orca["send_refused"] = json!(false));
    assert_eq!(did(&tick(&world, 1180)), ["judge 7 continued", "heartbeat done"]);
    assert_eq!(did(&tick(&world, 1270))[0], "judge 7 nothing");
    assert_eq!(worktree(&world), dir);
    assert_eq!(world.open_terminals().len(), 1);
}

#[test]
fn park_reads_the_verdict_again_and_refuses_what_is_not_parkable_now() {
    let world = fleet();
    in_flight(&world, 41);
    world.issue(42, &["bug"]);
    let untouched = |world: &World| {
        assert_eq!(world.claims(), [(7, "alpha".to_string())]);
        assert_eq!(world.edges(7), [] as [u64; 0]);
        assert_eq!(world.labels(7), ["ready-for-agent"]);
    };

    let error = park(&world, "alpha").error();
    assert!(error.contains("latest verdict is not `blocked`"), "{error}");
    world.comment(7, "<!--convoy:verdict phase=escalate-->\nwhy");
    assert!(park(&world, "alpha").error().contains("not `blocked`"));

    blocked_by(&world, "41,42");
    let error = park(&world, "alpha").error();
    assert!(error.contains("#42 is open but no fleet will work it"), "{error}");
    assert!(error.contains("escalate it instead") && error.contains("nothing was changed"));
    untouched(&world);

    blocked_by(&world, "41");
    let error = park(&world, "beta").error();
    assert!(error.contains("alpha"), "{error}");
    untouched(&world);

    world.close(41, "completed");
    let error = park(&world, "alpha").error();
    assert!(error.contains("dispatch it again instead"), "{error}");
    untouched(&world);

    in_flight(&world, 43);
    blocked_by(&world, "41, 43");
    let dir = worktree(&world);
    let answer = park(&world, "alpha").ok();
    assert_eq!(answer["action"], "parked");
    assert_eq!(answer["blocked_by"], json!([43]));
    assert_eq!(
        (answer["released"].clone(), answer["worktree"].clone()),
        (json!(true), json!("removed"))
    );
    assert_eq!(world.edges(7), [43]);
    assert_eq!(world.claims(), []);
    assert!(!dir.exists());

    // run once more: there is no claim left to park
    let again = park(&world, "alpha").ok();
    assert!(again["detail"].as_str().unwrap().contains("no claim is held"));
}

#[test]
fn a_parked_worktree_the_remote_holds_all_of_is_removed_and_recreated_at_the_pushed_tip() {
    let world = fleet();
    let dir = worktree(&world);
    let pushed = commit(&dir, "work.txt", "x\n");
    common::git(&dir, &["push", "-q", "origin", "HEAD"]);
    in_flight(&world, 41);
    blocked_by(&world, "41");

    assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
    assert_eq!(world.claims(), []);
    assert!(!dir.exists(), "a commit the remote holds is no reason to keep it");
    let board = world.boards(7).join("\n");
    assert!(board.contains("**Parked**") && !board.contains("worktree"), "{board}");

    // its blocker closes: the issue is worked again from the pushed branch
    world.close(41, "completed");
    assert_eq!(did(&tick(&world, 1180)), ["dispatch 7 done", "heartbeat done"]);
    let again = worktree(&world);
    assert_eq!(common::git(&again, &["rev-parse", "HEAD"]), pushed, "recreated at the pushed tip");
    assert_eq!(common::git(&again, &["log", "-1", "--format=%s"]), "work.txt");
}

#[test]
fn a_parked_worktree_holding_what_the_remote_does_not_is_kept_saying_why() {
    for (dirty, because, unpushed) in [(true, "dirty", 0), (false, "unpushed", 1)] {
        let world = fleet();
        let dir = worktree(&world);
        if dirty {
            // a pushed commit, and something uncommitted beside it
            commit(&dir, "work.txt", "x\n");
            common::git(&dir, &["push", "-q", "origin", "HEAD"]);
            std::fs::write(dir.join("scratch.txt"), "x").unwrap();
        } else {
            commit(&dir, "work.txt", "x\n");
        }
        in_flight(&world, 41);
        blocked_by(&world, "41");

        let answer = park(&world, "alpha").ok();
        assert_eq!(answer["worktree"], "kept", "{answer}");
        assert_eq!(answer["kept_because"], because, "{answer}");
        assert_eq!(answer["unpushed"], unpushed, "{answer}");
        assert!(dir.exists());
        let board = world.boards(7).join("\n");
        let holds = if dirty { "uncommitted changes" } else { "1 commit the remote does not have" };
        assert!(
            board.contains(&format!(
                "Its worktree is kept on the host that worked it: it holds {holds}"
            )),
            "{board}"
        );
    }
}

#[test]
fn escalate_says_what_a_kept_worktree_holds_and_lists_its_unpushed_commits() {
    let world = fleet();
    let dir = worktree(&world);
    commit(&dir, "pushed.txt", "x\n");
    common::git(&dir, &["push", "-q", "origin", "HEAD"]);
    commit(&dir, "first.txt", "x\n");
    commit(&dir, "second.txt", "x\n");
    let args = ["escalate", "--issue", "7", "--instance", "alpha", "--reason", "stuck"];
    let answer = world.afk(&[&args[..], &["--now", "1100"]].concat()).ok();
    assert_eq!(
        (&answer["worktree"], &answer["kept_because"], &answer["unpushed"]),
        (&json!("kept"), &json!("unpushed"), &json!(2)),
        "{answer}"
    );
    assert!(dir.exists());
    let said = escalation(&world);
    assert!(
        said.contains("it holds 2 commits the remote does not have.\n- second.txt\n- first.txt")
    );
    assert!(!said.contains("pushed.txt"), "{said}");
    assert!(world.boards(7).join("\n").contains("it holds 2 commits"));
}

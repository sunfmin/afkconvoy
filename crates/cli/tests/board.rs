use crate::common;

use std::path::PathBuf;

use common::{World, commit, git};
use serde_json::{Value, json};

fn tick_as(world: &World, instance: &str, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", instance, "--host", "h1", "--now", &now];
    world.afk(&[&args[..], &[]].concat()).ok()
}

fn tick(world: &World, now: u64) -> Value {
    tick_as(world, "alpha", now)
}

/// Everything a tick did, the status boards it brought up to date among it.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world with a worker dispatched on issue 7 at 1000, and at work.
fn fleet() -> World {
    let world = World::new();
    world.set_gate("true");
    world.issue(7, &["ready-for-agent"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done", "board 7 done", "heartbeat done"]);
    at_work(&world, 1000);
    world
}

/// The workers report they are at work, their last output at `now`.
fn at_work(world: &World, now: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
            terminal["lastOutputAt"] = json!(now * 1000);
        }
    });
}

/// The workers' turns ended at `now`.
fn idle(world: &World, now: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": now * 1000});
            terminal["lastOutputAt"] = json!(now * 1000);
        }
    });
}

/// The worker stopped, long enough ago to be past any grace.
fn stopped(world: &World) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": 1});
            terminal["lastOutputAt"] = json!(1);
        }
    });
}

fn worktree(world: &World) -> PathBuf {
    PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap())
}

/// The one status board of issue 7.
fn board(world: &World) -> String {
    let boards = world.boards(7);
    assert_eq!(boards.len(), 1, "one comment per issue: {boards:?}");
    boards[0].clone()
}

/// The steps of the board's task list that are ticked.
fn ticked(board: &str) -> Vec<String> {
    let done = board.lines().filter_map(|line| line.strip_prefix("- [x] "));
    done.map(str::to_string).collect()
}

/// The id of the comment the board of issue 7 is.
fn board_id(world: &World) -> u64 {
    let gh = world.gh();
    let rows = gh["comments"]["7"].as_array().unwrap();
    let row = rows.iter().find(|c| c["body"].as_str().unwrap().contains("convoy:status")).unwrap();
    row["id"].as_u64().unwrap()
}

/// The `gh` calls since the last look that read or wrote a comment.
fn comment_calls(world: &World) -> Vec<String> {
    let calls = world.gh_calls();
    let about = calls.iter().map(|call| call.join(" ")).filter(|call| call.contains("/comments"));
    about.map(|call| call.split(" -f ").next().unwrap().to_string()).collect()
}

#[test]
fn one_comment_follows_a_claim_from_claimed_to_merged() {
    let world = fleet();
    let id = board_id(&world);
    let claimed = board(&world);
    assert!(claimed.starts_with("<!--convoy:status-->\n"), "{claimed}");
    let by =
        format!("is being worked by fleet instance `alpha`, afkconvoy `{}`.", afkconvoy::VERSION);
    assert!(claimed.contains(&by), "{claimed}");
    assert_eq!(ticked(&claimed), ["Claimed"]);

    // its PR opens
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    at_work(&world, 1090);
    assert_eq!(did(&tick(&world, 1090)), ["watch 7 nothing", "board 7 done", "heartbeat done"]);
    assert_eq!(ticked(&board(&world)), ["Claimed", "PR open (#70)", "Taking a turn"]);

    // The worker's turn lands immediately; the next tick settles its claim.
    let landed = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1100"]).ok();
    assert_eq!(landed["outcome"], "merged");
    assert_eq!(landed["recorded"], true);
    // It woke the fleet and stopped at its prompt: settling ends it.
    idle(&world, 1150);
    assert_eq!(did(&tick(&world, 1180)), ["settle 7 done"]);
    let merged = board(&world);
    assert_eq!(ticked(&merged).len(), 4);
    assert!(merged.contains("was worked by fleet instance `alpha`"), "{merged}");
    assert_eq!(board_id(&world), id, "edited in place, start to finish");
    assert_eq!(world.claims(), []);
}

#[test]
fn a_board_is_rewritten_only_when_its_text_changed() {
    let world = fleet();
    comment_calls(&world);
    for now in [1090, 1180] {
        at_work(&world, now);
        let again = tick(&world, now);
        assert_eq!(did(&again), ["judge 7 nothing", "board 7 nothing", "heartbeat done"]);
    }
    assert_eq!(comment_calls(&world), [] as [String; 0], "nothing read, nothing written");

    // another checkout, with nothing remembered: the board is found, and left as it is
    std::fs::remove_dir_all(world.clone.join(".git/convoy-boards")).unwrap();
    at_work(&world, 1270);
    assert_eq!(did(&tick(&world, 1270))[1], "board 7 nothing");
    let calls = comment_calls(&world);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].contains("--paginate") && calls[0].contains("issues/7/comments"));

    // a board somebody edited is put back, in place
    let id = board_id(&world);
    world.with_gh(|gh| {
        for comment in gh["comments"]["7"].as_array_mut().unwrap() {
            if comment["id"] == id {
                comment["body"] = json!("<!--convoy:status-->\nscribbled over");
            }
        }
    });
    std::fs::remove_dir_all(world.clone.join(".git/convoy-boards")).unwrap();
    at_work(&world, 1360);
    assert_eq!(did(&tick(&world, 1360))[1], "board 7 done");
    assert_eq!(ticked(&board(&world)), ["Claimed"]);
    assert_eq!(board_id(&world), id);
}

#[test]
fn the_last_word_is_on_the_board_before_the_claim_is_released() {
    type Arrange = fn(&World);
    // (what happens, the step that ends the claim, the one that finishes a cut ending, the word)
    let endings: [(Arrange, &str, &str, &str); 4] = [
        (
            |world| {
                world.comment(7, "<!--convoy:verdict phase=escalate-->\nstuck");
            },
            "judge",
            "judge",
            "**Escalated to a human**",
        ),
        (
            |world| {
                world.comment(7, "<!--convoy:verdict phase=already-satisfied-->\ndone already");
            },
            "judge",
            // the issue is closed by then: the claim is settled
            "settle",
            "**Closed with nothing landed**",
        ),
        (
            |world| {
                world.issue(41, &["bug"]);
                world.pr(410, 41, "issue-41");
                world.comment(7, "<!--convoy:verdict phase=blocked blocked_by=41-->\nneeds 41");
            },
            "judge",
            "judge",
            "**Parked** — waiting on #41.",
        ),
        (|world| world.close_issue(7), "settle", "settle", "**Closed with nothing landed**"),
    ];
    for (arrange, step, finish, says) in endings {
        let world = fleet();
        arrange(&world);
        stopped(&world);
        // cut at the release: the remote refuses the claim's deletion
        world.watch_pushes(Some("claim/7"));
        assert_eq!(did(&tick(&world, 1400))[0], format!("{step} 7 failed"), "{says}");
        assert_eq!(world.claims().len(), 1, "the claim is still held");
        let last = board(&world);
        assert!(last.contains(says), "{last}");
        assert!(last.contains("was worked by fleet instance `alpha`"));
        assert_eq!(ticked(&last), ["Claimed"], "an off-ramp keeps what was reached");

        // finished by the next tick, with nothing more written on the board
        world.watch_pushes(None);
        let id = board_id(&world);
        comment_calls(&world);
        assert_eq!(did(&tick(&world, 1490)), [format!("{finish} 7 done")], "{says}");
        assert_eq!(world.claims(), []);
        assert_eq!((board(&world), board_id(&world)), (last, id));
        let writes = comment_calls(&world);
        let board = format!("/issues/comments/{id}");
        assert!(
            !writes.iter().any(|call| call.contains("PATCH") && call.contains(&board)),
            "the board is left as the cut tick wrote it: {writes:?}"
        );
    }
}

#[test]
fn an_escalated_joining_claim_keeps_its_pr_on_the_board() {
    let world = fleet();
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    stopped(&world);
    let escalate = ["escalate", "--issue", "7", "--instance", "alpha", "--now", "1100"];
    world.afk(&[&escalate[..], &["--reason", "by hand"]].concat()).ok();
    let last = board(&world);
    assert_eq!(ticked(&last), ["Claimed", "PR open (#70)", "Taking a turn"]);
    assert!(last.contains("**Escalated to a human**"));
}

#[test]
fn an_issue_claimed_again_keeps_its_one_board() {
    let world = fleet();
    world.comment(7, "<!--convoy:verdict phase=escalate-->\nstuck");
    stopped(&world);
    assert_eq!(did(&tick(&world, 1400)), ["judge 7 done"]);
    let id = board_id(&world);
    assert!(board(&world).contains("**Escalated to a human**"));

    // a human answers, and hands it back
    world.with_gh(|gh| {
        gh["issues"][0]["labels"] = json!(["ready-for-agent"]);
        gh["comments"]["7"]
            .as_array_mut()
            .unwrap()
            .retain(|c| !c["body"].as_str().unwrap().contains("convoy:verdict"));
    });
    assert_eq!(did(&tick_as(&world, "beta", 1500))[..2], ["dispatch 7 done", "board 7 done"]);
    let again = board(&world);
    assert!(again.contains("is being worked by fleet instance `beta`"), "{again}");
    assert_eq!(ticked(&again), ["Claimed"]);
    assert_eq!(board_id(&world), id);
}

#[test]
fn a_live_peers_board_is_left_alone() {
    let world = World::new();
    for n in [7, 8] {
        world.issue(n, &["ready-for-agent"]);
    }
    tick_as(&world, "beta", 1000);
    world.issue(9, &["ready-for-agent"]);
    let theirs = world.boards(7);
    assert!(theirs[0].contains("fleet instance `beta`"));
    comment_calls(&world);

    let ticked = tick(&world, 1010);
    assert_eq!(did(&ticked), ["dispatch 9 done", "board 9 done", "heartbeat done"]);
    assert_eq!(world.boards(7), theirs);
    assert!(world.boards(8)[0].contains("fleet instance `beta`"));
    assert!(world.boards(9)[0].contains("fleet instance `alpha`"));
    let calls = comment_calls(&world);
    assert!(!calls.iter().any(|call| call.contains("issues/7/") || call.contains("issues/8/")));
    assert!(!calls.iter().any(|call| call.contains("PATCH")), "{calls:?}");
}

mod common;

use common::{World, git};
use serde_json::{Value, json};

fn tick(world: &World, now: &str) -> Value {
    world
        .afk(&[
            "tick",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            now,
            "--worker-command",
            "agent",
        ])
        .ok()
}

fn did(ticked: &Value) -> Vec<String> {
    ticked["did"]
        .as_array()
        .unwrap()
        .iter()
        // the status boards a tick brings up to date are `board.rs`'s to test
        .filter(|step| step["do"] != "board")
        // a worker asked after, and left or nudged, is not what these tests are about
        .filter(|step| !(step["do"] == "judge" && step["outcome"] != "done"))
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

fn backlog(numbers: &[u64]) -> World {
    let world = World::new();
    for &n in numbers {
        world.issue(n, &["ready-for-agent"]);
    }
    world
}

fn alpha(numbers: &[u64]) -> Vec<(u64, String)> {
    numbers.iter().map(|&n| (n, "alpha".to_string())).collect()
}

#[test]
fn a_tick_dispatches_into_the_free_slots_lowest_number_first() {
    let world = backlog(&[9, 3, 5]);
    let ticked = tick(&world, "1000");
    assert_eq!(did(&ticked), ["dispatch 3 done", "dispatch 5 done", "heartbeat done"]);
    assert_eq!(ticked["held"], 2);
    assert_eq!(ticked["frontier_remaining"], 1);
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(world.claims(), alpha(&[3, 5]));

    // at the bound: the next tick takes nothing
    let again = tick(&world, "1090");
    assert_eq!(did(&again), ["heartbeat done"]);
    assert_eq!(world.claims(), alpha(&[3, 5]));
}

#[test]
fn a_tick_never_waits_for_the_workers_it_started() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert_eq!(
        calls.iter().filter(|call| *call == "terminal wait").count(),
        1,
        "only for readiness"
    );
}

#[test]
fn a_claim_whose_issue_is_closed_is_settled_and_its_slot_refilled() {
    let world = backlog(&[3, 5, 9]);
    tick(&world, "1000");
    let worktree = world.orca()["worktrees"][0]["path"].as_str().unwrap().to_string();
    world.close_issue(3);
    world.watch_pushes(None);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done", "dispatch 9 done", "heartbeat done"]);
    assert!(!std::path::Path::new(&worktree).exists(), "the worktree is removed");
    assert_eq!(world.claims(), alpha(&[5, 9]));
    // the worktree went first, the claim last
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(calls.contains(&"worktree rm".to_string()));
    assert_eq!(world.pushes()[0], "refs/convoy/claim/3");
}

#[test]
fn a_settling_that_failed_keeps_the_claim_and_the_pass_goes_on() {
    let world = backlog(&[3, 5]);
    world
        .afk(&[
            "dispatch",
            "--issue",
            "3",
            "--instance",
            "alpha",
            "--now",
            "1000",
            "--worker-command",
            "agent",
        ])
        .ok();
    world.close_issue(3);
    world.with_orca(|orca| orca["rm_fails"] = json!(true));

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 failed", "dispatch 5 done", "heartbeat done"]);
    assert_eq!(ticked["errors"][0]["do"], "settle");
    assert_eq!(ticked["errors"][0]["issue"], 3);
    assert!(ticked["errors"][0]["error"].as_str().unwrap().contains("worktree_busy"));
    // No topic explains what orca refused with: the step names none.
    assert_eq!(ticked["errors"][0].get("see"), None);
    assert_eq!(world.claims(), alpha(&[3, 5]), "the claim is held for the next tick");

    world.with_orca(|orca| orca["rm_fails"] = json!(false));
    assert_eq!(did(&tick(&world, "1180"))[0], "settle 3 done");
}

#[test]
fn a_start_that_failed_holds_its_claim_and_its_slot() {
    let world = backlog(&[3, 5, 9]);
    world.with_orca(|orca| orca["create_fails"] = json!([3]));
    let ticked = tick(&world, "1000");
    assert_eq!(did(&ticked), ["dispatch 3 failed", "dispatch 5 done", "heartbeat done"]);
    assert_eq!(ticked["held"], 2);
    assert_eq!(world.claims(), alpha(&[3, 5]));
}

#[test]
fn an_issue_a_peer_holds_is_passed_over() {
    let world = backlog(&[3, 5, 9]);
    world.afk(&["claim", "--issue", "3", "--instance", "beta", "--now", "1000"]).ok();
    let ticked = tick(&world, "1000");
    assert_eq!(did(&ticked), ["dispatch 5 done", "dispatch 9 done", "heartbeat done"]);
    assert_eq!(ticked["held"], 2);
}

#[test]
fn a_tick_holding_nothing_writes_nothing() {
    let world = World::new();
    world.watch_pushes(None);
    let ticked = tick(&world, "1000");
    assert_eq!(ticked["did"], json!([]));
    assert_eq!(world.pushes(), [] as [String; 0]);
}

#[test]
fn a_tick_leaves_nothing_behind_on_the_host() {
    let world = backlog(&[3]);
    let refs_before = git(&world.clone, &["for-each-ref"]);
    tick(&world, "1000");
    assert_eq!(git(&world.clone, &["status", "--porcelain"]), "");
    let refs_after = git(&world.clone, &["for-each-ref"]);
    let new: Vec<&str> = refs_after.lines().filter(|line| !refs_before.contains(line)).collect();
    assert_eq!(new.len(), 1, "only the worker's branch: {new:?}");
    assert!(new[0].contains("refs/heads/tester/issue-3"));
}

fn run(world: &World, extra: &[&str]) -> (Vec<String>, Value) {
    let mut args = vec!["run", "--host", "h1", "--worker-command", "agent", "--second-ms", "0"];
    args.extend(extra);
    world.afk_lines(&args)
}

#[test]
fn run_ticks_until_stopped_and_says_what_each_tick_did() {
    let world = backlog(&[3, 5, 9]);
    let (lines, stopped) = run(&world, &["--instance", "alpha", "--ticks", "2", "--now", "1000"]);
    assert_eq!(
        lines,
        [
            "tick 1: dispatched #3, #5 · holding 2/2 · frontier 1 · next in 90s",
            // their workers, started a moment ago, are left a grace period
            "tick 2: nothing to do · holding 2/2 · frontier 1 · next in 90s",
        ]
    );
    assert_eq!(stopped["ticks"], 2);
    // stopping is exiting: nothing is released on the way out
    assert_eq!(world.claims(), alpha(&[3, 5]));
}

#[test]
fn run_goes_idle_after_three_empty_ticks() {
    let world = World::new();
    let (lines, _) = run(&world, &["--ticks", "4"]);
    let sleeps: Vec<&str> =
        lines.iter().map(|line| line.rsplit("next in ").next().unwrap()).collect();
    assert_eq!(sleeps, ["90s", "90s", "1500s", "1500s"]);
    assert!(lines[0].starts_with("tick 1: nothing to do · holding 0/2 · frontier 0"));
}

#[test]
fn run_follows_a_tick_that_settled_a_claim_with_no_sleep_and_sleeps_after_one_that_did_nothing() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    world.close_issue(3);
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "2", "--now", "1090"]);
    assert_eq!(
        lines,
        [
            // what the landing unblocked is the next tick's to find
            "tick 1: settled #3 · holding 0/2 · frontier 0 · next now",
            "tick 2: nothing to do · holding 0/2 · frontier 0 · next in 90s",
        ]
    );
}

#[test]
fn run_mints_an_instance_id_named_for_the_host() {
    let world = backlog(&[3]);
    let (_, stopped) = run(&world, &["--ticks", "1"]);
    let instance = stopped["instance"].as_str().unwrap();
    assert!(instance.starts_with("h1-") && instance.len() == "h1-".len() + 8, "{instance}");
    assert_eq!(world.claims(), [(3, instance.to_string())]);
}

#[test]
fn a_tick_that_cannot_rebuild_is_a_line_and_the_loop_goes_on() {
    let world = backlog(&[3]);
    world.with_gh(|gh| gh["fail"] = json!(["pr list"]));
    let (lines, stopped) = run(&world, &["--ticks", "2"]);
    assert!(lines[0].starts_with("tick 1: FAILED: gh pr list failed"), "{lines:?}");
    assert!(lines[1].starts_with("tick 2: FAILED"));
    assert_eq!(stopped["ticks"], 2);
}

#[test]
fn a_failed_step_shows_in_the_line() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["create_fails"] = json!([3]));
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1"]);
    assert!(lines[0].contains("FAILED dispatch #3: orca worktree create failed"), "{lines:?}");
    // every step it had failed: that is not having had nothing to do
    assert!(!lines[0].contains("nothing to do"), "{lines:?}");
    assert!(lines[0].starts_with("tick 1: holding 1/2 · frontier 0 · FAILED"), "{lines:?}");
}

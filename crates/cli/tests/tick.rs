use crate::common;

use common::{World, git};
use serde_json::{Value, json};

fn tick(world: &World, now: &str) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]).ok()
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

/// A world whose fleet holds two claims at once, with `numbers` ready.
fn backlog(numbers: &[u64]) -> World {
    let world = World::new();
    world.say_concurrency(&world.clone, 2);
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
    let tmp = afkconvoy::worker_tmp::of(std::path::Path::new(&worktree));
    assert!(tmp.is_dir(), "its worker's TMPDIR");
    world.close_issue(3);
    world.watch_pushes(None);

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["settle 3 done", "dispatch 9 done", "heartbeat done"]);
    assert!(!std::path::Path::new(&worktree).exists(), "the worktree is removed");
    assert!(tmp.symlink_metadata().is_err(), "its worker's TMPDIR with it");
    assert_eq!(world.claims(), alpha(&[5, 9]));
    // the worktree went first, the claim last
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(calls.contains(&"worktree rm".to_string()));
    assert_eq!(world.pushes()[0], "refs/convoy/claim/3");
}

#[test]
fn a_settling_that_failed_keeps_the_claim_and_the_pass_goes_on() {
    let world = backlog(&[3, 5]);
    world.afk(&["dispatch", "--issue", "3", "--instance", "alpha", "--now", "1000"]).ok();
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
fn a_worker_that_never_took_its_brief_is_started_again_in_the_same_worktree() {
    for knob in ["never_ready", "trust_sticks", "send_refused"] {
        let world = backlog(&[3]);
        world.with_orca(|orca| {
            orca[knob] = json!(true);
            orca["trust_prompt"] = json!(knob == "trust_sticks");
        });
        let failed = tick(&world, "1000");
        assert_eq!(did(&failed), ["dispatch 3 failed", "heartbeat done"]);
        let before = world.orca();
        let path = before["worktrees"][0]["path"].clone();
        let branch = before["worktrees"][0]["branch"].clone();
        let path_dir = std::path::Path::new(path.as_str().unwrap());
        std::fs::write(path_dir.join("scratch.txt"), "keep uncommitted progress").unwrap();
        world.with_orca(|orca| {
            orca[knob] = json!(false);
            orca["trust_prompt"] = json!(false);
        });
        world.orca_calls();

        let again = tick(&world, "1090");
        assert!(again["errors"].as_array().unwrap().is_empty(), "{again}");
        let calls = world.orca_calls();
        assert!(
            calls.iter().any(|call| common::asked(call) == "terminal create"),
            "{knob}: the next tick must start the worker again: {again}"
        );
        assert!(!calls.iter().any(|call| common::asked(call) == "worktree create"));
        let after = world.orca();
        assert_eq!(after["worktrees"][0]["path"], path);
        assert_eq!(after["worktrees"][0]["branch"], branch);
        assert_eq!(
            std::fs::read_to_string(path_dir.join("scratch.txt")).unwrap(),
            "keep uncommitted progress"
        );
        let terminals = after["terminals"].as_array().unwrap();
        let worker = terminals.iter().find(|terminal| terminal["open"] == true).unwrap();
        let sent = worker["sent"].as_array().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0]["text"].as_str().unwrap().contains("convoy-worker-prompt.md"));
        assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
        assert_eq!(world.claims(), alpha(&[3]));
        // A successful delivery ended startup: the next tick leaves this worker alone.
        world.orca_calls();
        tick(&world, "1180");
        assert!(!world.orca_calls().iter().any(|call| common::asked(call) == "terminal create"));
    }
}

#[test]
fn a_worker_start_that_fails_again_escalates_with_the_start_errors() {
    for knob in ["create_fails", "never_ready", "send_refused"] {
        let world = backlog(&[3]);
        world.set_gate_config("gate_command: ./gate.sh\nescalate_to: human");
        world.with_orca(|orca| {
            orca[knob] = if knob == "create_fails" { json!([3]) } else { json!(true) };
        });
        let first = tick(&world, "1000");
        let error = first["errors"][0]["error"].as_str().unwrap();
        assert_eq!(world.claims(), alpha(&[3]));
        let again = tick(&world, "1090");
        assert_eq!(world.claims(), [], "{knob}: {again}");
        assert!(world.labels(3).contains(&"ready-for-human".to_string()));
        let comments = world.comments(3);
        let escalation = comments.iter().find(|body| body.contains("convoy:escalation")).unwrap();
        assert!(escalation.contains("worker start failed again"), "{escalation}");
        assert!(escalation.contains(error), "the exact original error: {escalation}");
        assert!(escalation.contains("Failed start:"), "{escalation}");
        assert!(!escalation.contains("went silent"), "{escalation}");
        assert!(world.open_terminals().is_empty());
        for terminal in world.orca()["terminals"].as_array().unwrap() {
            assert_eq!(terminal["sent"], json!([]), "no nudge or brief reached this shell");
        }
        tick(&world, "1600");
        assert_eq!(world.claims(), []);
    }
}

#[test]
fn an_incomplete_dispatch_does_not_belong_to_a_later_claim() {
    let world = backlog(&[3]);
    world.with_orca(|orca| orca["never_ready"] = json!(true));
    tick(&world, "1000");
    world.afk(&["release", "--issue", "3", "--instance", "alpha"]).ok();
    let new = tick(&world, "1090");
    assert_eq!(did(&new), ["dispatch 3 failed", "heartbeat done"]);
    assert_eq!(world.claims(), alpha(&[3]));
    assert!(!world.comments(3).iter().any(|body| body.contains("convoy:escalation")));
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
    let mut args = vec!["run", "--foreground", "--host", "h1", "--second-ms", "0"];
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
fn a_tick_says_which_issue_it_skipped_for_a_blocker_read_after_its_rebuild() {
    let world = backlog(&[3, 5]);
    // the rebuild reads no blocker of #3: GitHub records it as it answers the dispatch
    world.with_gh(|gh| {
        gh["edges"]["3"] = json!([5]);
        gh["issues"][0]["blockers_at_rebuild"] = json!([]);
    });
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    assert_eq!(
        lines,
        [
            "tick 1: dispatched #5 · skipped #3 (blocked by #5) · holding 1/2 · frontier 0 · next in 90s"
        ]
    );
    assert_eq!(world.claims(), alpha(&[5]));
}

#[test]
fn run_goes_idle_after_three_empty_ticks() {
    let world = World::new();
    world.say_concurrency(&world.clone, 2);
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
fn every_tick_says_as_it_starts_why_it_started() {
    let world = backlog(&[3]);
    tick(&world, "1000");
    world.close_issue(3);
    let args = ["run", "--foreground", "--host", "h1", "--second-ms", "0"];
    let more = ["--instance", "alpha", "--ticks", "3", "--now", "1090", "--no-host-sleep"];
    let (lines, _) = world.afk_every_line(&[&args[..], &more[..]].concat());
    assert_eq!(
        lines,
        [
            "start: run as alpha on h1, its workers started with `agent`",
            "tick 1: started (start)",
            "tick 1: settled #3 · holding 0/2 · frontier 0 · next now",
            "tick 2: started (follow-up)",
            "tick 2: nothing to do · holding 0/2 · frontier 0 · next in 90s",
            "tick 3: started (timer)",
            "tick 3: nothing to do · holding 0/2 · frontier 0 · next in 90s",
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

/// Takes issue `number` off the stand-in GitHub as `how` — `deleted` or
/// `transferred` — once `calls` more calls about it are answered.
fn gone(world: &World, number: u64, how: &str, calls: u64) {
    world.with_gh(|gh| {
        if calls == 0 {
            gh["issues"].as_array_mut().unwrap().retain(|issue| issue["number"] != number);
        }
        gh[how] = json!({ number.to_string(): calls });
    });
}

#[test]
fn a_claimed_issue_deleted_between_two_ticks_is_let_go_and_its_slot_refilled() {
    for (how, said) in
        [("deleted", "the issue was deleted"), ("transferred", "the issue is gone from this repo")]
    {
        let world = backlog(&[3, 5, 9]);
        tick(&world, "1000");
        let worktree = world.orca()["worktrees"][0]["path"].as_str().unwrap().to_string();
        gone(&world, 3, how, 0);

        let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1090"]);
        assert_eq!(
            lines,
            [format!(
                "tick 1: released #3 ({said}) · dispatched #9 · holding 2/2 · frontier 0 · \
                 next in 90s"
            )],
            "{how}"
        );
        assert_eq!(world.claims(), alpha(&[5, 9]), "{how}");
        // it held nothing the remote does not
        assert!(!std::path::Path::new(&worktree).exists(), "{how}: the worktree is removed");
        // nothing is written on an issue that is gone
        assert_eq!(world.boards(3).len(), 1, "{how}: only the board of its dispatch");
    }
}

#[test]
fn a_tick_lets_go_of_a_deleted_issue_in_its_answer_with_no_error() {
    let world = backlog(&[3, 5]);
    tick(&world, "1000");
    gone(&world, 3, "deleted", 0);
    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["forget 3 done", "heartbeat done"]);
    assert_eq!(ticked["errors"], json!([]));
}

#[test]
fn an_issue_deleted_during_its_own_dispatch_leaves_no_claim_behind() {
    let world = backlog(&[3, 5]);
    // answered as the dispatch reads it and its blockers; deleted once it is claimed
    gone(&world, 3, "deleted", 2);
    let (lines, _) = run(&world, &["--instance", "alpha", "--ticks", "1", "--now", "1000"]);
    assert_eq!(
        lines,
        ["tick 1: released #3 (the issue was deleted) · dispatched #5 · holding 1/2 · frontier \
             0 · next in 90s"]
    );
    assert_eq!(world.claims(), alpha(&[5]));
}

#[test]
fn an_issue_deleted_before_its_dispatch_claims_nothing_and_takes_no_slot() {
    let world = backlog(&[3, 5, 9]);
    gone(&world, 3, "deleted", 0);
    // the rebuild's list still had it
    world.with_gh(|gh| {
        gh["issues"].as_array_mut().unwrap().push(json!({"number": 3,
        "title": "Issue 3", "labels": ["ready-for-agent"], "state": "open"}))
    });
    let ticked = tick(&world, "1000");
    assert_eq!(
        did(&ticked),
        ["dispatch 3 gone", "dispatch 5 done", "dispatch 9 done", "heartbeat done"]
    );
    assert_eq!(world.claims(), alpha(&[5, 9]));
}

#[test]
fn a_claimed_issue_the_rebuild_cannot_read_is_an_error_of_that_tick_alone() {
    let world = backlog(&[3, 5, 9]);
    tick(&world, "1000");
    world.with_gh(|gh| {
        gh["issues"].as_array_mut().unwrap().retain(|issue| issue["number"] != 3);
        gh["fail"] = json!(["issues/3 --jq"]);
    });
    world.close_issue(5);
    let ticked = tick(&world, "1090");
    assert_eq!(ticked["errors"][0]["do"], "read");
    assert_eq!(ticked["errors"][0]["issue"], 3);
    assert!(ticked["errors"][0]["error"].as_str().unwrap().contains("injected failure"));
    // the rest of the tick is still made
    assert_eq!(did(&ticked)[..2], ["settle 5 done", "dispatch 9 done"]);
    assert_eq!(world.claims(), alpha(&[3, 9]));
}

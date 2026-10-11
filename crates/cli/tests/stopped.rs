mod common;

use std::path::PathBuf;

use common::{World, commit};
use serde_json::{Value, json};

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args =
        ["tick", "--instance", "alpha", "--host", "h1", "--now", &now, "--worker-command", "agent"];
    world.afk(&args).ok()
}

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

/// A world with a worker dispatched on issue 7 at `now`.
fn fleet(now: u64) -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    assert_eq!(did(&tick(&world, now)), ["dispatch 7 done", "heartbeat done"]);
    world
}

fn worktree(world: &World) -> PathBuf {
    PathBuf::from(world.orca()["worktrees"][0]["path"].as_str().unwrap())
}

fn declare(world: &World, phase: &str) -> String {
    world.comment(7, &format!("<!--convoy:verdict phase={phase}-->\nwhat the worker says"))
}

/// How many escalation comments issue 7 carries.
fn escalations(world: &World) -> usize {
    world.comments(7).iter().filter(|body| body.contains("convoy:escalation")).count()
}

fn act(world: &World, command: &str, instance: &str, more: &[&str]) -> common::Answer {
    let mut args = vec![command, "--issue", "7", "--instance", instance, "--now", "1100"];
    args.extend(more);
    world.afk(&args)
}

/// The worker's terminal reports `state` since `since`, its last output at `output` (epoch seconds).
fn report(world: &World, state: &str, since: u64, output: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["command"] == "agent" {
                terminal["agent"] = json!({"state": state, "stateStartedAt": since * 1000});
                terminal["lastOutputAt"] = json!(output * 1000);
            }
        }
    });
}

#[test]
fn a_busy_worker_is_left_with_no_github_read() {
    let world = fleet(1000);
    declare(&world, "needs-human");
    report(&world, "working", 1000, 1080);
    world.gh_calls();
    assert_eq!(did(&tick(&world, 1090)), ["judge 7 nothing", "heartbeat done"]);
    let calls: Vec<String> = world.gh_calls().iter().map(|call| call.join(" ")).collect();
    assert!(!calls.iter().any(|call| call.contains("issues/7/comments")), "{calls:?}");

    // `working`, but no output within the grace period: a stop report that was lost
    report(&world, "working", 1000, 1080);
    assert_eq!(did(&tick(&world, 1080 + 300))[0], "judge 7 done");
    assert_eq!(world.claims(), []);
}

#[test]
fn an_orca_that_cannot_be_asked_is_an_error_never_a_worker_that_is_gone() {
    let world = fleet(1000);
    declare(&world, "needs-human");
    world.with_orca(|orca| orca["ps_truncated"] = json!(true));
    let ticked = tick(&world, 1090);
    assert_eq!(did(&ticked)[0], "judge 7 failed");
    assert!(ticked["errors"][0]["error"].as_str().unwrap().contains("cut short"));
    assert_eq!(world.claims().len(), 1);

    world.with_orca(|orca| {
        orca["ps_truncated"] = json!(false);
        orca["down"] = json!(true);
    });
    let ticked = tick(&world, 1180);
    assert_eq!(did(&ticked)[0], "judge 7 failed");
    assert_eq!(world.claims().len(), 1);
    assert_eq!(world.state(7), "open");
}

#[test]
fn a_recent_sign_of_life_is_left_whatever_was_declared() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let world = fleet(now);
    declare(&world, "needs-human");
    // the worktree was touched moments ago
    assert_eq!(did(&tick(&world, now + 90))[0], "judge 7 nothing");
    assert_eq!(did(&tick(&world, now + 400))[0], "judge 7 done");
}

#[test]
fn already_satisfied_with_nothing_on_the_branch_closes_the_issue() {
    let world = fleet(1000);
    world.issue(9, &["ready-for-agent"]);
    world.issue(10, &["ready-for-agent"]);
    assert_eq!(did(&tick(&world, 1090))[1], "dispatch 9 done");
    declare(&world, "needs-human");
    declare(&world, "already-satisfied");
    let dir = worktree(&world);
    world.watch_pushes(None);

    let ticked = tick(&world, 1180);
    assert_eq!(
        did(&ticked),
        ["judge 7 done", "judge 9 nothing", "dispatch 10 done", "heartbeat done"],
        "the latest verdict wins, and the slot it frees is filled"
    );
    assert_eq!(world.state(7), "closed");
    assert!(!dir.exists(), "the worktree is removed");
    assert_eq!(world.claims().iter().map(|c| c.0).collect::<Vec<_>>(), [9, 10]);
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"], "nothing is relabelled");
    assert_eq!(world.comments(7).len(), 3, "…and nothing is commented");
}

#[test]
fn already_satisfied_over_work_on_the_branch_escalates_and_keeps_the_worktree() {
    for dirty in [false, true] {
        let world = fleet(1000);
        let dir = worktree(&world);
        if dirty {
            std::fs::write(dir.join("scratch.txt"), "x").unwrap();
        } else {
            commit(&dir, "work.txt", "x\n");
        }
        let said = declare(&world, "already-satisfied");

        assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
        assert_eq!(world.state(7), "open");
        assert_eq!(world.claims(), []);
        assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
        assert!(dir.exists(), "the worktree is evidence");
        // the branch record, the verdict, the escalation
        let comments = world.comments(7);
        assert_eq!(comments.len(), 3);
        assert!(comments[2].starts_with("<!--convoy:escalation claim="), "{}", comments[2]);
        assert!(comments[2].contains("escalated to a human"));
        assert!(comments[2].contains("its branch holds work") && comments[2].contains(&said));
    }
}

#[test]
fn needs_human_and_an_unknown_phase_escalate() {
    for (phase, says) in [("needs-human", "declared `needs-human`"), ("giving-up", "cannot read")] {
        let world = fleet(1000);
        let dir = worktree(&world);
        let said = declare(&world, phase);
        assert_eq!(did(&tick(&world, 1090)), ["judge 7 done"]);
        assert_eq!(world.claims(), []);
        assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
        let comments = world.comments(7);
        assert!(comments[2].contains(says) && comments[2].contains(&said), "{}", comments[2]);
        assert!(!dir.exists(), "a worktree whose branch holds no work is removed");
        // off the frontier: nothing dispatches it again
        assert_eq!(did(&tick(&world, 1180)), [] as [String; 0]);
    }
}

#[test]
fn a_worker_that_declared_nothing_is_left() {
    let world = fleet(1000);
    commit(&worktree(&world), "work.txt", "x\n");
    world.comment(7, "a comment that is no verdict");
    assert_eq!(did(&tick(&world, 1090)), ["judge 7 nothing", "heartbeat done"]);
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn escalate_picks_up_from_wherever_it_was_cut_and_comments_once() {
    let world = fleet(1000);
    let reason = ["--reason", "the schema needs a decision"];
    // cut after the comment: the relabel fails
    world.with_gh(|gh| gh["fail"] = json!(["/labels"]));
    let error = act(&world, "escalate", "alpha", &reason).error();
    assert!(error.contains("injected failure"), "{error}");
    assert_eq!(escalations(&world), 1);
    assert_eq!(world.claims().len(), 1, "the claim is still held");
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);

    // cut after the relabel: the release is refused
    world.with_gh(|gh| gh["fail"] = json!([]));
    world.watch_pushes(Some("claim/7"));
    let error = act(&world, "escalate", "alpha", &reason).error();
    assert!(error.contains("release failed"), "{error}");
    assert_eq!(escalations(&world), 1, "the comment of this claim is there already");
    assert_eq!(world.labels(7), ["bug", "ready-for-human"]);
    assert_eq!(world.claims().len(), 1);

    world.watch_pushes(None);
    let answer = act(&world, "escalate", "alpha", &reason).ok();
    assert_eq!(answer["action"], "escalated");
    assert_eq!(answer["released"], true);
    assert_eq!(answer["worktree"], "removed");
    assert_eq!(world.claims(), []);
    assert_eq!(escalations(&world), 1);
    assert!(world.comments(7)[1].contains("the schema needs a decision"));

    // run once more: there is no claim left to end
    world.gh_calls();
    let again = act(&world, "escalate", "alpha", &reason).ok();
    assert!(again["detail"].as_str().unwrap().contains("no claim is held"));
    assert_eq!(escalations(&world), 1);
}

#[test]
fn a_transition_refuses_a_claim_that_is_not_this_instances() {
    let world = fleet(1000);
    declare(&world, "already-satisfied");
    for (command, more) in [("escalate", &["--reason", "x"][..]), ("close", &[][..])] {
        let error = act(&world, command, "beta", more).error();
        assert!(error.contains("alpha"), "{error}");
    }
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    assert_eq!(world.state(7), "open");
    assert_eq!(escalations(&world), 0);
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);
}

#[test]
fn close_is_refused_over_work_and_closes_a_branch_with_none() {
    let world = fleet(1000);
    let dir = worktree(&world);
    std::fs::write(dir.join("scratch.txt"), "x").unwrap();
    let error = act(&world, "close", "alpha", &[]).error();
    assert!(error.contains("its branch holds work"), "{error}");
    assert_eq!(world.state(7), "open");
    assert_eq!(world.claims().len(), 1);

    std::fs::remove_file(dir.join("scratch.txt")).unwrap();
    world.watch_pushes(None);
    let answer = act(&world, "close", "alpha", &[]).ok();
    assert_eq!(answer["action"], "closed");
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert!(!dir.exists());
    assert_eq!(world.pushes(), ["refs/convoy/claim/7"]);
}

//! A turn's observable contract, through the real CLI and git remote.
use crate::common::{World, git};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

const LINE: &str = "refs/convoy/train/line/main";

fn quote(path: &Path) -> String {
    afkconvoy_core::line::Line::new().word(&path.to_string_lossy()).own()
}

fn until(done: impl Fn() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(
            began.elapsed() < Duration::from_secs(15),
            "the expected transition did not happen"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn start(world: &World, dir: &Path, cache: &Path, issue: u64) -> Child {
    world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), dir)
        .env("XDG_CACHE_HOME", cache)
        .env("AFKCONVOY_ISSUE", issue.to_string())
        .args(["gate", "--train"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(child: Child) -> Value {
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn seed(world: &World) {
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["recorded"], true, "{answer}");
}

fn workers_waiting(world: &World) -> usize {
    world.afk(&["doctor"]).json["host"]["exclusive"]["lanes"]["worker"]["waiting"]
        .as_array()
        .map_or(0, Vec::len)
}

#[test]
fn two_turns_merge_gate_and_join_in_order_and_the_train_uses_the_last_record() {
    let world = World::new();
    let started = world.root.join("first-started");
    let release = world.root.join("release");
    let second_started = world.root.join("second-started");
    world.set_gate(&format!(
        "if test -f one.txt && test ! -f two.txt; then touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done; fi; if test -f two.txt; then test -f one.txt || exit 9; touch {}; fi",
        quote(&started), quote(&world.root), quote(&release), quote(&second_started)));
    seed(&world);
    let one = world.worker(1, "one.txt", "one");
    let two = world.worker(2, "two.txt", "two");
    let _runner = world.host_held();
    let a = start(&world, &one, &world.cache(), 1);
    until(|| started.exists());
    let b = start(&world, &two, &world.cache(), 2);
    until(|| {
        world.afk(&["doctor"]).json["host"]["exclusive"]["lanes"]["repos"].as_array().is_some_and(
            |r| r.iter().any(|r| r["queue"]["waiting"].as_array().is_some_and(|w| !w.is_empty())),
        )
    });
    assert!(!second_started.exists(), "a worker without its turn runs no gate");
    std::fs::write(&release, "").unwrap();
    let (a, b) = (finish(a), finish(b));
    assert_eq!(a["outcome"], "joined", "{a}");
    assert_eq!(b["outcome"], "joined", "{b}");
    assert_eq!(b["merged"], a["commit"], "the tip is read when its turn starts");
    assert!(two.join("one.txt").exists());
    let train = world.root.join("convoy-train");
    git(
        &world.clone,
        &["worktree", "add", "-q", "--detach", train.to_str().unwrap(), "origin/main"],
    );
    let landed = world.afk_in(&train, &["land", "--train", "--merged-timeout", "0"]).ok();
    assert_eq!(landed["gate"]["source"], "recorded", "{landed}");
    assert_eq!(landed["commit"], b["commit"]);
}

#[test]
fn red_and_conflict_release_the_turn_without_joining() {
    let world = World::new();
    world.set_gate("test ! -f bad.txt");
    seed(&world);
    let red = world.worker(1, "bad.txt", "bad");
    let answer = world.afk_in(&red, &["gate", "--train"]).ok();
    assert_eq!(answer["status"], "red");
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
    let one = world.worker(2, "README.md", "two");
    let two = world.worker(3, "README.md", "three");
    assert_eq!(world.afk_in(&one, &["gate", "--train"]).ok()["outcome"], "joined");
    let before = world.remote(&["rev-parse", LINE]);
    let answer = world.afk_in(&two, &["gate", "--train"]).ok();
    assert_eq!(answer["outcome"], "conflict", "{answer}");
    assert_eq!(git(&two, &["diff", "--name-only", "--diff-filter=U"]), "README.md");
    assert_eq!(world.remote(&["rev-parse", LINE]), before);
}

#[test]
fn a_missing_tip_record_is_gated_in_the_train_lane_before_any_worker_turn() {
    let world = World::new();
    let certifying = world.root.join("certifying");
    let release = world.root.join("release");
    let worker_gate = world.root.join("worker-gate");
    world.set_gate(&format!(
        "if test -f worker.txt; then touch {}; else touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done; fi",
        quote(&worker_gate), quote(&certifying), quote(&world.root), quote(&release)));
    let worker = world.worker(1, "worker.txt", "one");
    let _runner = world.host_held();
    let child = start(&world, &worker, &world.cache(), 1);
    until(|| certifying.exists());
    let doctor = world.afk(&["doctor"]).json;
    let lanes = &doctor["host"]["exclusive"]["lanes"];
    assert!(!lanes["train"]["running"].is_null(), "{lanes}");
    assert_eq!(lanes["worker"]["running"], Value::Null);
    assert!(!worker_gate.exists());
    std::fs::write(release, "").unwrap();
    assert_eq!(finish(child)["outcome"], "joined");
    assert!(worker_gate.exists());
}

fn heavy(world: &World) {
    let gate = afkconvoy_core::line::Line::new()
        .program(env!("CARGO_BIN_EXE_afkconvoy"))
        .fixed(" exclusive -- sh ./gate.sh")
        .own();
    world.set_gate(&gate);
    let script = format!(
        "#!/bin/sh\nset -e\nif test -f held.txt; then touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done; fi\n",
        quote(&world.root.join("started")),
        quote(&world.root),
        quote(&world.root.join("release"))
    );
    crate::common::commit(&world.clone, "gate.sh", &script);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    seed(world);
}

#[test]
fn two_worker_lanes_run_two_repos_but_a_second_turn_of_one_repo_waits() {
    let (a, b) = (World::new(), World::new());
    heavy(&a);
    heavy(&b);
    a.say_host_settings(json!({"gate_lanes": 2, "train_lanes": 2}));
    let one = a.worker(1, "held.txt", "one");
    let another = a.worker(2, "second.txt", "second");
    let other_repo = b.worker(3, "held.txt", "other repo");
    let _runner = a.host_held();
    let first = start(&a, &one, &a.cache(), 1);
    until(|| a.root.join("started").exists());
    let second = start(&a, &another, &a.cache(), 2);
    until(|| workers_waiting(&a) == 1);
    let third = start(&b, &other_repo, &a.cache(), 3);
    until(|| b.root.join("started").exists());
    assert!(!another.join("held.txt").exists(), "the second turn has not merged yet");
    let doctor = a.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["settings"]["in_force"]["gate_lanes"], 2);
    assert_eq!(doctor["host"]["settings"]["in_force"]["train_lanes"], 2);
    std::fs::write(a.root.join("release"), "").unwrap();
    std::fs::write(b.root.join("release"), "").unwrap();
    assert_eq!(finish(first)["outcome"], "joined");
    assert_eq!(finish(third)["outcome"], "joined");
    assert_eq!(finish(second)["outcome"], "joined");
    assert!(another.join("held.txt").exists());
}

#[test]
fn a_light_gate_takes_its_repo_turn_while_the_worker_lane_is_occupied() {
    let world = World::new();
    let other = World::new();
    world.set_gate("true");
    seed(&world);
    let worker = world.worker(1, "one.txt", "one");
    let _runner = world.host_held();
    let held = world.root.join("held");
    let release = world.root.join("release");
    let script = format!(
        "touch {}; while test -d {} && test ! -f {}; do sleep 0.02; done",
        quote(&held),
        quote(&world.root),
        quote(&release)
    );
    let mut command = other.command(env!("CARGO_BIN_EXE_afkconvoy"), &other.clone);
    command.env("XDG_CACHE_HOME", world.cache());
    let heavy = command
        .args(["exclusive", "--", "sh", "-c", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    until(|| held.exists());
    assert_eq!(finish(start(&world, &worker, &world.cache(), 1))["outcome"], "joined");
    std::fs::write(release, "").unwrap();
    assert!(heavy.wait_with_output().unwrap().status.success());
}

#[test]
fn the_exclusive_bound_ends_the_whole_turn_and_releases_the_next_one() {
    let world = World::new();
    heavy(&world);
    world.say_host_settings(json!({"exclusive_seconds": 1}));
    let one = world.worker(1, "held.txt", "one");
    let two = world.worker(2, "second.txt", "two");
    let _runner = world.host_held();
    let first = start(&world, &one, &world.cache(), 1);
    until(|| world.root.join("started").exists());
    let next = start(&world, &two, &world.cache(), 2);
    let ended = first.wait_with_output().unwrap();
    assert_eq!(ended.status.code(), Some(124), "{}", String::from_utf8_lossy(&ended.stderr));
    let answer: Value = serde_json::from_slice(&ended.stdout).unwrap();
    assert!(answer["error"].as_str().unwrap().contains("exclusive bound of 1s"), "{answer}");
    assert_eq!(finish(next)["outcome"], "joined");
    assert!(!two.join("held.txt").exists(), "the red turn joined nothing");
}

#[test]
fn the_tip_uses_its_own_gate_command_and_each_turn_uses_the_merged_config() {
    let world = World::new();
    world.set_gate("true");
    let first = world.worker(1, "required.txt", "one");
    let second = world.worker(2, "second.txt", "two");
    crate::common::commit(
        &first,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: test -f required.txt\ngate_seconds: 600\n```\n",
    );
    let changed = world.afk_in(&first, &["gate", "--train"]).ok();
    assert_eq!(changed["outcome"], "joined", "{changed}");
    assert_eq!(changed["command"], "test -f required.txt");
    let merged = world.afk_in(&second, &["gate", "--train"]).ok();
    assert_eq!(merged["outcome"], "joined", "{merged}");
    assert_eq!(merged["command"], "test -f required.txt", "the merged config names the gate");
    assert_eq!(merged["recorded"], true);
    let train = world.root.join("convoy-train");
    git(
        &world.clone,
        &["worktree", "add", "-q", "--detach", train.to_str().unwrap(), "origin/main"],
    );
    let landed = world.afk_in(&train, &["land", "--train", "--merged-timeout", "0"]).ok();
    assert_eq!(landed["gate"]["source"], "recorded", "{landed}");
}

#[test]
fn a_green_run_without_a_written_record_joins_nothing_and_keeps_the_pr_in_its_answer() {
    let world = World::new();
    world.set_gate("true");
    seed(&world);
    let worker = world.worker(1, "one.txt", "one");
    world.watch_pushes(Some("convoy/gate"));
    let answer = world.afk_in(&worker, &["gate", "--train"]).ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["recorded"], false);
    assert_eq!(answer["outcome"], "gate_red");
    assert_eq!(answer["issue"], 1);
    assert_eq!(answer["pr"], 10);
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
    world.watch_pushes(None);
    assert_eq!(world.afk_in(&worker, &["gate", "--train"]).ok()["outcome"], "joined");
}

#[test]
fn a_green_tip_whose_record_cannot_be_written_returns_a_retryable_error() {
    let world = World::new();
    world.set_gate("true");
    let worker = world.worker(1, "one.txt", "one");
    world.watch_pushes(Some("convoy/gate"));
    let error = world.afk_in(&worker, &["gate", "--train"]).error();
    assert!(error.contains("gated green, but has no record"), "{error}");
    world.watch_pushes(None);
    assert_eq!(world.afk_in(&worker, &["gate", "--train"]).ok()["outcome"], "joined");
}

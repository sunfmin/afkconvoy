//! The train's worktree cut again: a tick that finds the train idle and its
//! worktree outgrown — by what the last landing measured — removes it and cuts
//! it anew. Where the line lies (`outgrown`) is the core's own tests'.

use crate::common;

use std::fs::File;
use std::path::{Path, PathBuf};

use afkconvoy_core::train::OUTGROWN_FLOOR;
use common::{BASE, World, commit, git};
use serde_json::{Value, json};

fn tick(world: &World, now: &str) -> Value {
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        .filter(|step| step["do"] != "board" && step["do"] != "heartbeat")
        .filter(|step| !(step["do"] == "judge" || step["do"] == "watch"))
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// What the gate of `building` builds into `out/`, which git ignores, each
/// time it runs: ten bytes more.
const TEN_BYTES: &str = "mkdir -p out && printf 0123456789 >> out/build";

/// A world whose gate is `gate`, with workers dispatched on `issues` at 1000.
fn building(issues: &[u64], gate: &str) -> World {
    let world = World::new();
    world.set_gate(gate);
    commit(&world.clone, ".gitignore", "out/\n");
    git(&world.clone, &["push", "-q", "origin", BASE]);
    for &n in issues {
        world.issue(n, &["ready-for-agent"]);
    }
    assert_eq!(tick(&world, "1000")["errors"], json!([]));
    world
}

fn worktree_of(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().find(|row| row["linkedIssue"] == issue).expect("a worktree of the issue");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The train's worktree orca lists: the one linked to no issue.
fn train_worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let trains: Vec<&Value> = rows.iter().filter(|row| row["linkedIssue"].is_null()).collect();
    assert_eq!(trains.len(), 1, "{trains:?}");
    PathBuf::from(trains[0]["path"].as_str().unwrap())
}

/// The worker on `issue` finishes: commits, opens PR #`issue`0 and joins the
/// train.
fn finish(world: &World, issue: u64) {
    let dir = worktree_of(world, issue);
    commit(&dir, &format!("f{issue}.txt"), "x\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(issue * 10, issue, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    let answer = world.afk_in(&dir, &["land", "--issue", &issue.to_string(), "--now", "1050"]).ok();
    assert_eq!(answer["outcome"], "joined", "{answer}");
}

/// The train worker lands the train → the answer.
fn land(world: &World, now: &str) -> Value {
    let train = train_worktree(world);
    let args = ["--now", now, "land", "--train", "--merged-timeout", "5"];
    let landed = world.afk_in(&train, &args).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    landed
}

/// Puts a file of `bytes` in `out/` of `dir`, which git ignores: sparse, it
/// takes no disk.
fn build_into(dir: &Path, name: &str, bytes: u64) {
    std::fs::create_dir_all(dir.join("out")).unwrap();
    File::create(dir.join("out").join(name)).unwrap().set_len(bytes).unwrap();
}

/// The train's worker is at work in its terminal.
fn at_work(world: &World, dir: &Path) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == dir.to_str().unwrap() && terminal["open"] == true {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": 1_200_000u64});
                terminal["lastOutputAt"] = json!(1_265_000u64);
            }
        }
    });
}

/// A train whose worktree outgrew its fresh build of ten bytes on its second
/// landing, #8's: the train is idle again, and #7 and #8 landed.
fn outgrown() -> World {
    let world = building(&[7, 8], TEN_BYTES);
    finish(&world, 7);
    tick(&world, "1090");
    assert_eq!(land(&world, "1170")["fresh"], 10);
    finish(&world, 8);
    let train = train_worktree(&world);
    build_into(&train, "old", 3 * OUTGROWN_FLOOR);
    let landed = land(&world, "1180");
    assert_eq!(landed["built"], 3 * OUTGROWN_FLOOR + 20, "{landed}");
    assert_eq!(landed["fresh"], 10);
    world
}

#[test]
fn an_idle_train_whose_worktree_outgrew_its_build_has_it_cut_again_and_the_log_says_so() {
    let world = outgrown();
    let train = train_worktree(&world);
    let (lines, ticked) = world.afk_lines(&[
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--ticks",
        "1",
        "--now",
        "1270",
        "--second-ms",
        "0",
    ]);
    let line = lines.iter().find(|line| line.contains("cut the train's worktree again"));
    let line = line.unwrap_or_else(|| panic!("{lines:#?} {ticked}"));
    assert!(line.contains("(built 3.0 GiB, its fresh build 10 B)"), "{line}");

    let cut = train_worktree(&world);
    assert!(!train.exists() || cut == train, "the old one is gone");
    assert!(!cut.join("out").exists(), "nothing it built is left");
    assert_eq!(git(&cut, &["rev-parse", "HEAD"]), world.remote(&["rev-parse", "refs/heads/main"]));
    assert_eq!(git(&cut, &["status", "--porcelain"]), "");
    // cut once: the next tick finds nothing measured in it
    assert_eq!(did(&tick(&world, "1360")), [] as [&str; 0]);
}

#[test]
fn a_train_with_a_pr_on_it_or_a_worker_at_work_is_never_cut_again() {
    let world = outgrown();
    let train = train_worktree(&world);
    at_work(&world, &train);
    let ticked = tick(&world, "1270");
    assert_eq!(did(&ticked), ["settle 7 done", "settle 8 done", "cut_train nothing"]);
    assert!(train.join("out/old").exists(), "its worker is at work: left as it is");

    // a PR on the train: no cut is even asked about, however big the worktree
    let world = outgrown();
    let train = train_worktree(&world);
    let nine = world.worker(9, "nine.txt", "9\n");
    let joined = world.afk_in(&nine, &["land", "--issue", "9", "--now", "1200"]).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");
    let ticked = tick(&world, "1270");
    let steps = did(&ticked);
    assert!(!steps.iter().any(|step| step.starts_with("cut_train")), "{steps:?}");
    assert!(train.join("out/old").exists());
    assert_eq!(train_worktree(&world), train);
}

#[test]
fn a_worktree_within_its_limit_is_left_as_it_is() {
    // within the floor
    let world = building(&[7, 8], TEN_BYTES);
    finish(&world, 7);
    tick(&world, "1090");
    land(&world, "1170");
    finish(&world, 8);
    let train = train_worktree(&world);
    let landed = land(&world, "1180");
    assert_eq!((landed["built"].as_u64(), landed["fresh"].as_u64()), (Some(20), Some(10)));
    assert_eq!(did(&tick(&world, "1270")), ["settle 7 done", "settle 8 done"]);
    assert!(train.join("out/build").exists());
    assert_eq!(train_worktree(&world), train);

    // past the floor, within three times its fresh build: a gate that builds
    // one sparse file past the floor, and ten bytes more each run
    let big = format!(
        "{TEN_BYTES} && dd if=/dev/zero of=out/big bs=1 count=0 seek={}",
        OUTGROWN_FLOOR + 1
    );
    let world = building(&[7, 8], &big);
    finish(&world, 7);
    tick(&world, "1090");
    let fresh = OUTGROWN_FLOOR + 11;
    assert_eq!(land(&world, "1170")["fresh"], fresh);
    finish(&world, 8);
    let train = train_worktree(&world);
    let landed = land(&world, "1180");
    assert_eq!(
        (landed["built"].as_u64(), landed["fresh"].as_u64()),
        (Some(fresh + 10), Some(fresh))
    );
    assert_eq!(did(&tick(&world, "1270")), ["settle 7 done", "settle 8 done"]);
    assert!(train.join("out/big").exists());
    assert_eq!(train_worktree(&world), train);
}

#[test]
fn a_worktree_with_no_fresh_build_past_the_floor_is_cut_once_and_then_follows_the_rule() {
    let world = building(&[7, 8], TEN_BYTES);
    finish(&world, 7);
    tick(&world, "1090");
    let train = train_worktree(&world);
    // built in before measuring began
    build_into(&train, "old", OUTGROWN_FLOOR + 1);
    let landed = land(&world, "1170");
    assert!(landed.get("fresh").is_none(), "{landed}");

    let (lines, _) = world.afk_lines(&[
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--ticks",
        "1",
        "--now",
        "1270",
        "--second-ms",
        "0",
    ]);
    let line = lines.iter().find(|line| line.contains("cut the train's worktree again"));
    let line = line.unwrap_or_else(|| panic!("{lines:#?}"));
    assert!(line.contains("(built 1.0 GiB, no fresh build on record)"), "{line}");
    let cut = train_worktree(&world);
    assert!(!cut.join("out").exists());

    // the next gate run there records its fresh build, and the rule is the 3× one
    finish(&world, 8);
    tick(&world, "1360");
    assert_eq!(train_worktree(&world), cut, "the train worker starts in the worktree cut");
    let next = land(&world, "1450");
    assert_eq!((next["built"].as_u64(), next["fresh"].as_u64()), (Some(10), Some(10)), "{next}");
    let ticked = tick(&world, "1540");
    assert_eq!(did(&ticked), ["settle 8 done"]);
    assert_eq!(train_worktree(&world), cut);
}

mod common;

use std::path::{Path, PathBuf};

use common::{World, commit, git};
use serde_json::{Value, json};

const LINE: &str = "refs/convoy/train/line/main";

fn tick(world: &World, now: &str) -> Value {
    let args =
        ["tick", "--instance", "alpha", "--host", "h1", "--now", now, "--worker-command", "agent"];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        // a worker asked after, and left or nudged, is not what these tests are about
        .filter(|step| step["do"] != "board")
        .filter(|step| {
            !((step["do"] == "judge" || step["do"] == "watch") && step["outcome"] != "done")
        })
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world with workers dispatched on `issues` at 1000.
fn fleet(issues: &[u64]) -> World {
    let world = World::new();
    world.set_gate("true");
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

/// The worker on `issue` finishes: commits, opens PR #`issue`0 and joins the
/// train → the join commit.
fn finish(world: &World, issue: u64) -> String {
    let dir = worktree_of(world, issue);
    commit(&dir, &format!("f{issue}.txt"), "x\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(issue * 10, issue, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    let answer = world.afk_in(&dir, &["land", "--issue", &issue.to_string(), "--now", "1050"]).ok();
    assert_eq!(answer["outcome"], "joined", "{answer}");
    answer["commit"].as_str().unwrap().to_string()
}

fn train_worktree(world: &World) -> PathBuf {
    world.root.join("wt/convoy-train")
}

/// The terminals open in a worktree.
fn terminals(world: &World, dir: &Path) -> Vec<Value> {
    let orca = world.orca();
    let rows = orca["terminals"].as_array().unwrap();
    let here = |t: &&Value| t["open"] == true && t["worktreePath"] == dir.to_str().unwrap();
    rows.iter().filter(here).cloned().collect()
}

fn status(world: &World, issue: u64) -> String {
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1100"]).ok();
    let mine = plan["mine"].as_array().unwrap();
    let row = mine.iter().find(|row| row["number"] == issue).expect("a claim of mine");
    row["status"].as_str().unwrap().to_string()
}

fn state(world: &World, issue: u64) -> String {
    let gh = world.gh();
    let rows = gh["issues"].as_array().unwrap();
    rows.iter().find(|row| row["number"] == issue).unwrap()["state"].as_str().unwrap().to_string()
}

#[test]
fn a_joined_pr_gets_a_train_worker_that_holds_no_claim_and_no_slot() {
    let world = fleet(&[7, 8, 9]);
    let tip = finish(&world, 7);
    world.orca_calls();

    let ticked = tick(&world, "1090");
    assert_eq!(did(&ticked), ["tend_train done", "heartbeat done"]);
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(ticked["held"], 2, "the train worker takes no slot");
    assert_eq!(world.claims().len(), 2, "…and holds no claim");

    let train = train_worktree(&world);
    let orca = world.orca();
    let row = orca["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == train.to_str().unwrap());
    assert_eq!(row.expect("the train's worktree")["linkedIssue"], Value::Null);
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1, "the worker's is the only terminal there");
    assert_eq!(open[0]["command"], "agent");
    let brief = PathBuf::from(git(&train, &["rev-parse", "--absolute-git-dir"]))
        .join("convoy-worker-prompt.md");
    let sent = open[0]["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0]["text"].as_str().unwrap().contains(brief.to_str().unwrap()));
    assert_eq!(sent[0]["enter"], true);
    let prompt = std::fs::read_to_string(&brief).unwrap();
    assert!(prompt.contains(&format!("{} land --train\n", env!("CARGO_BIN_EXE_afkconvoy"))));
    assert!(prompt.contains("train worker"));
    // cut at the base branch's tip, on a branch of its own
    assert_eq!(
        git(&train, &["rev-parse", "HEAD"]),
        world.remote(&["rev-parse", "refs/heads/main"])
    );
    assert_eq!(world.remote(&["rev-parse", LINE]), tip);
}

#[test]
fn a_train_worker_is_left_at_work_told_what_joined_and_started_again_when_gone() {
    let world = fleet(&[7, 8]);
    finish(&world, 7);
    tick(&world, "1090");
    let train = train_worktree(&world);
    let handle = terminals(&world, &train)[0]["handle"].clone();
    world.orca_calls();

    // it stopped on the tip it was told about: its silence is not this pass's
    assert_eq!(did(&tick(&world, "1180"))[0], "tend_train nothing");

    // more joined while it is at work: it is left alone
    let second = finish(&world, 8);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == handle {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": 1_200_000u64});
                terminal["lastOutputAt"] = json!(1_265_000u64);
            }
        }
    });
    assert_eq!(did(&tick(&world, "1270"))[0], "tend_train nothing");

    // it stopped: it is told that the train holds more
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["handle"] == handle {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_300_000u64});
            }
        }
    });
    world.orca_calls();
    assert_eq!(did(&tick(&world, "1360"))[0], "tend_train done");
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["handle"], handle, "the same worker");
    let sent = open[0]["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent[1]["text"].as_str().unwrap().starts_with("PRs have joined the landing train"));
    let calls: Vec<String> = world.orca_calls().iter().map(|call| call[..2].join(" ")).collect();
    assert!(!calls.contains(&"terminal create".to_string()), "{calls:?}");
    assert_eq!(world.remote(&["rev-parse", LINE]), second);
    assert_eq!(did(&tick(&world, "1450"))[0], "tend_train nothing", "told once per tip");

    // its terminal is gone: another is started, in the same worktree
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
    assert_eq!(did(&tick(&world, "1540"))[0], "tend_train done");
    let open = terminals(&world, &train);
    assert_eq!(open.len(), 1);
    assert_ne!(open[0]["handle"], handle);
    let trains = world.orca()["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["linkedIssue"].is_null())
        .count();
    assert_eq!(trains, 1, "no second worktree is cut beside it");
}

#[test]
fn a_landed_claim_is_settled_and_the_checkout_follows() {
    let world = fleet(&[7]);
    finish(&world, 7);
    tick(&world, "1090");
    let (train, worktree) = (train_worktree(&world), worktree_of(&world, 7));
    // the train worker lands the train
    let landed = world.afk_in(&train, &["land", "--train", "--merged-timeout", "5"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    let tip = world.remote(&["rev-parse", "refs/heads/main"]);
    assert_ne!(git(&world.clone, &["rev-parse", "main"]), tip);
    world.watch_pushes(None);

    let ticked = tick(&world, "1180");
    assert_eq!(did(&ticked), ["settle 7 done"]);
    assert_eq!(ticked["errors"], json!([]));
    assert!(!worktree.exists(), "the issue's worktree is removed");
    assert!(train.exists(), "the train's worktree is never swept");
    assert_eq!(world.claims(), []);
    assert_eq!(world.pushes(), ["refs/convoy/claim/7"]);
    // what landed is on this machine without anyone pulling
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), tip);
    assert_eq!(git(&world.clone, &["rev-parse", "origin/main"]), tip);
    assert_eq!(git(&world.clone, &["status", "--porcelain"]), "");
    assert!(world.clone.join("f7.txt").exists());
}

/// Puts the train on the base branch as a landing cut right after its push
/// would leave it: nothing else done.
fn cut_after_the_push(world: &World) -> String {
    let tip = world.remote(&["rev-parse", LINE]);
    world.remote(&["update-ref", "refs/heads/main", &tip]);
    tip
}

#[test]
fn a_landing_cut_after_its_push_is_finished_from_the_base_branch() {
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    assert_eq!(state(&world, 7), "open");
    assert_eq!(status(&world, 7), "landed", "never treated as PR-less");

    let ticked = tick(&world, "1180");
    assert_eq!(did(&ticked), ["settle 7 done"]);
    assert_eq!(state(&world, 7), "closed");
    assert_eq!(world.claims(), []);
    assert!(world.orca()["worktrees"].as_array().unwrap().is_empty());
}

#[test]
fn a_cut_landing_whose_settling_fails_keeps_the_claim_with_the_issue_closed() {
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    world.with_orca(|orca| orca["rm_fails"] = json!(true));

    let ticked = tick(&world, "1180");
    assert_eq!(did(&ticked), ["settle 7 failed", "heartbeat done"]);
    assert_eq!(state(&world, 7), "closed", "the issue first");
    assert_eq!(world.claims().len(), 1, "the claim last: held for the next tick");

    world.with_orca(|orca| orca["rm_fails"] = json!(false));
    assert_eq!(did(&tick(&world, "1270")), ["settle 7 done"]);
    assert_eq!(world.claims(), []);
}

#[test]
fn a_landed_pr_github_still_shows_open_is_closed_naming_its_commit() {
    let world = fleet(&[7]);
    let commit = finish(&world, 7);
    world.with_gh(|gh| gh["slow_merge"] = json!([70]));
    cut_after_the_push(&world);
    assert_eq!(status(&world, 7), "landed");

    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(state(&world, 7), "closed");
    let gh = world.gh();
    assert_eq!(gh["prs"][0]["state"], "closed");
    let comments = world.comments(70);
    assert_eq!(comments.len(), 1);
    assert!(comments[0].contains(&commit) && comments[0].contains("`main`"), "{comments:?}");
}

#[test]
fn an_open_pr_pushed_to_since_it_landed_is_not_a_cut_landing() {
    let world = fleet(&[7]);
    finish(&world, 7);
    world.with_gh(|gh| gh["slow_merge"] = json!([70]));
    let dir = worktree_of(&world, 7);
    commit(&dir, "more.txt", "more\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    cut_after_the_push(&world);
    assert_eq!(status(&world, 7), "joining", "its worker joins again with what it pushed");
    assert_eq!(did(&tick(&world, "1180")), ["heartbeat done"]);
    assert_eq!(state(&world, 7), "open");
}

#[test]
fn a_checkout_that_cannot_follow_never_fails_the_release() {
    // a local base branch that diverged
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    let local = commit(&world.clone, "local.txt", "mine\n");
    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), local, "left as it was");
    assert_eq!(world.claims(), []);

    // changes in the way
    let world = fleet(&[7]);
    finish(&world, 7);
    cut_after_the_push(&world);
    let before = git(&world.clone, &["rev-parse", "main"]);
    std::fs::write(world.clone.join("f7.txt"), "in the way\n").unwrap();
    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), before);
    assert_eq!(std::fs::read_to_string(world.clone.join("f7.txt")).unwrap(), "in the way\n");
    assert_eq!(world.claims(), []);
}

#[test]
fn a_base_branch_that_is_not_checked_out_follows_by_its_ref() {
    let world = fleet(&[7]);
    finish(&world, 7);
    let tip = cut_after_the_push(&world);
    git(&world.clone, &["checkout", "-q", "-b", "side"]);
    assert_eq!(did(&tick(&world, "1180")), ["settle 7 done"]);
    assert_eq!(git(&world.clone, &["rev-parse", "main"]), tip);
    assert_eq!(
        git(&world.clone, &["rev-parse", "HEAD"]),
        git(&world.clone, &["rev-parse", "side"])
    );
    assert!(!world.clone.join("f7.txt").exists(), "the checkout itself is not touched");
}

#[test]
fn a_new_worker_in_a_worktree_was_never_nudged() {
    let world = fleet(&[7]);
    finish(&world, 7);
    tick(&world, "1090");
    let train = train_worktree(&world);
    let nudged =
        PathBuf::from(git(&train, &["rev-parse", "--absolute-git-dir"])).join("convoy-nudged");
    std::fs::write(&nudged, "1100").unwrap();

    // the worker that was nudged is gone, and another is started in its place
    world.with_orca(|orca| orca["terminals"] = json!([]));
    assert_eq!(did(&tick(&world, "1180"))[0], "tend_train done");
    assert_eq!(terminals(&world, &train).len(), 1);
    assert!(!nudged.exists(), "the record of the nudge went with the worker it was sent to");
}

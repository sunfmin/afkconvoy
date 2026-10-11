mod common;

use std::path::{Path, PathBuf};

use common::{World, git};
use serde_json::Value;

const LINE: &str = "refs/convoy/train/line/main";

/// A world whose gate is red exactly while the file `red` exists beside it.
fn world() -> World {
    let world = World::new();
    world.set_gate(&format!("echo gate-ran; test ! -f {}", world.root.join("red").display()));
    world
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A worker's worktree for `issue`, cut from the base branch, with one commit
/// pushed and PR #`issue`0 open for it.
fn worker(world: &World, issue: u64, file: &str, text: &str) -> PathBuf {
    let dir = world.root.join(format!("wt-{issue}"));
    let branch = format!("issue-{issue}");
    git(&world.clone, &["fetch", "-q", "origin"]);
    git(
        &world.clone,
        &["worktree", "add", "-q", "-b", &branch, dir.to_str().unwrap(), "origin/main"],
    );
    commit(&dir, file, text);
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.issue(issue, &["ready-for-agent"]);
    world.pr(issue * 10, issue, &branch);
    dir
}

fn land(world: &World, dir: &Path, issue: u64) -> Value {
    world.afk_in(dir, &["land", "--issue", &issue.to_string(), "--now", "7000"]).ok()
}

fn line(world: &World) -> String {
    world.remote(&["rev-parse", LINE])
}

fn parents(world: &World, commit: &str) -> Vec<String> {
    let listed = world.remote(&["log", "-1", "--format=%P", commit]);
    listed.split_whitespace().map(str::to_string).collect()
}

fn base(world: &World) -> String {
    world.remote(&["rev-parse", "refs/heads/main"])
}

/// The status the rebuild gives my claim on `issue`.
fn status(world: &World, issue: u64) -> String {
    let plan = world.afk(&["plan", "--instance", "me", "--now", "10000"]).ok();
    let mine = plan["mine"].as_array().unwrap();
    let row = mine.iter().find(|row| row["number"] == issue).expect("a claim of mine");
    row["status"].as_str().unwrap().to_string()
}

fn claim(world: &World, issue: u64) {
    let issue = issue.to_string();
    world.afk(&["claim", "--issue", &issue, "--instance", "me", "--now", "10000"]).ok();
}

#[test]
fn a_pr_joins_the_train_as_one_merge_commit() {
    let world = world();
    let dir = worker(&world, 7, "seven.txt", "7\n");
    claim(&world, 7);
    assert_eq!(status(&world, 7), "joining");
    // what the worker committed since its last push is pushed by the join
    let head = commit(&dir, "more.txt", "more\n");
    let started = base(&world);

    let answer = land(&world, &dir, 7);
    assert_eq!(answer["outcome"], "joined");
    assert_eq!(answer["again"], false);
    assert_eq!((answer["issue"].as_u64(), answer["pr"].as_u64()), (Some(7), Some(70)));
    assert_eq!(answer["head"], head.as_str());

    let tip = line(&world);
    assert_eq!(answer["commit"], tip.as_str());
    assert_eq!(parents(&world, &tip), [started.clone(), head.clone()]);
    assert_eq!(world.remote(&["log", "-1", "--format=%B", &tip]), "pr 70 (#70)\n\nCloses #7");
    assert_eq!(world.remote(&["log", "-1", "--format=%ct", &tip]), "7000");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-7"]), head);
    assert_eq!(base(&world), started, "joining lands nothing");
    // the worktree was not touched and the branch did not move
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), "issue-7");
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");

    assert_eq!(status(&world, 7), "joined");
}

#[test]
fn a_pr_already_on_the_train_is_joined_as_it_stands() {
    let world = world();
    let dir = worker(&world, 7, "seven.txt", "7\n");
    let first = land(&world, &dir, 7);
    world.watch_pushes(None);
    let again = land(&world, &dir, 7);
    assert_eq!(again["outcome"], "joined");
    assert_eq!(again["again"], true);
    assert_eq!(again["commit"], first["commit"]);
    assert_eq!(world.pushes(), [] as [String; 0]);
}

#[test]
fn a_pr_whose_worker_pushed_again_joins_again_and_nothing_is_rewritten() {
    let world = world();
    let seven = worker(&world, 7, "seven.txt", "7\n");
    let eight = worker(&world, 8, "eight.txt", "8\n");
    claim(&world, 7);
    let first = land(&world, &seven, 7)["commit"].as_str().unwrap().to_string();
    let second = land(&world, &eight, 8)["commit"].as_str().unwrap().to_string();
    assert_eq!(parents(&world, &second)[0], first, "a PR joins onto the train's tip");
    assert_eq!(status(&world, 7), "joined");

    let head = commit(&seven, "fix.txt", "fix\n");
    git(&seven, &["push", "-q", "origin", "HEAD"]);
    assert_eq!(status(&world, 7), "joining", "the train holds another head than the PR has");

    let third = land(&world, &seven, 7);
    assert_eq!(third["again"], false);
    let tip = line(&world);
    assert_eq!(parents(&world, &tip), [second.clone(), head]);
    let below = world.remote(&["rev-list", "--first-parent", &tip]);
    let below: Vec<&str> = below.lines().collect();
    assert_eq!(&below[..3], [tip.as_str(), second.as_str(), first.as_str()]);
    assert_eq!(status(&world, 7), "joined");
}

#[test]
fn prs_joining_at_once_all_get_on_one_line() {
    let world = world();
    let issues = [3u64, 4, 5, 6];
    let dirs: Vec<PathBuf> =
        issues.iter().map(|n| worker(&world, *n, &format!("f{n}.txt"), "x\n")).collect();
    std::thread::scope(|scope| {
        for (issue, dir) in issues.iter().zip(&dirs) {
            let world = &world;
            scope.spawn(move || assert_eq!(land(world, dir, *issue)["outcome"], "joined"));
        }
    });
    let tip = line(&world);
    let joins = world.remote(&["log", "--first-parent", "--format=%s", &format!("main..{tip}")]);
    let mut joins: Vec<&str> = joins.lines().collect();
    joins.sort();
    assert_eq!(joins, ["pr 30 (#30)", "pr 40 (#40)", "pr 50 (#50)", "pr 60 (#60)"]);
    for n in issues {
        assert_eq!(world.remote(&["show", &format!("{tip}:f{n}.txt")]), "x");
    }
}

#[test]
fn a_pr_that_conflicts_with_the_train_resolves_it_in_its_own_branch() {
    let world = world();
    let seven = worker(&world, 7, "README.md", "seven\n");
    let eight = worker(&world, 8, "README.md", "eight\n");
    let tip = land(&world, &seven, 7)["commit"].as_str().unwrap().to_string();

    let answer = land(&world, &eight, 8);
    assert_eq!(answer["outcome"], "conflict");
    assert_eq!(answer["files"], serde_json::json!(["README.md"]));
    assert_eq!(answer["worktree"], eight.to_str().unwrap());
    assert_eq!(line(&world), tip, "the train is as it was");
    assert_eq!(git(&eight, &["rev-parse", "MERGE_HEAD"]), tip, "the merge is left in progress");

    // run again before it is resolved: the same conflict, reported as it stands
    let again = land(&world, &eight, 8);
    assert_eq!(again["outcome"], "conflict");
    assert_eq!(again["files"], serde_json::json!(["README.md"]));

    std::fs::write(eight.join("README.md"), "seven and eight\n").unwrap();
    git(&eight, &["add", "."]);
    // resolved but not committed is not something to join with
    let error = world.afk_in(&eight, &["land", "--issue", "8"]).error();
    assert!(error.contains("uncommitted changes"), "{error}");
    git(&eight, &["commit", "-q", "--no-edit"]);
    let head = git(&eight, &["rev-parse", "HEAD"]);

    let joined = land(&world, &eight, 8);
    assert_eq!(joined["outcome"], "joined");
    assert_eq!(parents(&world, &line(&world)), [tip, head.clone()]);
    assert_eq!(world.remote(&["show", &format!("{LINE}:README.md")]), "seven and eight");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-8"]), head);
}

#[test]
fn a_line_the_base_branch_holds_is_no_train() {
    let world = world();
    let seven = worker(&world, 7, "seven.txt", "7\n");
    let eight = worker(&world, 8, "eight.txt", "8\n");
    claim(&world, 7);
    let landed = land(&world, &seven, 7)["commit"].as_str().unwrap().to_string();
    // the train lands, and the base branch moves on past it
    world.remote(&["update-ref", "refs/heads/main", &landed]);
    let moved = world.land_elsewhere("else.txt");
    assert_eq!(status(&world, 7), "joining", "nothing is on a train the base branch holds");

    let answer = land(&world, &eight, 8);
    assert_eq!(answer["outcome"], "joined");
    let head = git(&eight, &["rev-parse", "HEAD"]);
    assert_eq!(parents(&world, &line(&world)), [moved, head]);

    // a PR whose head the base branch holds has nothing left to join
    let merged = land(&world, &seven, 7);
    assert_eq!(merged["outcome"], "merged");
}

#[test]
fn land_refuses_what_it_cannot_join() {
    let world = world();
    let dir = worker(&world, 7, "seven.txt", "7\n");
    let error = world.afk_in(&dir, &["land", "--issue", "9"]).error();
    assert!(error.contains("no open PR closes issue #9"), "{error}");

    std::fs::write(dir.join("seven.txt"), "edited\n").unwrap();
    let error = world.afk_in(&dir, &["land", "--issue", "7"]).error();
    assert!(error.contains("uncommitted changes to tracked files"), "{error}");
    assert!(world.remote(&["for-each-ref", "refs/convoy/train"]).is_empty());
}

#[test]
fn a_worktree_behind_its_pr_catches_up_before_it_joins() {
    let world = world();
    let dir = worker(&world, 7, "seven.txt", "7\n");
    let head = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["reset", "-q", "--hard", "HEAD^"]);
    let answer = land(&world, &dir, 7);
    assert_eq!(answer["head"], head.as_str());
    assert_eq!(parents(&world, &line(&world))[1], head);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-7"]), head);
}

fn gate_records(world: &World) -> usize {
    world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/gate"]).lines().count()
}

#[test]
fn gate_train_gates_the_branch_with_the_train_merged_in() {
    let world = world();
    let seven = worker(&world, 7, "seven.txt", "7\n");
    let eight = worker(&world, 8, "eight.txt", "8\n");

    // no train yet: what the PR lands behind is the base branch
    let moved = world.land_elsewhere("else.txt");
    let answer = world.afk_in(&seven, &["gate", "--train"]).ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["merged"], moved.as_str());
    assert_eq!(answer["recorded"], true);
    assert!(seven.join("else.txt").exists());
    let tip = land(&world, &seven, 7)["commit"].as_str().unwrap().to_string();

    let gate = world.afk_in(&eight, &["gate", "--train"]);
    assert!(gate.stderr.contains("gate-ran"));
    let answer = gate.ok();
    assert_eq!(answer["merged"], tip.as_str());
    assert_eq!(answer["recorded"], true);
    let head = git(&eight, &["rev-parse", "HEAD"]);
    assert_eq!(answer["head"], head.as_str());
    assert_eq!(git(&eight, &["rev-parse", "HEAD^2"]), tip, "a merge, never a rebase");
    assert!(eight.join("seven.txt").exists());

    // the tree gated is the tree the PR joins the train with
    let joined = land(&world, &eight, 8)["commit"].as_str().unwrap().to_string();
    assert_eq!(
        world.remote(&["rev-parse", &format!("{joined}^{{tree}}")]),
        git(&eight, &["rev-parse", "HEAD^{tree}"])
    );
}

#[test]
fn a_gate_train_merge_that_conflicts_runs_no_gate() {
    let world = world();
    let seven = worker(&world, 7, "README.md", "seven\n");
    let eight = worker(&world, 8, "README.md", "eight\n");
    land(&world, &seven, 7);

    let gate = world.afk_in(&eight, &["gate", "--train"]);
    assert!(!gate.stderr.contains("gate-ran"), "{}", gate.stderr);
    let answer = gate.ok();
    assert_eq!(answer["status"], "conflict");
    assert_eq!(answer["files"], serde_json::json!(["README.md"]));
    assert_eq!(answer["recorded"], false);
    assert_eq!(gate_records(&world), 0);

    // still unresolved: still no run
    let again = world.afk_in(&eight, &["gate", "--train"]);
    assert!(!again.stderr.contains("gate-ran"));
    assert_eq!(again.ok()["status"], "conflict");

    std::fs::write(eight.join("README.md"), "both\n").unwrap();
    git(&eight, &["add", "."]);
    git(&eight, &["commit", "-q", "--no-edit"]);
    let answer = world.afk_in(&eight, &["gate", "--train"]).ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["recorded"], true);
    assert_eq!(land(&world, &eight, 8)["outcome"], "joined");
}

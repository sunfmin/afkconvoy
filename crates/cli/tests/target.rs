//! Task destinations survive new processes and are never inferred from a PR.
use crate::common::{World, commit, git};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn branch(world: &World) -> String {
    let tip = commit(&world.clone, "delivery.txt", "only on delivery\n");
    git(&world.clone, &["push", "-q", "origin", "HEAD:delivery/ABC-123"]);
    git(&world.clone, &["reset", "--hard", "origin/main"]);
    tip
}

fn dispatch(world: &World) -> Value {
    world
        .afk(&["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1", "--now", "1000"])
        .ok()
}

fn brief(dir: &Path) -> String {
    let notes = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(notes.join("convoy-worker-prompt.md")).unwrap()
}

#[test]
fn selected_target_cuts_continues_gates_lands_and_settles_only_its_task() {
    let world = World::new();
    world.set_gate("true");
    let main = world.remote(&["rev-parse", "refs/heads/main"]);
    let selected = branch(&world);
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &[]);
    world.afk(&["target", "--issue", "7", "--branch", "delivery/ABC-123"]).ok();
    let started = dispatch(&world);
    let dir = PathBuf::from(started["worktree"].as_str().unwrap());
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), selected);
    assert!(brief(&dir).contains("gh pr create --base delivery/ABC-123"));
    commit(&dir, "work.txt", "task seven\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    // A new process, and then a new fleet instance after takeover, retain the
    // same target and worktree, even with a different configured base.
    git(&world.clone, &["push", "-q", "origin", "origin/main:refs/heads/other-base"]);
    let config = std::fs::read_to_string(world.clone.join("docs/agents/afkconvoy.md")).unwrap();
    std::fs::write(
        world.clone.join("docs/agents/afkconvoy.md"),
        config.replace("base_branch: main", "base_branch: other-base"),
    )
    .unwrap();
    let recovered = world.afk(&["recovery", "--issue", "7"]).ok();
    assert_eq!(recovered["base"], "delivery/ABC-123");
    assert_eq!(recovered["branch"]["commits_ahead"], 1);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
    world
        .afk(&[
            "takeover",
            "--yes",
            "--from",
            "alpha",
            "--instance",
            "beta",
            "--host",
            "h1",
            "--now",
            "1100",
        ])
        .ok();
    let continued = world
        .afk(&["dispatch", "--issue", "7", "--instance", "beta", "--host", "h1", "--now", "1100"])
        .ok();
    assert_eq!(continued["worktree"], started["worktree"], "{continued}");
    assert!(brief(&dir).contains("base is `delivery/ABC-123`"));
    let switched = world
        .afk(&[
            "switch",
            "--harness",
            "claude-code",
            "7",
            "--instance",
            "beta",
            "--host",
            "h1",
            "--now",
            "1150",
        ])
        .ok();
    assert_eq!(switched["failed"], json!([]), "{switched}");
    assert!(brief(&dir).contains("base is `delivery/ABC-123`"));
    world.pr(70, 7, started["branch"].as_str().unwrap());
    world.with_gh(|gh| gh["prs"][0]["baseRefName"] = json!("delivery/ABC-123"));
    let landed = world.afk_in(&dir, &["land", "--issue", "7"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert_eq!(landed["target_branch"], "delivery/ABC-123");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), main);
    assert_eq!(world.remote(&["show", "refs/heads/delivery/ABC-123:work.txt"]), "task seven");
    assert_eq!(world.gh()["issues"][0]["state"], "closed");
    assert_eq!(world.gh()["issues"][1]["state"], "open");
    let ticked = world.afk(&["tick", "--instance", "beta", "--host", "h1", "--now", "1200"]).ok();
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(world.claims().is_empty(), "settlement follows the target's history");
}

#[test]
fn wrong_base_missing_branch_and_lost_or_corrupt_records_refuse_without_fallback() {
    let world = World::new();
    world.set_gate("true");
    branch(&world);
    let dir = world.worker(7, "work.txt", "seven");
    world.afk(&["target", "--issue", "7", "--branch", "delivery/ABC-123"]).ok();
    let main = world.remote(&["rev-parse", "refs/heads/main"]);
    let wrong = world.afk_in(&dir, &["land", "--issue", "7"]).error();
    assert!(wrong.contains("pinned target") && wrong.contains("main"), "{wrong}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), main);
    world.afk(&["claim", "--issue", "7", "--instance", "alpha"]).ok();
    let changed = world.afk(&["target", "--issue", "7", "--branch", "main"]).error();
    assert!(changed.contains("pinned"), "{changed}");
    git(&world.clone, &["push", "-q", "origin", ":delivery/ABC-123"]);
    let missing = world.afk_in(&dir, &["land", "--issue", "7"]).error();
    assert!(missing.contains("delivery/ABC-123") && missing.contains("missing"), "{missing}");
    git(&world.clone, &["push", "-q", "origin", "HEAD:delivery/ABC-123"]);
    let target = world.remote(&["rev-parse", "refs/convoy/target/7"]);
    git(&world.clone, &["push", "-q", "origin", ":refs/convoy/target/7"]);
    let lost = world.afk(&["recovery", "--issue", "7"]).error();
    assert!(lost.contains("missing or contradictory pinned target"), "{lost}");
    git(&world.clone, &["push", "-q", "origin", &format!("{target}:refs/convoy/target/7")]);
    world.afk(&["recovery", "--issue", "7"]).ok();
    git(&world.clone, &["push", "-q", "--force", "origin", "HEAD:refs/convoy/target/7"]);
    let corrupt = world.afk(&["target", "--issue", "7"]).error();
    assert!(corrupt.contains("contradictory pinned target"), "{corrupt}");
    assert_eq!(world.gh()["issues"][0]["state"], "open");
}

#[test]
fn unresolved_default_is_pinned_before_claim_and_survives_config_change() {
    let world = World::new();
    world.issue(7, &[]);
    assert_eq!(world.afk(&["target", "--issue", "7"]).ok()["target_branch"], "main");
    world.afk(&["claim", "--issue", "7", "--instance", "alpha"]).ok();
    git(&world.clone, &["push", "-q", "origin", "HEAD:other"]);
    let config = std::fs::read_to_string(world.clone.join("docs/agents/afkconvoy.md")).unwrap();
    std::fs::write(
        world.clone.join("docs/agents/afkconvoy.md"),
        config.replace("base_branch: main", "base_branch: other"),
    )
    .unwrap();
    assert_eq!(world.afk(&["target", "--issue", "7"]).ok()["target_branch"], "main");
}

#[test]
fn unreadable_explicit_target_prevents_claim_without_holding_other_tasks_back() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    git(&world.clone, &["push", "-q", "origin", "HEAD:delivery/broken"]);
    world.afk(&["target", "--issue", "7", "--branch", "delivery/broken"]).ok();
    git(&world.clone, &["push", "-q", "--force", "origin", "HEAD:refs/convoy/target/7"]);
    let error =
        world.afk(&["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1"]).error();
    assert!(error.contains("unreadable target"), "{error}");
    assert!(world.claims().is_empty());
    let other =
        world.afk(&["dispatch", "--issue", "8", "--instance", "alpha", "--host", "h1"]).ok();
    assert_eq!(other["started"], true);
    assert_eq!(world.claims().iter().map(|(n, _)| *n).collect::<Vec<_>>(), [8]);
}

#[test]
fn a_target_repair_is_dispatched_and_landed_on_the_affected_branch() {
    let world = World::new();
    world.set_gate("test ! -f broken.txt || test -f repaired.txt");
    let main = world.remote(&["rev-parse", "refs/heads/main"]);
    let broken = commit(&world.clone, "broken.txt", "broken target\n");
    git(
        &world.clone,
        &["push", "-q", "origin", "HEAD:delivery/red", "HEAD:refs/convoy/base/red/delivery/red"],
    );
    git(&world.clone, &["reset", "--hard", "origin/main"]);
    world.issue(7, &[]);
    world.afk(&["target", "--issue", "7", "--branch", "delivery/red"]).ok();
    let ticked = world.afk(&["tick", "--instance", "alpha", "--host", "h1"]).ok();
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    let gh = world.gh();
    let repair = gh["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"].as_str().unwrap_or("").contains("Repair the red delivery/red"))
        .unwrap()["number"]
        .as_u64()
        .unwrap();
    let orca = world.orca();
    let row =
        orca["worktrees"].as_array().unwrap().iter().find(|w| w["linkedIssue"] == repair).unwrap();
    let dir = PathBuf::from(row["path"].as_str().unwrap());
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), broken);
    assert!(brief(&dir).contains("base is `delivery/red`"));
    commit(&dir, "repaired.txt", "repaired\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(2000, repair, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    world.with_gh(|gh| gh["prs"][0]["baseRefName"] = json!("delivery/red"));
    let fixed = world.afk_in(&dir, &["land", "--issue", &repair.to_string()]).ok();
    assert_eq!(fixed["outcome"], "merged", "{fixed}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), main);
    assert_eq!(world.remote(&["show", "refs/heads/delivery/red:repaired.txt"]), "repaired");
    assert_eq!(world.state(7), "open");
    assert_eq!(world.state(repair), "closed");
}

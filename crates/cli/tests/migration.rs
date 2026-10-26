//! Lines left by an older version remain recoverable until their work lands.
use crate::common::{World, git};
use afkconvoy_core::landing::{Of, merge_message};
use afkconvoy_core::tracker::Tracker;

const LINE: &str = "refs/convoy/train/line/main";

fn merge(world: &World, base: &str, head: &str, issue: u64) -> String {
    let tree = git(&world.clone, &["merge-tree", "--write-tree", base, head]);
    let message = merge_message("Legacy change", issue * 10, &Of::Issue(issue), &Tracker::Own);
    git(
        &world.clone,
        &["commit-tree", tree.lines().next().unwrap(), "-p", base, "-p", head, "-m", &message],
    )
}

fn seeded() -> World {
    let world = World::new();
    world.set_gate("true");
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    world
}

#[test]
fn a_recorded_legacy_tip_lands_by_fast_forward_and_its_issue_closes() {
    let world = seeded();
    let worker = world.worker(1, "one.txt", "one");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let head = git(&worker, &["rev-parse", "HEAD"]);
    let tip = merge(&world, &base, &head, 1);
    git(&world.clone, &["push", "-q", "origin", &format!("{tip}:{LINE}")]);
    let checkout = world.root.join("legacy");
    git(&world.clone, &["worktree", "add", "-q", "--detach", checkout.to_str().unwrap(), &tip]);
    assert_eq!(world.afk_in(&checkout, &["gate"]).ok()["recorded"], true);
    let answer = world.afk(&["land", "--train"]).ok();
    assert_eq!(answer["source"], "recorded", "{answer}");
    assert_eq!(answer["commit"], tip);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), tip);
    assert_eq!(world.state(1), "closed");
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
}

#[test]
fn unrecorded_legacy_prs_take_turns_and_no_work_on_the_old_line_is_lost() {
    let world = seeded();
    let one = world.worker(1, "one.txt", "one");
    let two = world.worker(2, "two.txt", "two");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let a = merge(&world, &base, &git(&one, &["rev-parse", "HEAD"]), 1);
    let tip = merge(&world, &a, &git(&two, &["rev-parse", "HEAD"]), 2);
    git(&world.clone, &["push", "-q", "origin", &format!("{tip}:{LINE}")]);
    let answer = world.afk(&["land", "--train"]).ok();
    assert_eq!(answer["outcome"], "merged", "{answer}");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    assert_eq!(world.remote(&["show", &format!("{base}:one.txt")]), "one");
    assert_eq!(world.remote(&["show", &format!("{base}:two.txt")]), "two");
    assert_eq!((world.state(1), world.state(2)), ("closed".into(), "closed".into()));
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
    // The old history is preserved even though the PRs landed in new turns.
    assert_eq!(world.remote(&["merge-base", &tip, &base]), tip);
}

#[test]
fn a_closed_legacy_pr_does_not_strand_its_recorded_head() {
    let world = seeded();
    let worker = world.worker(1, "one.txt", "one");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let tip = merge(&world, &base, &git(&worker, &["rev-parse", "HEAD"]), 1);
    git(&world.clone, &["push", "-q", "origin", &format!("{tip}:{LINE}")]);
    world.with_gh(|gh| {
        gh["prs"][0]["state"] = serde_json::json!("closed");
    });
    world.close_issue(1);
    let answer = world.afk(&["land", "--train"]).ok();
    assert_eq!(answer["outcome"], "merged", "{answer}");
    assert_eq!(world.remote(&["show", "refs/heads/main:one.txt"]), "one");
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", LINE]).is_empty());
}

#[test]
fn a_conflicted_legacy_repair_is_retained_once_and_dispatched_to_its_owner() {
    let world = seeded();
    let worker = world.worker(1, "one.txt", "one");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let merge = merge(&world, &base, &git(&worker, &["rev-parse", "HEAD"]), 1);
    let old = world.root.join("old");
    git(&world.clone, &["worktree", "add", "-q", "--detach", old.to_str().unwrap(), &merge]);
    crate::common::commit(&old, "README.md", "old repair");
    let tip = git(&old, &["rev-parse", "HEAD"]);
    git(&world.clone, &["push", "-q", "origin", &format!("{tip}:{LINE}")]);
    crate::common::commit(&world.clone, "README.md", "outside change");
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    let answer = world.afk(&["land", "--train"]).ok();
    assert_eq!(answer["outcome"], "conflict", "{answer}");
    let path = std::path::PathBuf::from(answer["worktree"].as_str().unwrap());
    let issue = answer["issue"].as_u64().unwrap();
    assert_eq!(git(&path, &["diff", "--name-only", "--diff-filter=U"]), "README.md");
    let count = world.worktrees().len();
    let again = world.afk(&["land", "--train"]).ok();
    assert_eq!(again["worktree"], answer["worktree"]);
    assert_eq!(again["outcome"], "pending");
    crate::common::write(&path, "README.md", "resolved locally");
    git(&path, &["add", "README.md"]);
    let staged = world.afk(&["land", "--train"]).ok();
    assert_eq!(staged["outcome"], "pending");
    assert_eq!(world.worktrees().len(), count, "retries do not leak workspaces");
    let tick = world.afk(&["tick", "--instance", "migration-host"]).ok();
    assert!(
        tick["did"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["do"] == "dispatch" && step["issue"] == issue),
        "{tick}"
    );
    assert_eq!(std::fs::read_to_string(path.join("README.md")).unwrap(), "resolved locally");
    let before = world.orca()["terminals"].clone();
    assert_eq!(world.afk(&["land", "--train"]).ok()["outcome"], "pending");
    assert_eq!(world.orca()["terminals"], before, "migration leaves the owner's worker alone");
}

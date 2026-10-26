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

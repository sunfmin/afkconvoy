//! The fixture itself: what every later test stands on.

mod common;

use common::{BASE, World, git};

#[test]
fn the_bare_repo_is_the_clones_remote() {
    let world = World::new();
    std::fs::write(world.clone.join("a.txt"), "a\n").unwrap();
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "a"]);
    git(&world.clone, &["push", "-q", "origin", BASE]);
    assert_eq!(world.remote(&["rev-parse", BASE]), git(&world.clone, &["rev-parse", "HEAD"]));
}

#[test]
fn gh_and_orca_on_path_are_the_stand_ins() {
    let world = World::new();
    let gh = world.command("gh", &world.clone).args(["issue", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&gh.stderr).contains("fake gh"));
    assert_eq!(world.gh()["calls"], serde_json::json!([["issue", "list"]]));

    let orca = world.command("orca", &world.clone).args(["status", "--json"]).output().unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&orca.stdout).unwrap();
    assert_eq!(answer["ok"], false);
    assert_eq!(world.orca()["calls"], serde_json::json!([["status", "--json"]]));
}

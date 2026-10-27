//! The fixture itself: what every later test stands on.

use crate::common;

use common::{BASE, World, git};

#[test]
fn the_bare_repo_is_the_clones_remote() {
    let world = World::new();
    std::fs::write(world.clone.join("a.txt"), "a\n").unwrap();
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "a"]);
    git(&world.clone, &["push", "-q", "origin", BASE]);
    assert_eq!(world.remote(&["rev-parse", BASE]), git(&world.clone, &["rev-parse", "HEAD"]));
    let began = std::time::Instant::now();
    drop(world);
    common::fixture_timing("world teardown", began);
}

#[test]
fn gh_and_orca_on_path_are_the_stand_ins() {
    let world = World::new();
    let gh = world.command("gh", &world.clone).args(["issue", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&gh.stderr).contains("fake gh"));
    assert_eq!(world.gh()["calls"], serde_json::json!([["issue", "list"]]));

    let orca =
        world.command("orca", &world.clone).args(["no-such-call", "--json"]).output().unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&orca.stdout).unwrap();
    assert_eq!(answer["ok"], false);
    assert_eq!(world.orca()["calls"], serde_json::json!([["no-such-call", "--json"]]));
}

#[test]
fn repository_templates_leave_each_world_with_its_own_writable_git_history() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let first = World::new();
    let second = World::new();
    first.reuse_repository_templates();
    second.reuse_repository_templates();
    let files = [("a directory/skill.md", "the original\n")];
    let a = first.repo("acme/role with spaces", &files);
    let b = second.repo("acme/role with spaces", &files);
    let original = git(&a, &["rev-parse", "HEAD"]);
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), original);
    assert_eq!(git(&b, &["rev-list", "--count", "HEAD"]), "2");
    for relative in ["a directory/skill.md", ".git/index", ".git/refs/heads/main"] {
        assert_ne!(
            std::fs::metadata(a.join(relative)).unwrap().ino(),
            std::fs::metadata(b.join(relative)).unwrap().ino(),
            "{relative} must not be hard-linked"
        );
    }
    common::commit(&a, "a directory/skill.md", "changed\n");
    std::fs::set_permissions(a.join(".git/index"), std::fs::Permissions::from_mode(0o400)).unwrap();
    drop(first);
    assert!(!a.exists(), "even the read-only index is cleaned up");
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), original);
    assert_eq!(std::fs::read_to_string(b.join("a directory/skill.md")).unwrap(), "the original\n");
    let c = second.repo("acme/a later copy", &files);
    assert_eq!(git(&c, &["rev-parse", "HEAD"]), original, "the seed stays intact");
    assert_eq!(git(&c, &["status", "--porcelain"]), "");
    common::commit(&b, "a directory/skill.md", "independent\n");
    assert_eq!(git(&b, &["rev-list", "--count", "HEAD"]), "3");
    assert_eq!(git(&c, &["rev-list", "--count", "HEAD"]), "2");
}

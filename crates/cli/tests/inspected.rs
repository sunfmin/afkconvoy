//! `afkconvoy inspected`: an inspection, the operator on its host, says what
//! held the work back, what it did and what it could not fix, from the
//! checkout it was started in, for its `run` to log and keep.

use crate::common;

use std::process::Command;

use common::World;

/// A checkout of `world`'s repo named as an inspection's is.
fn inspection_checkout(world: &World) -> std::path::PathBuf {
    let checkout = world.root.join("wt").join("afkconvoy-inspection");
    let made = Command::new("git")
        .current_dir(&world.clone)
        .args(["worktree", "add", "-q", "--detach", checkout.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(made.success());
    checkout
}

/// What the inspection in `checkout` said, as its `run` reads it.
fn kept(checkout: &std::path::Path) -> serde_json::Value {
    let git_dir = Command::new("git")
        .current_dir(checkout)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    let kept =
        std::fs::read_to_string(format!("{}/convoy-inspected.json", git_dir.trim())).unwrap();
    serde_json::from_str(&kept).unwrap()
}

#[test]
fn an_inspection_keeps_what_it_did_in_its_checkout_and_the_last_said_stands() {
    let world = World::new();
    let checkout = inspection_checkout(&world);
    let said = world
        .afk_in(
            &checkout,
            &["inspected", "--text", "a test never ends", "--left", "the test still hangs"],
        )
        .ok();
    assert_eq!((said["inspected"].as_bool(), said["found"].as_str()), (Some(true), Some("held")));
    assert_eq!(said["kind"], "worker");
    std::fs::write(checkout.join("found.md"), "cargo build of 900 crates\n").unwrap();
    let said = world
        .afk_in(
            &checkout,
            &[
                "inspected",
                "--text-file",
                "found.md",
                "--did",
                "ended pid 4242, an orphaned cargo build",
                "--did",
                "told the worker to build through afkconvoy exclusive",
            ],
        )
        .ok();
    assert_eq!(said["found"], "goes-on", "{said}");
    assert_eq!(said["did"].as_array().map(Vec::len), Some(2), "{said}");
    let kept = kept(&checkout);
    assert_eq!(kept["said"], "cargo build of 900 crates");
    assert_eq!(kept["did"][1], "told the worker to build through afkconvoy exclusive");
    assert!(kept.get("left").is_none(), "the last said stands: {kept}");
}

#[test]
fn a_host_inspection_says_what_it_did_in_the_same_words() {
    let world = World::new();
    let checkout = inspection_checkout(&world);
    let git_dir = Command::new("git")
        .current_dir(&checkout)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    std::fs::write(format!("{}/convoy-inspection-kind", git_dir.trim()), "host").unwrap();
    let said = world
        .afk_in(
            &checkout,
            &["inspected", "--text", "five worktrees of 14 GiB", "--did", "removed their target"],
        )
        .ok();
    assert_eq!((said["found"].as_str(), said["kind"].as_str()), (Some("goes-on"), Some("host")));
}

#[test]
fn only_an_inspection_s_checkout_says_what_was_done() {
    let world = World::new();
    let elsewhere = world.afk(&["inspected", "--text", "x"]).error();
    assert!(elsewhere.contains("is no inspection's checkout"), "{elsewhere}");
    let checkout = inspection_checkout(&world);
    let empty = world.afk_in(&checkout, &["inspected", "--text", " "]).error();
    assert!(empty.contains("the text is empty"), "{empty}");
    let did = world.afk_in(&checkout, &["inspected", "--text", "x", "--did", " "]).error();
    assert!(did.contains("a --did or a --left is empty"), "{did}");
    let found = world.afk_in(&checkout, &["inspected", "--found", "heavy", "--text", "x"]).error();
    assert!(found.contains("--found"), "no finding word is asked for any more: {found}");
}

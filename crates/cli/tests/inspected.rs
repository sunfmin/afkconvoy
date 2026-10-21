//! `afkconvoy inspected`: an inspection says what it found of a hot worker,
//! from the checkout it was started in, for its `run` to type and leave.

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

#[test]
fn an_inspection_keeps_what_it_found_in_its_checkout_and_the_last_said_stands() {
    let world = World::new();
    let checkout = inspection_checkout(&world);
    let said = world
        .afk_in(&checkout, &["inspected", "--found", "wrong", "--text", "a test never ends"])
        .ok();
    assert_eq!((said["inspected"].as_bool(), said["found"].as_str()), (Some(true), Some("wrong")));
    let file = std::fs::write(checkout.join("found.md"), "cargo build of 900 crates\n");
    file.unwrap();
    world.afk_in(&checkout, &["inspected", "--found", "heavy", "--text-file", "found.md"]).ok();
    let git_dir = Command::new("git")
        .current_dir(&checkout)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    let kept =
        std::fs::read_to_string(format!("{}/convoy-inspected.json", git_dir.trim())).unwrap();
    let kept: serde_json::Value = serde_json::from_str(&kept).unwrap();
    assert_eq!(kept["found"], "heavy");
    assert_eq!(kept["said"], "cargo build of 900 crates");
}

#[test]
fn only_an_inspection_s_checkout_says_what_was_found() {
    let world = World::new();
    let elsewhere = world.afk(&["inspected", "--found", "heavy", "--text", "x"]).error();
    assert!(elsewhere.contains("is no inspection's checkout"), "{elsewhere}");
    let checkout = inspection_checkout(&world);
    let empty = world.afk_in(&checkout, &["inspected", "--found", "heavy", "--text", " "]).error();
    assert!(empty.contains("the text is empty"), "{empty}");
    let unknown = world.afk_in(&checkout, &["inspected", "--found", "bad", "--text", "x"]).error();
    assert!(unknown.contains("wrong, heavy, not-its"), "{unknown}");
}

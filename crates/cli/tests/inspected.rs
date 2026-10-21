//! `afkconvoy inspected`: an inspection says what it found of a hot worker,
//! from the checkout it was started in, for its `run` to type and leave.

use crate::common;

use std::process::Command;

use common::World;

/// A checkout of `world`'s repo named as an inspection's is.
fn inspection_checkout(world: &World) -> std::path::PathBuf {
    inspection_checkout_named(world, "afkconvoy-inspection")
}

fn inspection_checkout_named(world: &World, name: &str) -> std::path::PathBuf {
    let checkout = world.root.join("wt").join(name);
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
fn a_host_inspection_finds_in_words_of_its_own() {
    let world = World::new();
    let checkout = inspection_checkout(&world);
    let git_dir = Command::new("git")
        .current_dir(&checkout)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    std::fs::write(format!("{}/convoy-inspection-kind", git_dir.trim()), "host").unwrap();
    let wrong = world.afk_in(&checkout, &["inspected", "--found", "wrong", "--text", "x"]).error();
    assert!(
        wrong.contains("an inspection of the host finds one of: reclaimable, fleet, not-its"),
        "{wrong}"
    );
    for found in ["reclaimable", "fleet", "not-its"] {
        let said = world
            .afk_in(&checkout, &["inspected", "--found", found, "--text", "five worktrees"])
            .ok();
        assert_eq!((said["found"].as_str(), said["kind"].as_str()), (Some(found), Some("host")));
    }
    // A hot worker's inspection is refused the host's words.
    let worker = inspection_checkout_named(&world, "afkconvoy-inspection-2");
    let fleet = world.afk_in(&worker, &["inspected", "--found", "fleet", "--text", "x"]).error();
    assert!(fleet.contains("an inspection of the worker finds one of: wrong, heavy"), "{fleet}");
}

/// An inspection checkout of `world`'s repo, started on the host.
fn host_checkout(world: &World) -> std::path::PathBuf {
    let checkout = inspection_checkout(world);
    let git_dir = Command::new("git")
        .current_dir(&checkout)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    std::fs::write(format!("{}/convoy-inspection-kind", git_dir.trim()), "host").unwrap();
    checkout
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ran = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(ran.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ran.stderr));
}

#[test]
fn a_host_inspection_frees_build_output_and_tool_caches_and_names_what_is_kept() {
    let world = World::new();
    let checkout = host_checkout(&world);
    let home = world.home();
    // A checkout under the home folder no process uses, with its build output.
    let other = home.join("src/gadgets");
    std::fs::create_dir_all(other.join("target/debug")).unwrap();
    git(&other, &["init", "-q"]);
    std::fs::write(other.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(other.join("lib.rs"), "fn main() {}\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"]);
    std::fs::write(other.join("draft.rs"), "// not committed\n").unwrap();
    std::fs::write(other.join("target/debug/big"), vec![1u8; 1 << 20]).unwrap();
    let cache = home.join("Library/Caches/Homebrew");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("bottle.tar.gz"), vec![1u8; 1 << 20]).unwrap();
    let weights = home.join(".cache/huggingface/hub");
    std::fs::create_dir_all(&weights).unwrap();
    std::fs::write(weights.join("model.safetensors"), vec![1u8; 1 << 20]).unwrap();
    let (other_s, cache_s, weights_s) =
        (other.to_str().unwrap(), cache.to_str().unwrap(), weights.to_str().unwrap());

    // Of a line any path of which may not be freed, nothing is.
    let kept = world
        .afk_in(
            &checkout,
            &[
                "inspected",
                "--found",
                "reclaimable",
                "--text",
                "x",
                "--reclaim",
                other_s,
                "--reclaim",
                weights_s,
            ],
        )
        .error();
    assert!(kept.contains("nothing was freed") && kept.contains("name it for a human"), "{kept}");
    assert!(other.join("target/debug/big").exists());
    let outside = world
        .afk_in(
            &checkout,
            &[
                "inspected",
                "--found",
                "reclaimable",
                "--text",
                "x",
                "--reclaim",
                world.clone.to_str().unwrap(),
            ],
        )
        .error();
    assert!(outside.contains("not under the home folder"), "{outside}");
    let fleet = world
        .afk_in(&checkout, &["inspected", "--found", "fleet", "--text", "x", "--reclaim", other_s])
        .error();
    assert!(fleet.contains("only what is reclaimable is freed"), "{fleet}");

    let said = world
        .afk_in(
            &checkout,
            &[
                "inspected",
                "--found",
                "reclaimable",
                "--text",
                "build output and a cache",
                "--reclaim",
                other_s,
                "--reclaim",
                cache_s,
                "--name",
                weights_s,
            ],
        )
        .ok();
    assert!(!other.join("target").exists(), "what git ignores is freed");
    assert!(other.join("lib.rs").exists() && other.join("draft.rs").exists(), "and nothing else");
    assert!(!cache.exists() && home.join("Library/Caches").exists());
    assert!(weights.join("model.safetensors").exists(), "what is named is never deleted");
    let freed = said["freed"].as_u64().unwrap();
    assert!(freed >= 2 << 20, "{said}");
    assert_eq!(said["reclaimed"].as_array().map(Vec::len), Some(2), "{said}");
    let named = &said["named"][0];
    assert!(named["path"].as_str().unwrap().ends_with(".cache/huggingface/hub"), "{said}");
    assert!(named["bytes"].as_u64().unwrap() >= 1 << 20, "{said}");
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

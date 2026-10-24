//! The checkout a command is about: named after it, or with `-C` where what
//! follows the command is its own, or else the one it is run in — any
//! directory of a checkout naming that checkout, and a worker's worktree
//! refused but for the commands a worker runs there.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{World, git};
use serde_json::json;

/// What the binary's `--help` of `command` prints.
fn text_of(command: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args([command, "--help"]).output();
    String::from_utf8(out.unwrap().stdout).unwrap()
}

/// The worktree of the worker dispatched on #3, as alpha.
fn worker_worktree(world: &World) -> PathBuf {
    world.issue(3, &["ready-for-agent"]);
    let on = ["dispatch", "--issue", "3", "--instance", "alpha", "--now", "1000"];
    assert_eq!(world.afk(&on).ok()["started"], true);
    let worktrees = world.worktrees();
    assert_eq!(worktrees.len(), 1, "{worktrees:?}");
    worktrees[0].clone()
}

fn path(dir: &Path) -> &str {
    dir.to_str().unwrap()
}

#[test]
fn a_command_run_outside_any_checkout_is_about_the_one_it_names() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let elsewhere = &world.root;
    let here = world.afk(&["config"]).ok();
    assert_eq!(world.afk_in(elsewhere, &["config", path(&world.clone)]).ok(), here);
    let plan = world.afk_in(elsewhere, &["plan", path(&world.clone), "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 7, "{plan}");
    // a path is taken from where the command is run
    assert_eq!(world.afk_in(elsewhere, &["config", "clone"]).ok(), here);
    // naming none, it is about the checkout it is run in: here, none
    let error = world.afk_in(elsewhere, &["config"]).error();
    assert!(error.ends_with("is not in a git checkout"), "{error}");
    // a checkout named that is none is refused, whatever the command needs
    let file = world.clone.join("docs/agents/afkconvoy.md");
    let named = ["config", path(&world.root), "--file", path(&file)];
    let error = world.afk_in(elsewhere, &named).error();
    assert!(error.ends_with("is not in a git checkout"), "{error}");
    // …and with none named, a command that needs none is run anywhere
    assert_eq!(world.afk_in(elsewhere, &["config", "--file", path(&file)]).ok(), here);
}

#[test]
fn a_directory_inside_a_checkout_names_the_checkout() {
    let world = World::new();
    let inside = world.clone.join("docs/agents");
    let here = world.afk(&["config"]).ok();
    assert_eq!(world.afk_in(&world.root, &["config", path(&inside)]).ok(), here);
    assert_eq!(world.afk_in(&inside, &["config"]).ok(), here);
    assert_eq!(world.afk_in(&inside, &["flows"]).ok(), world.afk(&["flows"]).ok());
}

#[test]
fn a_command_with_arguments_of_its_own_names_its_checkout_with_c() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let clone = path(&world.clone);
    let claim = ["claim", "-C", clone, "7", "--instance", "beta", "--now", "1000"];
    assert_eq!(world.afk_in(&world.root, &claim).ok()["won"], true);
    assert_eq!(world.claims(), [(7, "beta".to_string())]);
    // where what follows it is its own, a checkout is no argument of it
    let error = world.afk_in(&world.root, &["claim", clone, "7", "--instance", "beta"]).error();
    assert!(error.contains("unexpected argument"), "{error}");
    let inside = world.clone.join("docs");
    let release = ["release", "-C", path(&inside), "--issue", "7", "--instance", "beta"];
    assert_eq!(world.afk_in(&world.root, &release).ok()["released"], true);
    assert_eq!(world.claims(), []);
    // `exclusive` runs its command there
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root)
        .args(["exclusive", "-C", path(&inside), "--", "pwd"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), path(&inside));
}

#[test]
fn a_workers_worktree_is_refused_for_a_fleet_command_and_taken_for_a_workers() {
    let world = World::new();
    let worktree = worker_worktree(&world);
    let refused = |cwd: &Path, args: &[&str]| {
        let error = world.afk_in(cwd, args).error();
        assert!(error.contains("is a worker's worktree"), "{args:?}: {error}");
        assert!(error.contains(path(&world.clone)), "it names a checkout to use: {error}");
    };
    refused(&worktree, &["scan"]);
    refused(&world.root, &["scan", path(&worktree)]);
    refused(&worktree.join("docs"), &["config"]);
    refused(&world.root, &["park", "-C", path(&worktree), "3", "--instance", "alpha"]);
    // the commands a worker runs there are taken
    let woke = world.afk_in(&worktree, &["wake"]).ok();
    assert_eq!(woke["woke"], false, "{woke}");
    let plan = world.afk_in(&worktree, &["plan", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"], json!([]), "{plan}");
    // a human's own worktree of the repo is no worker's
    let own = world.root.join("own");
    git(&world.clone, &["worktree", "add", "-q", "-b", "own", path(&own)]);
    assert_eq!(world.afk_in(&own, &["scan"]).ok()["claims"][0]["number"], 3);
}

#[test]
fn each_command_about_one_repo_says_how_it_takes_its_checkout() {
    let heading = "##### Which checkout it is about";
    for (command, form, worker) in [
        ("plan", "`[CHECKOUT]`", true),
        ("scan", "`[CHECKOUT]`", false),
        ("verdict", "`-C <CHECKOUT>`", true),
        ("claim", "`-C <CHECKOUT>`", false),
    ] {
        let text = text_of(command);
        let said = text.split_once(heading).map(|(_, said)| said).unwrap_or_else(|| {
            panic!("`{command} --help` says nothing of its checkout: {text}");
        });
        let said = said.split("\n##### ").next().unwrap();
        assert!(said.contains(form), "{command}: {said}");
        assert!(said.contains("directory in one"), "{command}: {said}");
        let refused = said.contains("A worker's worktree is refused");
        assert_eq!(!refused, worker, "{command}: {said}");
    }
    for command in ["kit", "skill", "scaffold", "draft", "guard"] {
        assert!(!text_of(command).contains(heading), "{command} is about no repo");
    }
}

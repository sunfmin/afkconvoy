//! First-start decisions through the real binary, with no fixture supplying
//! an answer. A refused start must leave the entire world unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::common::{Answer, World};
use serde_json::{Value, json};

/// Ordinary worker worlds know their host's name. First-start scenarios
/// deliberately begin before any host settings have been saved.
fn fresh_host() -> World {
    let world = World::new();
    std::fs::remove_file(world.cache().join("afkconvoy/checkouts.json")).unwrap();
    world
}

fn run(world: &World, cwd: &Path, args: &[&str]) -> Answer {
    let out =
        world.command(env!("CARGO_BIN_EXE_afkconvoy"), cwd).arg("run").args(args).output().unwrap();
    Answer {
        json: serde_json::from_slice(&out.stdout).expect("exactly one JSON object"),
        code: out.status.code().unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// Every directory, file's bytes and symbolic link: catches even a new
/// empty cache directory, a probe note, or a label written by a refusal.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let (path, kind) = (entry.path(), entry.file_type().unwrap());
            let bytes = if kind.is_symlink() {
                std::fs::read_link(&path).unwrap().as_os_str().as_encoded_bytes().to_vec()
            } else if kind.is_dir() {
                visit(&path, files);
                Vec::new()
            } else {
                std::fs::read(&path).unwrap()
            };
            files.insert(path, bytes);
        }
    }
    let mut files = BTreeMap::new();
    visit(root, &mut files);
    files
}

fn refused(world: &World, cwd: &Path, args: &[&str], settings: &[&str]) -> Value {
    let before = snapshot(&world.root);
    let answer = run(world, cwd, args);
    assert_ne!(answer.code, 0, "{}", answer.json);
    assert_eq!(answer.stderr, "");
    assert_eq!(answer.json["see"], "afkconvoy help first-start");
    assert!(answer.json["error"].as_str().unwrap().contains("user"));
    let asks = answer.json["asks"].as_array().unwrap();
    let got: Vec<_> = asks.iter().map(|ask| ask["setting"].as_str().unwrap()).collect();
    assert_eq!(got, settings);
    assert_eq!(snapshot(&world.root), before, "a first-start refusal writes nothing");
    answer.json
}

#[test]
fn a_new_checkout_asks_for_its_tracker_before_any_startup_writes() {
    let world = fresh_host();
    // A missing kit and flags that would write settings are deliberate:
    // neither setup nor those writes may precede this refusal.
    std::fs::remove_file(world.kit_path()).unwrap();
    refused(
        &world,
        &world.clone,
        &["--hot-cores", "2", "--concurrency", "1", "--panel", "7123"],
        &["tracker", "watch-dir"],
    );
    world.say_host_settings(json!({}));
    refused(&world, &world.clone, &[], &["tracker", "concurrency"]);
    // A saved host does not waive the tracker choice for another checkout.
    let second = world.second_repo();
    refused(&world, &second, &[], &["tracker", "concurrency"]);
}

#[test]
fn a_new_host_asks_nothing_of_host_sleep() {
    let world = fresh_host();
    let all = ["tracker", "concurrency", "watch-dir"];
    refused(&world, &world.clone, &[], &all);
    refused(&world, &world.clone, &["--foreground"], &all);
    // Its tracker given, a new host starts: no fleet puts a host to sleep,
    // so there is nothing of it to settle (docs/adr/0050). Its panel is
    // served on any free port: 7070 is the panel of the host the tests run on.
    let first =
        run(&world, &world.clone, &["--tracker", "default", "--hot-memory", "2", "--panel", "0"])
            .ok();
    assert_eq!(first["added"], true);
    world.afk(&["stop", "--all"]).ok();
}

#[test]
fn explicit_choices_start_and_existing_checkouts_and_host_defaults_need_no_flags() {
    let world = World::new();
    let first = run(&world, &world.clone, &["--tracker", "default"]).ok();
    assert_eq!(first["added"], true);
    let again = run(&world, &world.clone, &[]).ok();
    assert_eq!(again["added"], false);
    assert_eq!(again["instance"], first["instance"]);
    world.afk(&["stop", "--all"]).ok();
    // An older host's saved settings need no acknowledgment or migration.
    world.say_host_settings(json!({"no_host_sleep": true}));
    let restarted = run(&world, &world.clone, &[]).ok();
    assert_eq!(restarted["added"], false);
    world.afk(&["stop", "--all"]).ok();
}

#[test]
fn every_first_start_question_and_option_is_shared_by_the_topic_and_json_asks() {
    let world = fresh_host();
    let all = ["tracker", "concurrency", "watch-dir"];
    let answer = refused(&world, &world.clone, &[], &all);
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["help", "first-start"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let topic = String::from_utf8(out.stdout).unwrap();
    for section in ["Tell the user", "Set up without asking", "Ask the user"] {
        assert!(topic.contains(&format!("## {section}\n")), "{topic}");
    }
    let questions = serde_json::to_value(afkconvoy::first_start::questions()).unwrap();
    let questions = questions.as_array().unwrap();
    assert_eq!(answer["asks"], json!(questions), "every shared question is in the refusal");
    let headings: BTreeSet<_> =
        topic.lines().filter_map(|line| line.strip_prefix("### ")).collect();
    let settings: BTreeSet<_> = questions.iter().map(|q| q["setting"].as_str().unwrap()).collect();
    assert_eq!(headings, settings, "no question appears in only one interface");
    assert!(settings.contains("watch-dir"), "the operator's directory is a first-start question");
    for question in questions {
        for key in ["question", "default", "recommend"] {
            assert!(topic.contains(question[key].as_str().unwrap()), "{key}: {question}");
        }
        for option in question["options"].as_array().unwrap() {
            for key in ["option", "line"] {
                assert!(topic.contains(option[key].as_str().unwrap()), "{key}: {option}");
            }
        }
    }
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    let needs = help
        .split("## What it needs")
        .nth(1)
        .unwrap()
        .split("## What to run first")
        .next()
        .unwrap();
    assert!(needs.contains("tracker") && needs.contains("Host settings"));
    let first = help
        .split("## What to run first")
        .nth(1)
        .unwrap()
        .split("\n1. ")
        .nth(1)
        .unwrap()
        .split("\n2. ")
        .next()
        .unwrap();
    assert!(first.contains("afkconvoy checkouts") && first.contains("afkconvoy help first-start"));
}

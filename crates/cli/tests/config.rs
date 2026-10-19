use crate::common;

use std::collections::BTreeSet;

use common::World;

fn write_config(world: &World, yaml: &str) {
    let dir = world.clone.join("docs/agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("afkconvoy.md"), format!("# config\n\n```yaml\n{yaml}\n```\n"))
        .unwrap();
}

#[test]
fn config_prints_every_key_with_defaults_filled() {
    let world = World::new();
    write_config(&world, "base_branch: main\ngate_command: make test\nconcurrency: 5");
    let config = world.afk(&["config"]).ok();
    assert_eq!(
        config,
        serde_json::json!({
            "base_branch": "main",
            "gate_command": "make test",
            "gate_seconds": 3600,
            "ready_label": "ready-for-agent",
            "epic_labels": ["epic", "prd"],
            "concurrency": 5,
            "escalate_label": "ready-for-human",
            "lock_label": "fleet-lock",
        })
    );
}

#[test]
fn config_is_found_from_anywhere_in_the_checkout() {
    let world = World::new();
    write_config(&world, "base_branch: main\ngate_command: make test");
    let config = world.afk_in(&world.clone.join("docs"), &["config"]).ok();
    assert_eq!(config["base_branch"], "main");
}

#[test]
fn a_bad_config_is_an_error_naming_the_key() {
    let world = World::new();
    for (yaml, key) in [
        ("gate_command: make", "base_branch"),
        ("base_branch: main", "gate_command"),
        ("base_branch: main\ngate_command: make\nretry: 2", "retry"),
        ("base_branch: main\ngate_command: make\nconcurrency: lots", "concurrency"),
        ("base_branch: main\ngate_command: make\nconcurrency: 0", "concurrency"),
        ("base_branch: main\ngate_command: make\ngate_seconds: 0", "gate_seconds"),
        ("base_branch: main\ngate_command: make\ngate_seconds: 1.5", "gate_seconds"),
        ("base_branch: main\ngate_command: make\ngate_seconds: -1", "gate_seconds"),
        ("base_branch: main\ngate_command: make\ngate_seconds: an hour", "gate_seconds"),
        (
            "base_branch: main\ngate_command: make\nescalate_label: ready-for-agent",
            "escalate_label",
        ),
    ] {
        write_config(&world, yaml);
        let error = world.afk(&["config"]).error();
        assert!(error.contains(key), "{yaml:?} → {error}");
    }
}

#[test]
fn a_failure_is_one_json_object_and_a_non_zero_exit() {
    let world = World::new();
    std::fs::remove_file(world.clone.join("docs/agents/afkconvoy.md")).unwrap();
    assert!(world.afk(&["config"]).error().contains("cannot read the config file"));
    assert!(world.afk(&["no-such-command"]).error().contains("no-such-command"));
    assert!(world.afk(&["config", "--file", "/nowhere.md"]).error().contains("/nowhere.md"));
}

#[test]
fn this_repos_own_config_names_the_gate() {
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/agents/afkconvoy.md");
    let world = World::new();
    let config = world.afk(&["config", "--file", file]).ok();
    assert_eq!(config["gate_command"], "./gate.sh");
}

/// The config file `config --help` shows whole: its ```` ```yaml ```` block,
/// as the yaml a repo would write.
fn shown_config() -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_afkconvoy"))
        .args(["config", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    let block = help.lines().skip_while(|line| line.trim() != "```yaml").skip(1);
    let block: Vec<&str> = block.take_while(|line| line.trim() != "```").map(str::trim).collect();
    assert!(!block.is_empty(), "`config --help` shows no config file");
    block.join("\n")
}

/// The file the help shows is one the loader reads, holds every key the
/// loader knows, and gives each key that has a default that default.
#[test]
fn the_help_shows_a_config_file_whole_with_every_key_at_its_default() {
    let world = World::new();
    let shown = shown_config();
    write_config(&world, &shown);
    let whole = world.afk(&["config"]).ok();
    // Every key: the answer of a bound repo has no key the file leaves out.
    let key = |line: &str| line.split_once(':').unwrap().0.to_string();
    let written: BTreeSet<String> = shown.lines().map(key).collect();
    let answered: BTreeSet<String> = whole.as_object().unwrap().keys().cloned().collect();
    assert_eq!(written, answered);
    // At its default: the keys with none, alone, answer the same.
    let required =
        |line: &&str| ["base_branch", "gate_command", "team"].contains(&key(line).as_str());
    let shortest: Vec<&str> = shown.lines().filter(required).collect();
    assert_eq!(shortest.len(), 3);
    write_config(&world, &shortest.join("\n"));
    assert_eq!(world.afk(&["config"]).ok(), whole);
}

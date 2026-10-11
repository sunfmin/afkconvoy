mod common;

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
            "ready_label": "ready-for-agent",
            "epic_labels": ["epic", "prd"],
            "concurrency": 5,
            "escalate_label": "ready-for-human",
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

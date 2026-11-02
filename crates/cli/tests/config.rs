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
    write_config(&world, "base_branch: main\ngate_command: make test");
    let config = world.afk(&["config"]).ok();
    let every = serde_json::json!({
        "base_branch": "main",
        "gate_command": "make test",
        "gate_seconds": 3600,
        "ready_label": "ready-for-agent",
        "epic_labels": ["epic", "prd"],
        "escalate_label": "ready-for-human",
        "lock_label": "fleet-lock",
            "escalate_to": "human",
    });
    assert_eq!(config, every);
    // A concurrency is its checkout's, said on its host: one the config
    // still names is read, and changes nothing.
    write_config(&world, "base_branch: main\ngate_command: make test\nconcurrency: 5");
    assert_eq!(world.afk(&["config"]).ok(), every);
}

#[test]
fn config_is_found_from_anywhere_in_the_checkout() {
    let world = World::new();
    write_config(&world, "base_branch: main\ngate_command: make test");
    let config = world.afk_in(&world.clone.join("docs"), &["config"]).ok();
    assert_eq!(config["base_branch"], "main");
}

#[test]
fn config_errors_wire_yaml_parsing_and_domain_validation_to_json() {
    let world = World::new();
    // One required key, unknown field, YAML type and domain boundary. The
    // full rule tables live in core::config and src/config.rs's parser tests.
    for (yaml, key) in [
        ("gate_command: make", "base_branch"),
        ("base_branch: main\ngate_command: make\nretry: 2", "retry"),
        ("base_branch: main\ngate_command: make\ngate_seconds: 0", "gate_seconds"),
        ("base_branch: main\ngate_command: make\ngate_seconds: 1.5", "gate_seconds"),
        ("base_branch: main\ngate_command: make\ngate_budget_seconds: 0", "gate_budget_seconds"),
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
    assert_eq!(config["gate_command"], "afkconvoy exclusive -- ./gate.sh");
    assert_eq!(config["cut"]["busy_after_seconds"], 18_000);
    let flux = &config["gate_needs"][0];
    assert_eq!(flux["name"], "flux-toolchain");
    assert_eq!(flux["install"], "./verify.sh install");
    assert_eq!(flux["install_seconds"], 5400);
}

#[test]
fn a_warning_budget_has_one_home_and_old_readers_keep_the_termination_limit() {
    let world = World::new();
    write_config(&world, "base_branch: main\ngate_command: make test\ngate_seconds: 3600");
    let file = world.clone.join("docs/agents/afkconvoy.md");
    let original = std::fs::read_to_string(&file).unwrap();
    let budget = "\n```yaml gate-budget\ngate_budget_seconds: 600\n```\n";
    let declared = format!("{original}{budget}");
    std::fs::write(&file, &declared).unwrap();
    let config = world.afk(&["config"]).ok();
    assert_eq!(config["gate_budget_seconds"], 600);
    assert_eq!(config["gate_seconds"], 3600);
    // The first block is precisely what older released versions read.
    let first = afkconvoy::config::yaml_block(&declared).unwrap();
    assert!(!first.contains("gate_budget_seconds"));
    let older: serde_json::Value = serde_yaml::from_str(&first).unwrap();
    assert_eq!(older["gate_seconds"], 3600);
    for duplicate in [
        format!("{declared}{budget}"),
        declared.replace("gate_seconds: 3600", "gate_seconds: 3600\ngate_budget_seconds: 600"),
    ] {
        std::fs::write(&file, duplicate).unwrap();
        let error = world.afk(&["config"]).error();
        assert!(
            error.contains("duplicate field") && error.contains("gate_budget_seconds"),
            "{error}"
        );
    }
    for invalid in ["0", "3600", "-1", "1.5", "'600'"] {
        std::fs::write(
            &file,
            declared
                .replace("gate_budget_seconds: 600", &format!("gate_budget_seconds: {invalid}")),
        )
        .unwrap();
        assert!(world.afk(&["config"]).error().contains("gate_budget_seconds"));
    }
    std::fs::write(&file, declared.replace("gate_budget_seconds: 600", "gate_buget_seconds: 600"))
        .unwrap();
    assert!(world.afk(&["config"]).error().contains("unknown field"));
}

/// The config file `config --help` shows whole: the ```` ```yaml ```` block of
/// the one file it shows, as the yaml a repo would write.
fn shown_config() -> String {
    let files = common::shown("`afkconvoy config`", "markdown");
    assert_eq!(files.len(), 1, "`config --help` shows one config file: {files:?}");
    let yaml = files[0].split_once("```yaml\n").map(|(_, yaml)| yaml);
    let yaml = yaml.and_then(|yaml| yaml.split_once("```")).map(|(yaml, _)| yaml);
    yaml.unwrap_or_else(|| panic!("the config file shows no yaml block: {files:?}"))
        .trim_end()
        .into()
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

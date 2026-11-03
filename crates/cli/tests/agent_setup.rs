//! The agent setup (sunfmin/control#528): the operator's instructions and
//! skills, declared by the host settings, given to every harness the host kit
//! holds — by `kit`, by a tick to a harness held since — and checked by
//! `doctor`.
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::common::World;

const TICK: [&str; 5] = ["tick", "--instance", "alpha", "--host", "h1"];

/// A world whose host declares an agent setup: its instructions file, and a
/// skill list of one `*` source and one of named skills.
fn declared() -> (World, PathBuf) {
    let world = World::new();
    let dotfiles = world.root.join("dotfiles");
    std::fs::create_dir_all(dotfiles.join("claude")).unwrap();
    let instructions = dotfiles.join("claude/CLAUDE.md");
    std::fs::write(&instructions, "# the operator's principles\n").unwrap();
    let skills = dotfiles.join("skills.txt");
    std::fs::write(
        &skills,
        "# every machine's skills\nmattpocock/skills *\nsunfmin/mypython mypython\n",
    )
    .unwrap();
    world.say_host_settings(json!({"agent_instructions": instructions, "agent_skills": skills}));
    (world, instructions)
}

fn npx_asked(world: &World) -> Vec<String> {
    let log = std::fs::read_to_string(world.root.join("npx-asked")).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

fn installed(world: &World, agent: &str, skills: Value) {
    std::fs::write(world.root.join(format!("npx-ls-{agent}.json")), skills.to_string()).unwrap();
}

fn drifted(doctor: &Value) -> Vec<Value> {
    let findings = doctor["findings"].as_array().unwrap();
    let of = |finding: &&Value| {
        finding["what"].as_str().unwrap().contains("differs from the agent setup")
    };
    findings.iter().filter(of).cloned().collect()
}

#[test]
fn a_tick_gives_a_harness_held_since_the_setup_and_doctor_names_each_that_drifted() {
    let (world, instructions) = declared();
    // Before anything gave it, each held harness differs: no link, no skill.
    let doctor = world.afk(&["doctor", "--all"]).ok();
    let found = drifted(&doctor);
    assert_eq!(found.len(), 1, "the host kit holds Claude Code alone: {doctor}");
    assert!(found[0]["what"].as_str().unwrap().contains("`claude-code`"), "{found:?}");
    let evidence = found[0]["evidence"].as_str().unwrap();
    assert!(evidence.contains("~/.claude/CLAUDE.md is missing"), "{evidence}");
    assert!(evidence.contains("missing skills: every skill of mattpocock/skills, mypython"));
    assert_eq!(found[0]["fix"], "afkconvoy kit");

    // What `npx skills` will track for Claude Code once installed, and a
    // skill of the user's own it does not.
    let fake = json!({"name": "fake", "source": "mattpocock/skills", "agents": ["Claude Code"]});
    installed(&world, "claude-code", json!([fake]));
    std::fs::create_dir_all(world.home().join(".claude/skills/mine")).unwrap();

    // A tick gives it to the harness held, with no step by the operator.
    world.afk(&TICK).ok();
    let linked = world.home().join(".claude/CLAUDE.md");
    assert_eq!(
        std::fs::canonicalize(&linked).unwrap(),
        std::fs::canonicalize(&instructions).unwrap()
    );
    let asked = npx_asked(&world);
    assert!(
        asked.contains(
            &"npx -y skills add 'mattpocock/skills' -g -y -a 'claude-code' -s '*'".to_string()
        ),
        "{asked:?}"
    );
    assert!(asked.contains(
        &"npx -y skills add 'sunfmin/mypython' -g -y -a 'claude-code' -s 'mypython'".to_string()
    ));
    // The copies are read-only once the sync is done: the shared one, and
    // the one of Claude Code's own that `npx skills` tracks; a skill of the
    // user's own beside it is left as it is.
    for copy in [".agents/skills/fake/SKILL.md", ".claude/skills/fake/SKILL.md"] {
        let copy = world.home().join(copy);
        let mode = std::fs::metadata(&copy).unwrap().permissions().mode();
        assert_eq!(mode & 0o222, 0, "{} is writable: {mode:o}", copy.display());
        assert!(std::fs::write(&copy, "edited in place\n").is_err());
    }
    std::fs::write(world.home().join(".claude/skills/mine/SKILL.md"), "still mine\n").unwrap();

    // A later tick gives nothing again to a harness given it.
    let before = npx_asked(&world).len();
    world.afk(&TICK).ok();
    assert_eq!(npx_asked(&world).len(), before, "given already");

    // A harness held since gets it at the next tick: Codex, here.
    world.kit_command("codex", afkconvoy_core::harness::Harness::Codex.stock());
    world.afk(&TICK).ok();
    let codex = world.home().join(".codex/AGENTS.md");
    assert_eq!(
        std::fs::canonicalize(&codex).unwrap(),
        std::fs::canonicalize(&instructions).unwrap()
    );
    let asked = npx_asked(&world);
    assert!(
        asked[before..]
            .contains(&"npx -y skills add 'mattpocock/skills' -g -y -a 'codex' -s '*'".to_string()),
        "a sync opens the read-only copies for its own writes: {asked:?}"
    );
    assert!(!asked[before..].iter().any(|line| line.contains("'claude-code'")), "{asked:?}");

    // Installed as declared, Claude Code is in step; Codex holds one the list
    // does not name, and a skill npx does not track is no one's.
    let skill = |name: &str, source: Option<&str>, agent: &str| json!({"name": name, "source": source, "agents": [agent], "status": "tracked"});
    installed(
        &world,
        "claude-code",
        json!([
            skill("tdd", Some("mattpocock/skills"), "Claude Code"),
            skill("mypython", Some("sunfmin/mypython"), "Claude Code"),
            skill("handmade", None, "Claude Code"),
        ]),
    );
    installed(
        &world,
        "codex",
        json!([
            skill("tdd", Some("mattpocock/skills"), "Codex"),
            skill("mypython", Some("sunfmin/mypython"), "Codex"),
            skill("stray", Some("someone/else"), "Codex"),
        ]),
    );
    let doctor = world.afk(&["doctor", "--all"]).ok();
    let found = drifted(&doctor);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0]["what"].as_str().unwrap().contains("`codex`"));
    assert_eq!(found[0]["evidence"], "skills the list does not name: stray");

    // Writing the host kit gives it to every held harness, and removes it.
    std::fs::remove_file(&linked).unwrap();
    std::fs::write(&linked, "a file of the user's own\n").unwrap();
    // Each listed here; the rest are not found.
    world.shell_claude_models(&["opus"]);
    world.shell_server_models(&["gpt-5.5"]);
    for program in ["pi", "dsh-tui", "kimi", "grok"] {
        world.shell_knows(program, None);
    }
    let written = world.afk(&["kit"]).ok();
    assert_eq!(written["agent_setup"]["harnesses"], json!(["claude-code", "codex"]), "{written}");
    assert_eq!(
        std::fs::read_to_string(world.home().join(".claude/CLAUDE.md.before-agent-setup")).unwrap(),
        "a file of the user's own\n",
        "what stood there is kept aside"
    );
    assert!(std::fs::symlink_metadata(&linked).unwrap().file_type().is_symlink());
    let asked = npx_asked(&world);
    assert!(
        asked.contains(&"npx -y skills remove -g -y -a 'codex' -s 'stray'".to_string()),
        "{asked:?}"
    );
}

#[test]
fn with_no_agent_setup_declared_nothing_is_given_and_nothing_is_reported() {
    let world = World::new();
    world.afk(&TICK).ok();
    let written = world.afk(&["kit"]).ok();
    assert_eq!(written["agent_setup"], Value::Null);
    assert!(npx_asked(&world).is_empty());
    assert!(std::fs::symlink_metadata(world.home().join(".claude/CLAUDE.md")).is_err());
    let doctor = world.afk(&["doctor", "--all"]).ok();
    assert!(drifted(&doctor).is_empty(), "{doctor}");
}

#[test]
fn a_run_says_where_the_agent_setup_is_declared_and_default_removes_it() {
    let world = World::new();
    let file = world.root.join("skills.txt");
    std::fs::write(&file, "sunfmin/mypython mypython\n").unwrap();
    let missing = world.afk(&["run", "--agent-skills", "/no/such/file"]).error();
    assert!(missing.contains("is no file"), "{missing}");
    // The setting is kept before the checkout is handed over, whether or not
    // this host can work a fleet yet.
    let said = world.afk(&["run", "--agent-skills", file.to_str().unwrap()]).json;
    assert_eq!(
        said["settings"]["agent_skills"]["to"],
        json!(std::fs::canonicalize(&file).unwrap())
    );
    assert_eq!(said["agent_setup"]["harnesses"], json!(["claude-code"]), "{said}");
    assert_eq!(
        world.checkout_list()["settings"]["agent_skills"],
        json!(std::fs::canonicalize(&file).unwrap())
    );
    let _ = world.afk(&["run", "--agent-skills", "default"]);
    assert_eq!(world.checkout_list()["settings"]["agent_skills"], Value::Null);
    let _ = world.afk(&["stop", "--all"]);
}

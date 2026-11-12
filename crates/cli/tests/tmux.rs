//! The tmux of a Guild machine (sunfmin/control#753): writing the host kit
//! installs it where a standard account can and leaves one already there
//! alone, and `doctor` names a Guild machine with none, with the line that
//! fixes it.
use std::os::unix::fs::PermissionsExt;

use serde_json::Value;

use crate::common::World;

/// A world whose host is a Guild machine: its user has Guild's home.
fn guild() -> World {
    let world = World::new();
    std::fs::create_dir_all(world.home().join(".guild")).unwrap();
    world
}

/// Puts an executable `program` under the world's home at `dir`.
fn put(world: &World, dir: &str, program: &str) {
    let dir = world.home().join(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(program);
    std::fs::write(&file, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn asked(world: &World) -> Vec<String> {
    let log = std::fs::read_to_string(world.root.join("shell-asked")).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

fn installs(world: &World) -> Vec<String> {
    asked(world).into_iter().filter(|line| line.ends_with("install tmux")).collect()
}

fn missing(doctor: &Value) -> Vec<Value> {
    let findings = doctor["findings"].as_array().unwrap();
    let of = |finding: &&Value| finding["kind"] == "tmux_missing";
    findings.iter().filter(of).cloned().collect()
}

#[test]
fn the_host_kit_installs_tmux_with_pixi_and_doctor_names_a_guild_machine_until_then() {
    let world = guild();
    put(&world, ".pixi/bin", "pixi");

    let found = missing(&world.afk(&["doctor", "--all"]).ok());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0]["what"].as_str().unwrap().contains("Guild machine has no tmux"));
    assert_eq!(found[0]["fix"], "afkconvoy kit");

    let written = world.afk(&["kit"]).ok();
    let pixi = world.home().join(".pixi/bin/pixi");
    let line = format!("'{}' global install tmux", pixi.display());
    assert_eq!(written["tmux"]["by"], line, "{written}");
    let tmux = world.home().join(".pixi/bin/tmux");
    assert_eq!(written["tmux"]["installed"], tmux.display().to_string());
    assert_eq!(installs(&world), vec![line]);

    assert!(missing(&world.afk(&["doctor", "--all"]).ok()).is_empty());
    // The tmux there is left alone: writing the kit again installs nothing.
    let again = world.afk(&["kit"]).ok();
    assert!(again["tmux"].is_null(), "{again}");
    assert_eq!(installs(&world).len(), 1);
}

#[test]
fn an_existing_tmux_is_left_alone_and_no_other_host_is_asked() {
    let world = guild();
    put(&world, ".pixi/bin", "pixi");
    std::fs::write(world.root.join("shell-tmux"), "/opt/homebrew/bin/tmux\n").unwrap();
    assert!(world.afk(&["kit"]).ok()["tmux"].is_null());
    assert!(missing(&world.afk(&["doctor", "--all"]).ok()).is_empty());
    assert!(installs(&world).is_empty());

    // A host that is no Guild machine has nothing of tmux asked or said.
    let other = World::new();
    assert!(other.afk(&["kit"]).ok()["tmux"].is_null());
    assert!(missing(&other.afk(&["doctor", "--all"]).ok()).is_empty());
    assert!(asked(&other).iter().all(|line| !line.ends_with(" tmux")), "{:?}", asked(&other));
}

#[test]
fn homebrew_installs_tmux_only_at_a_prefix_with_bottles() {
    let world = guild();
    std::fs::write(world.root.join("shell-brew"), "/Users/h/homebrew/bin/brew\n").unwrap();
    // ~/homebrew would build tmux's ncurses and openssl: it is not used,
    // and the fix installs pixi first.
    let written = world.afk(&["kit"]).ok();
    assert!(written["tmux"]["error"].as_str().unwrap().contains("pixi"), "{written}");
    assert!(installs(&world).is_empty());
    let found = missing(&world.afk(&["doctor", "--all"]).ok());
    assert_eq!(found[0]["fix"], "curl -fsSL https://pixi.sh/install.sh | sh && afkconvoy kit");

    std::fs::write(world.root.join("shell-brew"), "/opt/homebrew/bin/brew\n").unwrap();
    std::fs::write(world.root.join("brew-prefix"), "/opt/homebrew\n").unwrap();
    let written = world.afk(&["kit"]).ok();
    assert_eq!(written["tmux"]["by"], "'/opt/homebrew/bin/brew' install tmux", "{written}");
    assert_eq!(written["tmux"]["installed"], "/opt/homebrew/bin/tmux");
}

#[test]
fn a_failed_install_fails_no_write_of_the_host_kit() {
    let world = guild();
    put(&world, ".pixi/bin", "pixi");
    std::fs::write(world.root.join("tmux-install-fails"), "").unwrap();
    let written = world.afk(&["kit"]).ok();
    assert_eq!(written["written"], true);
    assert!(written["tmux"]["error"].as_str().unwrap().contains("could not solve tmux"));
    assert_eq!(written["tmux"]["fix"], "afkconvoy kit");
    assert_eq!(missing(&world.afk(&["doctor", "--all"]).ok()).len(), 1);
}

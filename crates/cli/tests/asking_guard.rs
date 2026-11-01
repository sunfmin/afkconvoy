//! Execute the generated launches with a recording harness, then ask the exact
//! hook they installed. No model, real account, or operator Terminal is used.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

use afkconvoy_core::harness::Harness;
use serde_json::{Value, json};

use crate::common::{World, write};

pub(super) fn prepare(world: &World, harness: Harness) {
    world.shell_help(Some(&harness.flags().join(" ")));
    write(
        &world.root,
        "capture",
        r#"#!/bin/sh
printf '%s\n' "$@" > "$CAPTURE/argv"
printf '%s' "${AFKCONVOY_GUARD-}" > "$CAPTURE/guard"
printf '%s' "${KIMI_CODE_HOME-}${GROK_HOME-}" > "$CAPTURE/config-home"
printf '%s' "${AFKCONVOY_HARNESS-}" > "$CAPTURE/harness"
"#,
    );
    std::fs::set_permissions(world.root.join("capture"), std::fs::Permissions::from_mode(0o755))
        .unwrap();
    world.with_kit(|kit| {
        kit["harnesses"] = json!([{"harness":harness.key(), "command":world.root.join("capture")}]);
    });
    if harness == Harness::Codex {
        world.kit_models(harness.key(), &["gpt-x"]);
    }
}

pub(super) fn check_launch(
    world: &World,
    harness: Harness,
    dir: &Path,
    line: &str,
    outside_role: bool,
) {
    let capture = world.root.join("captured");
    std::fs::create_dir_all(&capture).unwrap();
    let out = world
        .command("/bin/sh", dir)
        .args(["-c", line])
        .env("CAPTURE", &capture)
        .env("ORCA_TERMINAL_HANDLE", "worker")
        .output()
        .unwrap();
    assert!(out.status.success(), "{harness:?}: {}\n{line}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(capture.join("harness")).unwrap(), harness.key());
    let argv = std::fs::read_to_string(capture.join("argv")).unwrap();
    let args: Vec<_> = argv.lines().collect();
    let guard = match harness {
        Harness::ClaudeCode => {
            let settings = args.windows(2).rev().find(|pair| pair[0] == "--settings").unwrap()[1];
            let value: Value = serde_json::from_str(settings).unwrap();
            value["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().to_string()
        }
        Harness::Codex => {
            let config = args.iter().find(|arg| arg.starts_with("hooks=")).unwrap();
            let value: toml::Value = toml::from_str(config).unwrap();
            value["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().to_string()
        }
        Harness::Pi | Harness::Dsh => {
            let flag = if harness == Harness::Pi { "--extension" } else { "--patch" };
            assert!(args.contains(&flag), "{argv}");
            std::fs::read_to_string(capture.join("guard")).unwrap()
        }
        Harness::Kimi | Harness::Grok => {
            let home = std::fs::read_to_string(capture.join("config-home")).unwrap();
            let config = std::fs::read_to_string(Path::new(&home).join("config.toml")).unwrap();
            let value: toml::Value = toml::from_str(&config).unwrap();
            if harness == Harness::Kimi {
                value["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|hook| hook["event"].as_str() == Some("PreToolUse"))
                    .unwrap()["command"]
                    .as_str()
                    .unwrap()
                    .to_string()
            } else {
                value["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().to_string()
            }
        }
    };
    assert_eq!(guard.contains("--terminal-input-only"), outside_role, "{guard}");
    write(&world.cache(), "afkconvoy/asking/terminal.json", &json!({
        "session": {"id":"asking", "directory":dir, "stock":false, "harness":"codex", "model":null,
            "terminal":"asks", "transcript":dir.join("asking.log"), "session_started":1},
        "status":"running", "question":null, "terminal":null, "problem":null, "command":""
    }).to_string());
    for (terminal, expected) in [("asks", 2), ("other", 0)] {
        let line = format!("orca terminal send --terminal {terminal} --text yes --enter");
        let described = match harness {
            Harness::Pi => json!({"toolName":"bash", "input":{"command":line}}),
            Harness::Dsh => json!({"name":"bash", "arguments":{"command":line}}),
            Harness::Grok => {
                json!({"toolName":"run_terminal_command", "toolInput":{"command":line}})
            }
            _ => json!({"tool_name":"Bash", "tool_input":{"command":line}}),
        };
        let mut child = world
            .command("/bin/sh", dir)
            .args(["-c", &guard])
            .env("AFKCONVOY_HARNESS", harness.key())
            .env("ORCA_TERMINAL_HANDLE", "worker")
            .envs([
                ("AFKCONVOY_STAGE", "build"),
                ("AFKCONVOY_WRITES", "true"),
                ("AFKCONVOY_CONSULT", "false"),
                ("AFKCONVOY_BASE_BRANCH", "main"),
                ("AFKCONVOY_ESCALATE_LABEL", "stuck"),
                ("AFKCONVOY_LOCK_LABEL", "halt"),
            ])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(described.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{harness:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn worker(harness: Harness, bound: bool) {
    let world = World::new();
    prepare(&world, harness);
    if bound {
        world.repo(
            "acme/dev",
            &[
                ("CLAUDE.md", "Build the change"),
                (".claude/skills/build/SKILL.md", "---\nname: build\ndescription: Build\n---\n"),
            ],
        );
        world.repo("acme/team", &[("team.md", &format!("```yaml\nroles:\n  dev: acme/dev\nroster:\n  dev: {{ run: [{{ harness: {} }}] }}\n```\n", harness.key())),
            ("workflows/feature.md", "```yaml\nstages:\n  - name: build\n    by: dev\n    do: build\n    on: { pass: done }\n```\n")]);
        crate::common::commit(
            &world.clone,
            "docs/agents/afkconvoy.md",
            "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n",
        );
    }
    world.issue(7, &[if bound { "flow:feature" } else { "ready-for-agent" }]);
    let started =
        world.afk(&["dispatch", "7", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok();
    let dir = Path::new(started["worktree"].as_str().unwrap());
    let orca = world.orca();
    let terminal = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|terminal| terminal["worktreePath"] == started["worktree"])
        .unwrap();
    check_launch(&world, harness, dir, terminal["command"].as_str().unwrap(), !bound);
}

macro_rules! workers {
    ($($name:ident, $bound:ident: $harness:ident),* $(,)?) => { $(
        #[test]
        fn $name() { worker(Harness::$harness, false); }
        #[test]
        fn $bound() { worker(Harness::$harness, true); }
    )* };
}

workers! {
    unbound_claude, bound_claude: ClaudeCode, unbound_pi, bound_pi: Pi, unbound_codex, bound_codex: Codex,
    unbound_dsh, bound_dsh: Dsh, unbound_kimi, bound_kimi: Kimi, unbound_grok, bound_grok: Grok,
}

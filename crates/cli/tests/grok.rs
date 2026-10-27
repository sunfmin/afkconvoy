//! Grok is a harness: a bound worker starts in its role, under a generated
//! home that holds the role's limits, the fleet's guard and the report.
//! The role's rules and skills stay where every harness writes them.

use crate::common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::{World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  auditor: acme/auditor\n\
roster:\n  dev: { run: [{ harness: grok, model: grok-4.7 }] }\n\
\x20 auditor: { run: [{ harness: grok }] }\n```\n";
const FEATURE: &str = "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    on: { pass: done }\n```\n";
const AUDIT: &str = "```yaml\nstages:\n  - name: look\n    by: auditor\n    do: review\n    writes: false\n    on: { pass: done }\n```\n";
const CONFIG: &str = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
const LIMITS: &str = "[permission]\ndeny = [\"Bash(rm -rf *)\"]\n\
[[hooks.PreToolUse]]\nmatcher = \"Write\"\n\
hooks = [{ type = \"command\", command = \"role-limit\", timeout = 5 }]\n";

fn bound_with(team: &str) -> World {
    let world = World::new();
    world.hold_every_harness();
    world.kit_models("grok", &["grok-4.7"]);
    world.with_kit(|kit| {
        let entries = kit["harnesses"].as_array_mut().unwrap();
        let at = entries.iter().position(|held| held["harness"] == "grok").unwrap();
        let held = entries.remove(at);
        entries.insert(0, held);
    });
    world.repo(
        "acme/dev",
        &[
            ("CLAUDE.md", "the rules of a dev\n"),
            (
                ".claude/skills/implement/SKILL.md",
                "---\nname: implement\ndescription: build\n---\n",
            ),
            (".grok/config.toml", LIMITS),
        ],
    );
    world.repo(
        "acme/auditor",
        &[
            ("CLAUDE.md", "the rules of an auditor\n"),
            (".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n"),
        ],
    );
    world.repo(
        "acme/team",
        &[
            ("team.md", team),
            ("CLAUDE.md", "the rules of the team\n"),
            ("workflows/feature.md", FEATURE),
            ("workflows/audit.md", AUDIT),
        ],
    );
    write(&world.clone, "docs/agents/afkconvoy.md", CONFIG);
    write(&world.clone, "AGENTS.md", "the rules of the project\n");
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

fn tick(world: &World) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok()
}

fn head(world: &World, repo: &str) -> String {
    git(&world.root.join("github").join(repo), &["rev-parse", "HEAD"])
}

fn role_copy(world: &World, repo: &str) -> PathBuf {
    world.cache().join("afkconvoy/copies").join(repo).join(head(world, repo))
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let row = orca["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|row| row["linkedIssue"] == issue)
        .unwrap();
    PathBuf::from(row["path"].as_str().unwrap())
}

fn started(world: &World, dir: &Path) -> String {
    let orca = world.orca();
    let open: Vec<&Value> = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|terminal| {
            terminal["worktreePath"] == dir.to_str().unwrap() && terminal["open"] == true
        })
        .collect();
    assert_eq!(open.len(), 1, "{open:?}");
    open[0]["command"].as_str().unwrap().to_string()
}

fn kept_from(line: &str) -> PathBuf {
    let (_, after) = line.split_once("GROK_HOME='").unwrap();
    PathBuf::from(after.split('\'').next().unwrap())
}

#[test]
fn a_grok_worker_gets_an_isolated_role_home_and_keeps_the_role_s_own_files() {
    let world = bound_with(&TEAM.replace("```\n", "host_user: false\n```\n"));
    write(&world.home(), ".grok/auth.json", "{}\n");
    write(
        &world.home(),
        ".grok/config.toml",
        "[[hooks.PreToolUse]]\nmatcher = 'Bash'\nhooks = [{ command = 'user-hook' }]\n",
    );
    write(
        &world.home(),
        ".grok/skills/theirs/SKILL.md",
        "---\nname: theirs\ndescription: theirs\n---\n",
    );
    world.issue(7, &["flow:feature"]);
    tick(&world);

    let line = started(&world, &worktree(&world, 7));
    let role = role_copy(&world, "acme/dev");
    let kept = kept_from(&line);
    assert!(line.starts_with(&format!("cd '{}'", role.display())), "{line}");
    assert!(line.contains("GROK_MEMORY=0"), "{line}");
    assert!(line.contains("GROK_CLAUDE_HOOKS_ENABLED=0"), "{line}");
    assert!(line.contains("GROK_CLAUDE_MCPS_ENABLED=0"), "{line}");
    assert!(line.contains("GROK_CURSOR_HOOKS_ENABLED=0"), "{line}");
    assert!(line.contains("GROK_CURSOR_MCPS_ENABLED=0"), "{line}");
    assert!(line.contains("GROK_CLAUDE_RULES_ENABLED=0"), "{line}");
    assert!(line.contains("GROK_CURSOR_SKILLS_ENABLED=0"), "{line}");
    assert!(!line.contains("GROK_CLAUDE_SKILLS_ENABLED=0"), "{line}");
    assert!(line.contains("AFKCONVOY_HARNESS='grok' AFKCONVOY_MODEL='grok-4.7'"), "{line}");
    assert!(line.contains(" && grok --always-approve --add-dir "), "{line}");
    assert_eq!(line.matches("--add-dir").count(), 2, "{line}");
    assert!(line.contains(" --trust --model 'grok-4.7'"), "{line}");
    assert!(!line.contains("--skills-dir") && !line.contains("--rules "), "{line}");
    assert!(kept.starts_with(world.cache().join("afkconvoy/harness/grok")), "{kept:?}");
    assert_ne!(kept, world.cache().join("afkconvoy/harness/grok"));
    assert!(role.join("CLAUDE.md").is_file());
    assert!(role.join(".claude/skills/implement/SKILL.md").is_file());
    assert!(!kept.join("CLAUDE.md").exists());
    assert!(!kept.join("skills").exists());
    assert_eq!(
        std::fs::read_link(kept.join("auth.json")).unwrap(),
        world.home().join(".grok/auth.json")
    );

    let config = std::fs::read_to_string(kept.join("config.toml")).unwrap();
    for expected in [
        "role-limit",
        "afkconvoy guard",
        "exit 2",
        "grok-hook.sh",
        "exit 0",
        "Bash(rm -rf *)",
        "~/.grok/skills",
    ] {
        assert!(config.contains(expected), "missing {expected:?}: {config}");
    }
    for excluded in ["user-hook", "extra_rule_dirs", "${base_prompt}"] {
        assert!(!config.contains(excluded), "inherited {excluded:?}: {config}");
    }
    let parsed: Value = toml::from_str(&config).unwrap();
    assert_eq!(parsed["compat"]["claude"]["hooks"], false);
    assert_eq!(parsed["compat"]["claude"]["mcps"], false);
    assert_eq!(parsed["compat"]["claude"]["rules"], false);
    assert!(parsed["compat"]["claude"].get("skills").is_none());
    assert_eq!(parsed["compat"]["cursor"]["hooks"], false);
    assert_eq!(parsed["compat"]["cursor"]["mcps"], false);
    let told = std::fs::read_to_string(kept.join("rules/told.md")).unwrap();
    assert!(told.contains("# The repo's own rules:"), "{told}");
    assert!(
        told.contains("the rules of the project") && told.contains("the rules of the team"),
        "{told}"
    );
    assert!(!told.contains("base_prompt"), "{told}");
}

#[test]
fn roles_do_not_share_a_home_and_the_host_user_s_skills_and_rules_are_named() {
    let world = bound_with(TEAM);
    write(&world.home(), ".grok/auth.json", "{}\n");
    write(
        &world.home(),
        ".grok/skills/theirs/SKILL.md",
        "---\nname: theirs\ndescription: theirs\n---\n",
    );
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:audit"]);
    tick(&world);

    let dev = started(&world, &worktree(&world, 7));
    let auditor = started(&world, &worktree(&world, 8));
    let (dev_home, auditor_home) = (kept_from(&dev), kept_from(&auditor));
    assert_ne!(dev_home, auditor_home);
    assert!(
        dev_home.starts_with(world.cache().join("afkconvoy/harness/grok-host-user")),
        "{dev_home:?}"
    );
    assert!(!dev.contains("GROK_CLAUDE_RULES_ENABLED"), "{dev}");
    assert!(!dev.contains("GROK_CLAUDE_SKILLS_ENABLED"), "{dev}");
    assert_eq!(
        std::fs::read_link(dev_home.join("skills")).unwrap(),
        world.home().join(".grok/skills")
    );
    assert_eq!(
        std::fs::read_link(dev_home.join("auth.json")).unwrap(),
        world.home().join(".grok/auth.json")
    );
    let dev_config = std::fs::read_to_string(dev_home.join("config.toml")).unwrap();
    let auditor_config = std::fs::read_to_string(auditor_home.join("config.toml")).unwrap();
    assert!(
        dev_config.contains("role-limit") && dev_config.contains("~/.grok/rules"),
        "{dev_config}"
    );
    assert!(!auditor_config.contains("role-limit"));
    assert!(auditor_config.contains("afkconvoy guard"));
    assert!(!dev_config.contains("ignore"), "{dev_config}");
    assert!(dev_config.contains("extra_rule_dirs"), "{dev_config}");
}

fn guard(world: &World, described: Value) -> (i32, String) {
    let afk = env!("CARGO_BIN_EXE_afkconvoy");
    let mut child = world
        .command(afk, &world.clone)
        .args(["guard"])
        .env("AFKCONVOY_HARNESS", "grok")
        .env("AFKCONVOY_STAGE", "look")
        .env("AFKCONVOY_WRITES", "false")
        .env("AFKCONVOY_CONSULT", "false")
        .env("AFKCONVOY_BASE_BRANCH", "main")
        .env("AFKCONVOY_ESCALATE_LABEL", "ready-for-human")
        .env("AFKCONVOY_LOCK_LABEL", "fleet-lock")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(described.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn the_guard_reads_grok_s_hook_payload() {
    let world = World::new();
    let shell = |command: &str, name: &str, camel: bool| {
        if camel {
            json!({"toolName": name, "toolInput": {"command": command}})
        } else {
            json!({"tool_name": name, "tool_input": {"command": command}})
        }
    };
    let (code, said) = guard(&world, shell("git commit -m x", "run_terminal_command", true));
    assert_eq!(code, 2);
    assert!(said.contains("the stage `look` is marked `writes: false`"), "{said}");
    assert_eq!(guard(&world, shell("git status", "run_terminal_command", true)).0, 0);
    assert_eq!(guard(&world, shell("git commit -m x", "run_terminal_command", false)).0, 2);
    assert_eq!(guard(&world, shell("git commit -m x", "Bash", true)).0, 0);
}

#[test]
fn missing_or_logged_out_grok_is_named_with_its_fix() {
    for missing in [true, false] {
        let world = bound_with(TEAM);
        if missing {
            world.shell_knows("grok", None);
        } else {
            world.logged_out("grok");
        }
        world.issue(7, &["flow:feature"]);
        world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok();
        let expected =
            if missing { "is not installed" } else { "is not logged in or listed no model" };
        let plan = world.afk(&["plan", "--now", "1000"]).ok();
        assert!(plan["selection"]["skipped"].to_string().contains(expected), "{plan}");
        assert!(world.worktrees().is_empty());
        let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
        let findings = doctor["findings"].to_string();
        assert!(findings.contains(expected), "{findings}");
        let fix = if missing { "https://x.ai/cli/install.sh" } else { "grok login" };
        assert!(findings.contains(fix), "{findings}");
    }
}

#[test]
fn kit_discovers_grok_models_through_its_isolated_home() {
    let world = World::new();
    std::fs::remove_file(world.kit_path()).unwrap();
    world.shell_knows("dsh-tui", None);
    world.shell_knows("claude", None);
    world.shell_knows("pi", None);
    world.shell_knows("codex", None);
    world.shell_knows("kimi", None);
    write(
        &world.root,
        "shell-grok-models",
        "You are logged in with grok.com.\nDefault model: grok-4.7\nAvailable models:\n  * grok-4.7 (default)\n  - grok-4.5\n",
    );
    world.probes_answer();
    let written = world.afk(&["kit"]).ok();
    let held = written["kit"]["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|held| held["harness"] == "grok")
        .unwrap();
    assert_eq!(held["command"], "grok --always-approve");
    assert_eq!(held["models"][0]["model"], "grok-4.7");
    assert_eq!(held["models"][1]["model"], "grok-4.5");
    assert!(held["models"][0].get("names").is_none());
    assert!(world.shell_asked().iter().any(|line| {
        line.contains("export GROK_HOME=") && line.ends_with("&& grok --always-approve models")
    }));
}

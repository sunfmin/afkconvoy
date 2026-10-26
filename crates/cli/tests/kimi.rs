//! Kimi Code is a harness: dispatch builds its isolated role home, loads its
//! native limits into it, and starts an interactive worker with the role's
//! rules and skills. The fleet's guard reads the same hook payload Kimi sends.

use crate::common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::{World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  auditor: acme/auditor\n\
roster:\n  dev: { run: [{ harness: kimi, model: kimi-code/k3 }] }\n\
\x20 auditor: { run: [{ harness: kimi }] }\n```\n";
const FEATURE: &str = "```yaml\nstages:\n  - name: build\n    by: dev\n    do: implement\n    on: { pass: done }\n```\n";
const AUDIT: &str = "```yaml\nstages:\n  - name: look\n    by: auditor\n    do: review\n    writes: false\n    on: { pass: done }\n```\n";
const CONFIG: &str = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";

fn bound_with(team: &str) -> World {
    let world = World::new();
    world.hold_every_harness();
    world.kit_models("kimi", &["kimi-code/k3"]);
    world.with_kit(|kit| {
        let entries = kit["harnesses"].as_array_mut().unwrap();
        let at = entries.iter().position(|held| held["harness"] == "kimi").unwrap();
        let held = entries.remove(at);
        entries.insert(0, held);
    });
    world.repo(
        "acme/dev",
        &[
            ("CLAUDE.md", "the rules of a dev\n"),
            (".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n"),
            (".kimi-code/limits.toml", "[[hooks]]\nevent = 'PreToolUse'\ncommand = 'role-limit'\n[[permission.rules]]\ndecision = 'deny'\npattern = 'Bash(rm -rf*)'\n"),
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

fn bound() -> World {
    bound_with(TEAM)
}

fn tick(world: &World) -> Value {
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok()
}

fn head(world: &World, repo: &str) -> String {
    git(&world.root.join("github").join(repo), &["rev-parse", "HEAD"])
}

fn role_copy(world: &World, repo: &str) -> PathBuf {
    world
        .cache()
        .join("afkconvoy/copies")
        .join(repo)
        .join(format!("{}-role+kimi-", head(world, repo)))
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
    let (_, after) = line.split_once("KIMI_CODE_HOME='").unwrap();
    PathBuf::from(after.split('\'').next().unwrap())
}

#[test]
fn a_kimi_worker_gets_an_isolated_role_home_rules_skills_limits_and_guard() {
    let world = bound_with(&TEAM.replace("```\n", "host_user: false\n```\n"));
    write(
        &world.home(),
        ".kimi-code/config.toml",
        "default_model = 'kimi-code/k3'\ntelemetry = true\n[providers.managed]\ntype = 'kimi'\napi_key = 'secret-sentinel'\n[models.\"kimi-code/k3\"]\nprovider = 'managed'\nmodel = 'k3'\nmax_context_size = 100\n[[hooks]]\nevent = 'PreToolUse'\ncommand = 'user-hook'\n[mcp]\nstartup_timeout_ms = 1\n",
    );
    write(&world.home(), ".kimi-code/credentials/managed.json", "{}\n");
    world.issue(7, &["flow:feature"]);
    tick(&world);

    let line = started(&world, &worktree(&world, 7));
    let role = role_copy(&world, "acme/dev");
    let kept = kept_from(&line);
    assert!(
        kept.join(".skip-migration-from-kimi-cli").is_file(),
        "a fresh role home must suppress the kimi-cli migration prompt"
    );
    assert!(line.starts_with(&format!("cd '{}'", role.display())), "{line}");
    assert!(
        line.contains(
            "KIMI_CODE_TRUST_WORKSPACE=1 KIMI_CODE_NO_AUTO_UPDATE=1 KIMI_DISABLE_TELEMETRY=1"
        ),
        "{line}"
    );
    assert!(line.contains("AFKCONVOY_HARNESS='kimi' AFKCONVOY_MODEL='kimi-code/k3'"), "{line}");
    assert!(line.contains(" && kimi --auto --add-dir "), "{line}");
    assert!(line.contains(&format!("--skills-dir '{}/.claude/skills'", role.display())), "{line}");
    assert!(!line.contains("kimi-skills") && !line.contains("agent-skills"), "{line}");
    assert!(kept.starts_with(world.cache().join("afkconvoy/harness/kimi")), "{kept:?}");
    assert_ne!(kept, world.cache().join("afkconvoy/harness/kimi"));
    assert_eq!(std::fs::read_link(role.join("AGENTS.md")).unwrap(), PathBuf::from("CLAUDE.md"));
    assert_eq!(
        std::fs::read_link(kept.join("credentials")).unwrap(),
        world.home().join(".kimi-code/credentials")
    );

    let config = std::fs::read_to_string(kept.join("config.toml")).unwrap();
    for expected in [
        "secret-sentinel",
        "role-limit",
        "afkconvoy guard",
        "kimi-hook.sh",
        "default_permission_mode = \"auto\"",
        "decision = \"deny\"",
    ] {
        assert!(config.contains(expected), "missing {expected:?}: {config}");
    }
    for excluded in ["user-hook", "telemetry", "[mcp]"] {
        assert!(!config.contains(excluded), "inherited {excluded:?}: {config}");
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(kept.join("config.toml")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the generated config holds model credentials");
    let system = std::fs::read_to_string(kept.join("SYSTEM.md")).unwrap();
    assert!(system.starts_with("${base_prompt}"), "{system}");
    assert!(
        system.contains("the rules of the project") && system.contains("the rules of the team"),
        "{system}"
    );
    for question in ["type -- 'kimi'", "kimi --auto --help", "kimi provider list --json"] {
        assert!(
            world.shell_asked().contains(&question.to_string()),
            "{question}: {:?}",
            world.shell_asked()
        );
    }
}

#[test]
fn roles_do_not_share_configuration_and_host_user_resources_are_explicit() {
    let world = bound();
    write(
        &world.home(),
        ".kimi-code/skills/theirs/SKILL.md",
        "---\nname: theirs\ndescription: theirs\n---\n",
    );
    write(
        &world.home(),
        ".agents/skills/shared/SKILL.md",
        "---\nname: shared\ndescription: shared\n---\n",
    );
    write(&world.home(), ".kimi-code/AGENTS.md", "the user's rules\n");
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:audit"]);
    tick(&world);

    let dev = started(&world, &worktree(&world, 7));
    let auditor = started(&world, &worktree(&world, 8));
    let (dev_home, auditor_home) = (kept_from(&dev), kept_from(&auditor));
    assert_ne!(dev_home, auditor_home);
    assert!(dev.contains("kimi-skills") && dev.contains("agent-skills"), "{dev}");
    assert!(auditor.contains("kimi-skills") && auditor.contains("agent-skills"), "{auditor}");
    assert_eq!(
        std::fs::read_link(dev_home.join("AGENTS.md")).unwrap(),
        world.home().join(".kimi-code/AGENTS.md")
    );
    let dev_config = std::fs::read_to_string(dev_home.join("config.toml")).unwrap();
    let auditor_config = std::fs::read_to_string(auditor_home.join("config.toml")).unwrap();
    assert!(dev_config.contains("role-limit"));
    assert!(!auditor_config.contains("role-limit"));
    assert!(auditor_config.contains("afkconvoy guard"));
}

fn guard(world: &World, shell: &str) -> (i32, String) {
    let afk = env!("CARGO_BIN_EXE_afkconvoy");
    let mut child = world
        .command(afk, &world.clone)
        .args(["guard"])
        .env("AFKCONVOY_HARNESS", "kimi")
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
    let described =
        json!({"hook_event_name":"PreToolUse", "tool_name":"Bash", "tool_input":{"command":shell}});
    child.stdin.take().unwrap().write_all(described.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn the_guard_reads_kimi_s_hook_payload() {
    let world = World::new();
    let (code, said) = guard(&world, "git commit -m x");
    assert_eq!(code, 2);
    assert!(said.contains("the stage `look` is marked `writes: false`"), "{said}");
    assert_eq!(guard(&world, "git status").0, 0);
}

#[test]
fn missing_or_unconfigured_kimi_is_named_with_its_fix() {
    for missing in [true, false] {
        let world = bound();
        if missing {
            world.shell_knows("kimi", None);
        } else {
            world.logged_out("kimi");
        }
        world.issue(7, &["flow:feature"]);
        world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"]).ok();
        let expected = if missing { "is not installed" } else { "has no models configured" };
        let plan = world.afk(&["plan", "--now", "1000"]).ok();
        assert!(plan["selection"]["skipped"].to_string().contains(expected), "{plan}");
        assert!(world.worktrees().is_empty());
        let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
        let findings = doctor["findings"].to_string();
        assert!(findings.contains(expected), "{findings}");
        let fix = if missing { "https://code.kimi.com/kimi-code/install.sh" } else { "kimi login" };
        assert!(findings.contains(fix), "{findings}");
    }
}

#[test]
fn kit_discovers_kimi_models_through_its_isolated_home() {
    let world = World::new();
    std::fs::remove_file(world.kit_path()).unwrap();
    world.shell_knows("dsh-tui", None);
    world.shell_knows("claude", None);
    world.shell_knows("pi", None);
    world.shell_knows("codex", None);
    write(
        &world.root,
        "shell-kimi-models",
        &json!({"providers":{},"models":{"kimi-code/k3":{"display_name":"K3"},"kimi-code/k3-256k":{}}}).to_string(),
    );
    world.probes_answer();
    let written = world.afk(&["kit"]).ok();
    let held = written["kit"]["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|held| held["harness"] == "kimi")
        .unwrap();
    assert_eq!(held["command"], "kimi --auto");
    assert_eq!(held["models"][0]["model"], "kimi-code/k3");
    assert_eq!(held["models"][0]["names"], json!(["K3"]));
    assert_eq!(held["models"][1]["model"], "kimi-code/k3-256k");
    assert!(world.shell_asked().iter().any(|line| line.contains("export KIMI_CODE_HOME=")
        && line.ends_with("&& kimi provider list --json")));
}

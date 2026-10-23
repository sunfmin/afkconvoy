//! DSH's launch, profile isolation and the import-free plugin's real guard
//! and worker backend contracts. No installed DSH or model is needed here.
use std::process::Command;

use crate::common::{World, write};
use afkconvoy::kept::keep_under;
use afkconvoy_core::harness::Harness;

fn bound() -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[
            ("CLAUDE.md", "role-rules"),
            (".claude/skills/build/SKILL.md", "---\nname: build\ndescription: build it\n---\n"),
        ],
    );
    world.repo("acme/team", &[
        ("team.md", "```yaml\nroles: { dev: acme/dev }\n```\n"),
        ("workflows/feature.md", "```yaml\nstages:\n  - name: build\n    by: dev\n    do: build\n    harness: dsh\n    model: deepseek-test\n    on: { pass: done }\n```\n"),
        ("patrols/sweep.md", "```yaml\nby: dev\ndo: build\nharness: dsh\nevery: 1h\nmax_open: 1\n```\n"),
    ]);
    write(
        &world.clone,
        "docs/agents/afkconvoy.md",
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\nharness: dsh\n```\n",
    );
    world
}

#[test]
fn dsh_is_read_on_stages_patrols_and_the_repo_and_is_probed_before_start() {
    let world = bound();
    let read = world.afk(&["check"]).ok();
    assert_eq!(read["workflows"][0]["stages"][0]["harness"], "dsh");
    assert_eq!(read["patrols"][0]["harness"], "dsh");
    let config = world.afk(&["config"]).ok();
    assert_eq!(config["harness"], "dsh");
    // All entry points refuse a wrapper that cannot take the overlay.
    for args in [
        vec!["tick", "--instance", "a", "--host", "h1", "--now", "1000"],
        vec![
            "run",
            "--instance",
            "a",
            "--host",
            "h1",
            "--now",
            "1000",
            "--ticks",
            "1",
            "--second-ms",
            "0",
        ],
    ] {
        let why =
            world.afk(&[args.as_slice(), &["--worker-command", "dsh=agent"]].concat()).error();
        assert!(why.contains("names no --patch"), "{why}");
        assert!(why.contains("DeepSeek Harness taking flags"), "{why}");
    }
    world.shell_help(Some("no fleet flags"));
    let why = world.afk(&["plan", "--now", "1000"]).error();
    assert!(why.contains("names no --patch"), "{why}");
    let diagnosis = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
    assert!(diagnosis.to_string().contains("--patch"), "{diagnosis}");
    assert!(world.worktrees().is_empty());
}

#[test]
fn a_dsh_profile_links_only_credentials_and_opted_in_skills_and_rules() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    for (name, text) in [
        (".credentials.yaml", "credentials"),
        ("AGENTS.md", "host-rules"),
        ("skills/build/SKILL.md", "host-skill"),
        ("cordis.patch.yml", "host-patch"),
        ("profiles/dsh-tui/cordis.patch.yml", "profile-patch"),
        ("settings.yaml", "host-settings"),
    ] {
        write(&home, &format!(".dsh/{name}"), text);
    }
    let cache = temp.path().join("cache");
    write(&home, ".agents/skills/common/SKILL.md", "common-skill");
    write(&home, ".agents/AGENTS.md", "common-rules");
    for host_user in [true, false] {
        let kept = keep_under(&cache, Some(&home), Harness::Dsh, host_user).unwrap();
        assert!(kept.join(".credentials.yaml").is_symlink());
        assert_eq!(kept.join("AGENTS.md").exists(), host_user);
        assert_eq!(kept.join("host-skills").exists(), host_user);
        assert!(!kept.join("skills").exists());
        assert_eq!(kept.join("host-agents-skills").exists(), host_user);
        assert_eq!(kept.join("agents/AGENTS.md").exists(), host_user);
        assert!(!kept.join("settings.yaml").exists());
        assert!(!kept.join("cordis.patch.yml").exists());
        assert!(!kept.join("profiles/dsh-tui/cordis.patch.yml").exists());
        let overlay = std::fs::read_to_string(kept.join("afkconvoy.patch.yml")).unwrap();
        assert!(overlay.contains(&kept.join("afkconvoy.mjs").display().to_string()));
    }
    assert_eq!(std::fs::read_to_string(home.join(".dsh/cordis.patch.yml")).unwrap(), "host-patch");
    assert_eq!(
        std::fs::read_to_string(home.join(".dsh/profiles/dsh-tui/cordis.patch.yml")).unwrap(),
        "profile-patch"
    );
}

#[test]
fn the_plugin_refuses_through_the_real_guard_and_reports_running_and_interrupted_idle() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "plugin.mjs", include_str!("../../core/src/harness/dsh/plugin.mjs"));
    write(root, "CLAUDE.md", "role-rules");
    write(root, "team/CLAUDE.md", "team-rules");
    write(root, "project/AGENTS.md", "project-rules");
    write(root, "host/AGENTS.md", "host-rules");
    write(root, ".claude/skills/build/SKILL.md", "---\nname: build\ndescription: role-skill\n---");
    write(
        root,
        "host/host-skills/build/SKILL.md",
        "---\nname: build\ndescription: shadowed-skill\n---",
    );
    write(root, "driver.mjs", DRIVER);
    let output = Command::new("node")
        .arg(root.join("driver.mjs"))
        .current_dir(root)
        .env("AFKCONVOY_GUARD", format!("'{}' guard", env!("CARGO_BIN_EXE_afkconvoy")))
        .env("AFKCONVOY_WORKTREE", root.join("project"))
        .env("AFKCONVOY_TEAM_DIR", root.join("team"))
        .env("DSH_HOME", root.join("host"))
        .env("AFKCONVOY_HARNESS", "dsh")
        .env("AFKCONVOY_STAGE", "review")
        .env("AFKCONVOY_WRITES", "false")
        .env("AFKCONVOY_CONSULT", "false")
        .env("AFKCONVOY_BASE_BRANCH", "main")
        .env("AFKCONVOY_ESCALATE_LABEL", "stuck")
        .env("AFKCONVOY_LOCK_LABEL", "halt")
        .env("AFKCONVOY_DSH_MODEL", "stage-model")
        .env_remove("ORCA_AGENT_HOOK_ENDPOINT")
        .env_remove("ORCA_PANE_KEY")
        .env_remove("ORCA_AGENT_LAUNCH_TOKEN")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

const DRIVER: &str = r#"
import assert from 'node:assert/strict'
import http from 'node:http'
import { existsSync, writeFileSync } from 'node:fs'
import { apply, inject } from './plugin.mjs'
assert.deepEqual(inject, ['systemPrompt'])
const events = []
const server = http.createServer((req, res) => {
  assert.equal(req.url, '/hook/dsh')
  assert.equal(req.headers['x-orca-agent-hook-token'], 'test-token')
  let body = ''
  req.on('data', chunk => body += chunk)
  req.on('end', () => {
    const form = new URLSearchParams(body)
    assert.equal(form.get('paneKey'), 'test-pane')
    assert.equal(form.get('launchToken'), 'test-launch')
    events.push(JSON.parse(form.get('payload')).hook_event_name)
    res.end('{}')
  })
})
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
writeFileSync('endpoint', `export ORCA_AGENT_HOOK_PORT='${server.address().port}'\nexport ORCA_AGENT_HOOK_TOKEN='test-token'\n`)
process.env.ORCA_AGENT_HOOK_ENDPOINT = 'endpoint'
process.env.ORCA_AGENT_PANE = 'test-pane'
process.env.ORCA_AGENT_LAUNCH = 'test-launch'
const listeners = new Map()
let rules
let activated = false
apply({ on: (event, fn) => listeners.set(event, fn), systemPrompt: { section: s => rules = s.text },
  provide: (name, value) => { assert.equal(name, 'afkconvoyGuard'); assert.equal(value, true); activated = true } })
assert(activated)
for (const rule of ['role-rules', 'team-rules', 'project-rules', 'host-rules', 'role-skill']) assert(rules.includes(rule))
assert(!rules.includes('shadowed-skill'))
const call = listeners.get('tools/pre-execute')
let continued = 0
const next = () => { continued++; return { kind: 'allow' } }
const allowed = await call({ name: 'bash', arguments: { command: 'git status' } }, next)
assert.equal(allowed.kind, 'allow')
const denied = await call({ name: 'bash', arguments: { command: 'git commit -m forbidden' } }, next)
assert.equal(denied.kind, 'deny')
assert.match(denied.reason, /commit/)
assert.equal(continued, 1)
await call({ name: 'edit', arguments: { path: 'a', content: 'git commit' } }, next)
assert.equal(continued, 2)
const status = listeners.get('agent/status')
await listeners.get('agent/created')({ agent: { id: 'parent' } })
assert.deepEqual(events, ['SessionStart', 'Stop'])
events.length = 0
await status({ agent: { id: 'parent' }, status: 'running' })
await listeners.get('agent/created')({ agent: { id: 'child' } })
await status({ agent: { id: 'child' }, status: 'running' })
await status({ agent: { id: 'child' }, status: 'idle' })
assert.deepEqual(events, ['UserPromptSubmit'])
// An interrupt emits idle without a turn-stopping event.
await status({ agent: { id: 'parent' }, status: 'idle' })
await status({ agent: { id: 'parent' }, status: 'running' })
await status({ agent: { id: 'parent' }, status: 'idle' })
assert.deepEqual(events, ['UserPromptSubmit', 'Stop', 'UserPromptSubmit', 'Stop'])
assert.equal(await listeners.get('agent/pre-step')({}, () => 'continued'), 'continued')
assert.deepEqual(await listeners.get('agent/request')({}, async () => ({ model: 'default', provider: 'deepseek' })), { model: 'stage-model', provider: 'deepseek' })
process.env.AFKCONVOY_GUARD = 'exit 3'
// Guard command captured at activation; a second worker captures the broken one.
apply({ on: (event, fn) => listeners.set(event, fn), systemPrompt: { section() {} }, provide() {} })
assert.equal((await listeners.get('tools/pre-execute')({ name: 'read', arguments: {} }, next)).kind, 'deny')
server.close()
"#;

#[test]
fn the_generated_line_uses_native_guard_dependencies_and_quotes_every_path() {
    use afkconvoy_core::role::{Facts, On, Start, Told};
    use std::io::Write;
    use std::process::Stdio;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("a role's paths");
    std::fs::create_dir_all(&root).unwrap();
    let kept = keep_under(&root.join("cache"), Some(&root), Harness::Dsh, false).unwrap();
    let overlay = std::fs::read_to_string(kept.join("afkconvoy.patch.yml")).unwrap();
    assert_eq!(overlay.matches("inject: [afkconvoyGuard]").count(), 2);
    let role = root.join("role");
    let project = root.join("project");
    let team = root.join("team");
    std::fs::create_dir_all(&project).unwrap();
    write(&role, "CLAUDE.md", "role-rules");
    write(&role, ".dsh/cordis.patch.yml", "[]\n");
    write(&team, "CLAUDE.md", "team-rules");
    write(
        &root,
        "worker.mjs",
        r#"
import { pathToFileURL } from 'node:url'
const { apply } = await import(pathToFileURL(process.env.DSH_HOME + '/afkconvoy.mjs'))
apply({ on() {}, systemPrompt: { section() {} }, provide() {} })
process.stdin.once('data', input => {
  console.log(JSON.stringify({ input: input.toString(), cwd: process.cwd(),
    harness: process.env.AFKCONVOY_HARNESS, stage: process.env.AFKCONVOY_STAGE,
    model: process.env.AFKCONVOY_DSH_MODEL, args: process.argv.slice(2) }))
  process.exit(0)
})
"#,
    );
    let start = Start {
        role_dir: role.to_str().unwrap(),
        worktree: project.to_str().unwrap(),
        team_dir: team.to_str().unwrap(),
        model: Some("deepseek-stage"),
        guard: "true",
        kept: kept.to_str().unwrap(),
        host_user: false,
        shadowed: &[],
        told: Told::default(),
        facts: Facts {
            on: On::Stage { issue: 7, flow: "feature", stage: "build" },
            role: "dev",
            skill: "build",
            writes: true,
            consult: false,
            base_branch: "main",
            escalate_label: "stuck",
            lock_label: "halt",
            role_commit: "abc",
            team_commit: "def",
        },
    };
    let launch = afkconvoy_core::line::Line::new()
        .fixed("node ")
        .word(root.join("worker.mjs").to_str().unwrap())
        .own();
    let line = Harness::Dsh.in_role(&launch, &start);
    let run = |line: &str| {
        let mut child = Command::new("sh")
            .args(["-c", line])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"the worker's brief").unwrap();
        child.wait_with_output().unwrap()
    };
    let output = run(&line);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let read: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(read["input"], "the worker's brief");
    assert_eq!(read["cwd"], role.to_str().unwrap());
    assert_eq!(read["harness"], "dsh");
    assert_eq!(read["stage"], "build");
    assert_eq!(read["model"], "deepseek-stage");
    assert_eq!(
        read["args"],
        serde_json::json!([
            "--patch",
            role.join(".dsh/cordis.patch.yml"),
            "--patch",
            kept.join("afkconvoy.patch.yml"),
            project,
        ])
    );
    std::fs::remove_file(kept.join("afkconvoy.mjs")).unwrap();
    let output = run(&line);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("guard file afkconvoy.mjs is missing")
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn doctor_names_both_programs_a_dsh_worker_needs() {
    for program in ["dsh-tui", "dsh"] {
        let world = bound();
        world.shell_knows(program, None);
        let diagnosis = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
        let text = diagnosis.to_string();
        assert!(text.contains(program) && text.contains("not installed"), "{diagnosis}");
    }
}

/// Real DSH loader semantics; offline, no model or credentials. The two
/// worker entries stand for the TUI and loop so this tests their startup
/// dependency without depending on the installed community TUI's packages.
#[test]
#[ignore = "requires dsh installed; runs offline with an isolated profile"]
fn native_dsh_loader_refuses_a_worker_without_its_guard_service() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let kept = keep_under(&root.join("cache"), Some(root), Harness::Dsh, false).unwrap();
    write(
        &kept,
        "system.mjs",
        "export function apply(ctx) { ctx.provide('systemPrompt', { section() {} }) }\n",
    );
    write(
        &kept,
        "worker.mjs",
        "import { writeFileSync } from 'node:fs'; export function apply() { writeFileSync(process.env.DSH_HOME + '/started', 'started') }\n",
    );
    write(
        &kept,
        "control.mjs",
        "import { existsSync } from 'node:fs'; export function apply() { setTimeout(() => { console.log(JSON.stringify({ started: existsSync(process.env.DSH_HOME + '/started') })); process.exit(0) }, 100) }\n",
    );
    write(&kept, "profiles/smoke/package.json", "{\"name\":\"convoy-smoke\",\"private\":true}\n");
    write(
        &kept,
        "profiles/smoke/cordis.patch.yml",
        "- insert:\n    - id: system-prompt\n      name: ../../system.mjs\n    - id: agent-loop\n      name: ../../worker.mjs\n    - id: dsh-tui\n      name: ../../worker.mjs\n    - id: control\n      name: ../../control.mjs\n",
    );
    let run = |guard: bool| {
        let mut command = Command::new("dsh");
        command
            .args(["--profile", "smoke", "--patch"])
            .arg(kept.join("afkconvoy.patch.yml"))
            .current_dir(root)
            .env("HOME", root)
            .env("DSH_HOME", &kept)
            .env("DSH_AGENTS_HOME", root.join("agents"))
            .env("AFKCONVOY_WORKTREE", root)
            .env("AFKCONVOY_TEAM_DIR", root);
        if guard {
            command.env("AFKCONVOY_GUARD", "true");
        } else {
            command.env_remove("AFKCONVOY_GUARD");
        }
        command.output().expect("dsh must be installed to run this ignored smoke test")
    };
    let output = run(true);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(kept.join("started").is_file(), "{}", String::from_utf8_lossy(&output.stderr));
    std::fs::remove_file(kept.join("started")).unwrap();
    let output = run(false);
    assert!(!output.status.success());
    assert!(!kept.join("started").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("afkconvoyGuard"));
    std::fs::remove_file(kept.join("afkconvoy.mjs")).unwrap();
    let output = run(true);
    assert!(!output.status.success());
    assert!(!kept.join("started").exists());
    // Exercise native filesystem discovery too: all three roots have the
    // same name, and the canonical role skill must be the selected one.
    write(&kept, "afkconvoy.mjs", include_str!("../../core/src/harness/dsh/plugin.mjs"));
    for (dir, description) in [
        (root.join(".claude/skills/shared"), "role-skill"),
        (kept.join("host-skills/shared"), "host-skill"),
        (kept.join("host-agents-skills/shared"), "agents-skill"),
    ] {
        write(&dir, "SKILL.md", &format!("---\nname: shared\ndescription: {description}\n---\n"));
    }
    write(
        &kept,
        "profiles/smoke/package.json",
        "{\"name\":\"convoy-smoke\",\"private\":true,\"dsh\":{\"profile\":{\"bundles\":[\"@deepseek-ai/dsh-base\"]}}}\n",
    );
    write(
        &kept,
        "profiles/smoke/cordis.patch.yml",
        "- insert:\n    - id: dsh-tui\n      name: ../../worker.mjs\n    - id: control\n      name: ../../skills-control.mjs\n",
    );
    write(
        &kept,
        "skills-control.mjs",
        r#"
export const inject = ['skills', 'agentLoop']
export function apply(ctx) {
  setTimeout(async () => {
    try {
      const skills = await ctx.skills.list({ cwd: process.cwd() })
      process.exit(skills.some(skill => skill.name === 'shared' && skill.description === 'role-skill') ? 0 : 1)
    } catch(error) { console.error(error); process.exit(1) }
  }, 100)
}
"#,
    );
    let output = run(true);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn dsh_workers_receive_the_stock_proxy_bypass_or_the_hosts_own_list() {
    for hosts in [None, Some("dsh=custom.example"), Some("dsh=")] {
        let world = bound();
        world.issue(7, &["flow:feature"]);
        let mut args = vec!["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
        if let Some(hosts) = hosts {
            args.extend(["--worker-no-proxy", hosts]);
        }
        world.afk(&args).ok();
        let orca = world.orca();
        let commands: Vec<_> = orca["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|terminal| terminal["command"].as_str())
            .collect();
        let line = commands
            .iter()
            .find(|line| line.contains("dsh-tui"))
            .unwrap_or_else(|| panic!("{orca}"));
        match hosts {
            None => assert!(line.contains("'api.deepseek.com' && export NO_PROXY="), "{line}"),
            Some("dsh=") => assert!(!line.contains("export no_proxy="), "{line}"),
            Some(_) => assert!(line.contains("'custom.example' && export NO_PROXY="), "{line}"),
        }
    }
}

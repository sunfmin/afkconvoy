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
    for args in [vec!["tick", "--instance", "a", "--host", "h1", "--now", "1000"]] {
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
    for host_user in [true, false] {
        let kept = keep_under(&cache, Some(&home), Harness::Dsh, host_user).unwrap();
        assert!(kept.join(".credentials.yaml").is_symlink());
        assert_eq!(kept.join("AGENTS.md").exists(), host_user);
        assert_eq!(kept.join("skills").exists(), host_user);
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
    write(root, "host/skills/build/SKILL.md", "---\nname: build\ndescription: shadowed-skill\n---");
    write(root, "driver.mjs", DRIVER);
    let output = Command::new("node")
        .arg(root.join("driver.mjs"))
        .current_dir(root)
        .env("AFKCONVOY_GUARD", format!("'{}' guard", env!("CARGO_BIN_EXE_afkconvoy")))
        .env("AFKCONVOY_WORKTREE", root.join("project"))
        .env("AFKCONVOY_TEAM_DIR", root.join("team"))
        .env("DSH_HOME", root.join("host"))
        .env("AFKCONVOY_DSH_READY", root.join("ready"))
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
apply({ on: (event, fn) => listeners.set(event, fn), systemPrompt: { section: s => rules = s.text } })
assert(existsSync('ready'))
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
await status({ agent: { id: 'parent' }, status: 'running' })
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
process.env.AFKCONVOY_DSH_READY = 'ready-again'
apply({ on: (event, fn) => listeners.set(event, fn), systemPrompt: { section() {} } })
assert.equal((await listeners.get('tools/pre-execute')({ name: 'read', arguments: {} }, next)).kind, 'deny')
server.close()
"#;

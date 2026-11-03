//! pi is a harness (docs/adr/0013): a stage that names it gets a worker that
//! is a pi session, started in its role's directory with what pi does not
//! load from there added to its worker launch command — and a stage that
//! names none is started as ever. The line is read off a stand-in orca, and
//! then run with a stand-in pi, which loads the fleet's extension as pi does
//! and hands it tool calls: what refuses them is the guard itself.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{World, git, write};
use serde_json::{Value, json};

/// A team whose `dev` runs pi on one model, whose `auditor` runs pi as its
/// command is written and whose `reviewer` runs Claude Code.
const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n  auditor: acme/auditor\n\
roster:\n  dev: { run: [{ harness: pi, model: openai/gpt-5.5 }] }\n  auditor: { run: [{ harness: pi }] }\n\
\x20 reviewer: { run: [{ harness: claude-code }] }\n```\n";
/// A workflow whose first stage is on pi and whose second on Claude Code.
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: done, reject: build }
```
";
/// A workflow whose one stage reviews, on pi: no model, and nothing written.
const AUDIT: &str = "\
```yaml
stages:
  - name: look
    by: auditor
    do: review
    writes: false
    on: { pass: done }
```
";
const SWEEP: &str = "```yaml\nby: dev\ndo: implement\nevery: 1h\nmax_open: 1\n```\n";
const CONFIG: &str = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";

/// The fleet's extension for pi, as the tool carries it.
const EXTENSION: &str = include_str!("../../core/src/harness/pi/extension.js");
/// Where orca installs its own, under the home of its host's user.
const ORCA_S: &str = ".pi/agent/extensions/orca-agent-status.ts";

/// A world whose repo is bound to a team of two roles and two workflows, with
/// rules of its own, and whose own rules are in an `AGENTS.md`.
fn bound() -> World {
    bound_with(TEAM)
}

/// `bound`, of a team whose workers are given nothing of the host user's.
fn bound_alone() -> World {
    bound_with(&TEAM.replace("```\n", "host_user: false\n```\n"))
}

/// `bound`, of the team file `team`.
fn bound_with(team: &str) -> World {
    bound_to(&[
        ("team.md", team),
        ("CLAUDE.md", "the rules of the team\n"),
        ("workflows/feature.md", FEATURE),
        ("workflows/audit.md", AUDIT),
        ("patrols/sweep.md", SWEEP),
    ])
}

/// A world whose repo is bound to a team of two roles, made of `team`.
fn bound_to(team: &[(&str, &str)]) -> World {
    let world = World::new();
    world.hold_every_harness();
    world.kit_command("claude-code", afkconvoy_core::harness::Harness::ClaudeCode.stock());
    world.kit_researched_models("pi", &["openai/gpt-5.5"]);
    world.with_kit(|kit| {
        let entries = kit["harnesses"].as_array_mut().unwrap();
        let at = entries.iter().position(|h| h["harness"] == "pi").unwrap();
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
        ],
    );
    world.repo(
        "acme/reviewer",
        &[(".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n")],
    );
    world.repo(
        "acme/auditor",
        &[(".claude/skills/review/SKILL.md", "---\nname: review\ndescription: review\n---\n")],
    );
    world.repo("acme/team", team);
    write(&world.clone, "docs/agents/afkconvoy.md", CONFIG);
    write(&world.clone, "AGENTS.md", "the rules of the project\n");
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

fn tick(world: &World, now: u64) -> common::Answer {
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now])
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

fn head(world: &World, repo: &str) -> String {
    git(&world.root.join("github").join(repo), &["rev-parse", "HEAD"])
}

/// Where the fleet keeps its copy of `repo` at its head.
fn copy(world: &World, repo: &str) -> PathBuf {
    world.cache().join("afkconvoy/copies").join(repo).join(head(world, repo))
}

/// The directory the fleet keeps for pi on this world's host, for a worker
/// given the host user's skills and rules: one of a team that does not turn
/// them off.
fn kept(world: &World) -> PathBuf {
    world.cache().join("afkconvoy/harness/pi-host-user")
}

/// The directory the fleet keeps for pi on this world's host, for a worker
/// of a team that turns the host user's skills and rules off.
fn kept_alone(world: &World) -> PathBuf {
    world.cache().join("afkconvoy/harness/pi")
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The line the one terminal open in `dir` was started with.
fn started(world: &World, dir: &Path) -> String {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    let open: Vec<&Value> = here.filter(|t| t["open"] == true).collect();
    assert_eq!(open.len(), 1, "{open:?}");
    open[0]["command"].as_str().unwrap().to_string()
}

#[test]
fn a_role_s_range_holds_pi_and_pi_has_a_stock_worker_launch_command() {
    let world = bound();
    let read = world.afk(&["check"]).ok();
    let roles = read["roles"].as_array().unwrap().iter().map(|role| {
        let range = role["range"].as_array().unwrap().iter();
        let on: Vec<String> = range.map(|held| held["harness"].as_str().unwrap().into()).collect();
        format!("{} on {}", role["name"].as_str().unwrap(), on.join(", "))
    });
    let roles: Vec<String> = roles.collect();
    assert_eq!(roles, ["dev on pi", "reviewer on claude-code", "auditor on pi"]);
    assert_eq!(read["roles"][0]["range"][0]["models"], json!(["openai/gpt-5.5"]));

    // Nobody said what starts a pi worker: its stock command does, checked
    // by the start beside Claude Code's — each asked for its own flags.
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let asked = world.shell_asked();
    for question in [
        "type -- 'pi'",
        "pi --help",
        r#"claude --dangerously-skip-permissions --settings '{"skipDangerousModePermissionPrompt":true}' --help"#,
    ] {
        assert!(asked.contains(&question.to_string()), "{question:?} is not in {asked:?}");
    }
    assert!(started(&world, &worktree(&world, 7)).contains(" && pi --approve "));

    // An unsafe candidate holds only the affected role.
    let world = bound();
    world.issue(7, &["flow:audit"]);
    world.kit_command("pi", "agent");
    tick(&world, 1000).ok();
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert!(plan["selection"]["skipped"].to_string().contains("names no --approve"), "{plan}");
    assert!(plan["frontier"].as_array().unwrap().is_empty());
    assert!(world.worktrees().is_empty());
}

/// `plan`, a `tick` and a `run` → the answer of each, its last line: the
/// starts that probe.
fn starts(world: &World) -> Vec<common::Answer> {
    let run = "run --tracker default --foreground --instance alpha --host h1 --now 1000 --ticks 1 --second-ms 0";
    let tick = "tick --instance alpha --host h1 --now 1000";
    let answer = |start: &str| {
        let afk = env!("CARGO_BIN_EXE_afkconvoy");
        let out = world.command(afk, &world.clone).args(start.split(' ')).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let last = stdout.lines().last().unwrap_or_default();
        common::Answer {
            json: serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {stdout}")),
            code: out.status.code().unwrap(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    };
    ["plan --now 1000", tick, run].map(answer).into()
}

/// What `doctor` finds of pi: each finding's `what`, and the line that fixes it.
/// One of pi names it, as `` `pi` ``: a bare "pi" is in many a temporary
/// directory's random name, a fix line's path among them.
fn doctor_finds(world: &World) -> Vec<(String, String)> {
    let answer = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
    let text = |value: &Value| value.as_str().unwrap().to_string();
    let findings = answer["findings"].as_array().unwrap().iter();
    let of_pi = findings.filter(|finding| text(&finding["what"]).contains("`pi`"));
    of_pi.map(|finding| (text(&finding["what"]), text(&finding["fix"]))).collect()
}

#[test]
fn a_host_without_pi_is_refused_with_the_ranges_that_call_for_it_and_its_installer() {
    let world = bound();
    world.shell_knows("pi", None);
    world.issue(7, &["flow:feature"]);
    let install = "curl -fsSL https://pi.dev/install.sh | sh";
    for start in starts(&world) {
        start.ok();
        let plan = world.afk(&["plan", "--now", "1000"]).ok();
        assert!(plan["selection"]["skipped"].to_string().contains("is not installed"), "{plan}");
        assert!(plan["frontier"].as_array().unwrap().is_empty());
    }
    assert!(world.worktrees().is_empty());
    assert_eq!(world.claims(), []);
    let asked = world.shell_asked();
    assert!(
        !asked.iter().any(|line| line == "pi --list-models"),
        "missing is not asked: {asked:?}"
    );
    // `doctor` finds the one fault, with the same line
    assert_eq!(
        doctor_finds(&world),
        [(
            "`pi` is not installed: the fleet cannot work without it".to_string(),
            install.to_string()
        )]
    );
}

#[test]
fn a_pi_that_holds_no_credentials_is_refused_with_the_line_that_logs_it_in() {
    let world = bound();
    world.logged_out("pi");
    world.issue(7, &["flow:feature"]);
    let words = "holds credentials for no provider (`/login` at its prompt logs it in)";
    for start in starts(&world) {
        start.ok();
        let plan = world.afk(&["plan", "--now", "1000"]).ok();
        assert!(
            plan["selection"]["skipped"].to_string().contains("holds credentials for no provider"),
            "{plan}"
        );
        assert!(plan["frontier"].as_array().unwrap().is_empty());
    }
    assert!(world.worktrees().is_empty());
    assert_eq!(doctor_finds(&world), [(format!("`pi` {words}"), "pi".to_string())]);

    // logged in, the same starts go to work, and `doctor` finds nothing
    std::fs::remove_file(world.root.join("logged-out-pi")).unwrap();
    for start in starts(&world) {
        start.ok();
    }
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    assert_eq!(doctor_finds(&world), []);
}

#[test]
fn an_unavailable_pi_does_not_block_a_team_whose_roles_use_claude() {
    let on_claude = TEAM
        .replace("{ harness: pi, model: openai/gpt-5.5 }", "{ harness: claude-code }")
        .replace("{ harness: pi }", "{ harness: claude-code }");
    let world = bound_to(&[
        ("team.md", &on_claude),
        ("workflows/feature.md", FEATURE),
        ("patrols/sweep.md", SWEEP),
    ]);
    world.shell_knows("pi", None);
    world.logged_out("pi");
    world.issue(7, &["flow:feature"]);
    for start in starts(&world) {
        let said = start.stderr.clone() + &start.ok().to_string();
        assert!(!said.contains("\"error\":"), "{said}");
    }
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    assert!(doctor_finds(&world).iter().any(|(what, _)| what.contains("not installed")));
    let asked = world.shell_asked();
    assert!(
        !asked.iter().any(|line| line == "pi --list-models"),
        "missing pi is never asked for credentials: {asked:?}"
    );
}

#[test]
fn a_command_that_wraps_pi_is_asked_nothing_of_pi_s_credentials() {
    let world = bound();
    world.logged_out("pi");
    world.issue(7, &["flow:audit"]);
    world.shell_help(Some(
        "--approve --no-skills --skill --extension --model\n--add-dir --setting-sources --settings",
    ));
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    {
        world.kit_command("pi", "mypi");
        world.afk(&args)
    }
    .ok();
    assert!(!world.shell_asked().contains(&"pi --list-models".to_string()));
}

#[test]
fn a_pi_worker_starts_in_its_role_with_what_pi_does_not_load_from_there() {
    let world = bound_alone();
    // What this host's user has of pi's: a login, a rule, a skill, settings,
    // an extension of their own, and the one orca installed.
    let home = world.home();
    for (file, text) in [
        (".pi/agent/auth.json", "{\"a\": \"login\"}\n"),
        (".pi/agent/AGENTS.md", "the user's rules\n"),
        (".pi/agent/settings.json", "{}\n"),
        (".pi/agent/skills/theirs/SKILL.md", "---\nname: theirs\ndescription: theirs\n---\n"),
        (".pi/agent/extensions/theirs.ts", "// theirs\n"),
        (ORCA_S, "// orca's\n"),
    ] {
        write(&home, file, text);
    }
    world.issue(7, &["flow:feature"]);
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[0], "dispatch 7 done", "{ticked}");

    let (role, dir, kept) = (copy(&world, "acme/dev"), worktree(&world, 7), kept_alone(&world));
    assert_eq!(
        started(&world, &dir),
        format!(
            "cd '{role}' && export PI_CODING_AGENT_DIR='{kept}' AFKCONVOY_GUARD='{guard} guard' \
             AFKCONVOY_WORKTREE='{worktree}' AFKCONVOY_TEAM_DIR='{team_dir}' \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='true' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='{dev}' AFKCONVOY_TEAM_COMMIT='{team}' \
             AFKCONVOY_HARNESS='pi' && pi --approve --no-skills \
             --skill '{role}/.claude/skills' \
             --extension '{kept}/afkconvoy.js'",
            guard = "afkconvoy",
            role = role.display(),
            kept = kept.display(),
            worktree = dir.display(),
            team_dir = copy(&world, "acme/team").display(),
            dev = head(&world, "acme/dev"),
            team = head(&world, "acme/team"),
        )
    );
    // Nothing was asked at its terminal: its question is answered on the
    // line, and the worker backend was sent the brief's pointer and no key.
    let sent: Vec<Vec<String>> = world
        .orca_calls()
        .into_iter()
        .filter(|call| call.starts_with(&["terminal".to_string(), "send".to_string()]))
        .collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].iter().any(|word| word.starts_with("Your task brief is the file ")));

    // The fleet's own user directory for pi: the extension the tool carries,
    // the user's login and orca's extension as links to theirs, and no
    // models file, which the user has none of. Nothing else of the user's.
    let mut held: Vec<String> = vec![];
    for dir in [kept.clone(), kept.join("extensions")] {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            held.push(path.strip_prefix(&kept).unwrap().display().to_string());
        }
    }
    held.sort();
    assert_eq!(
        held,
        ["afkconvoy.js", "auth.json", "extensions", "extensions/orca-agent-status.ts"]
    );
    assert_eq!(std::fs::read_to_string(kept.join("afkconvoy.js")).unwrap(), EXTENSION);
    let link = |name: &str| std::fs::read_link(kept.join(name)).unwrap();
    assert_eq!(link("auth.json"), home.join(".pi/agent/auth.json"));
    assert_eq!(link("extensions/orca-agent-status.ts"), home.join(ORCA_S));

    // The next start follows what the user has by then: a login gone, the
    // models they defined since, and an extension an older build left.
    std::fs::remove_file(home.join(".pi/agent/auth.json")).unwrap();
    std::fs::remove_file(home.join(ORCA_S)).unwrap();
    write(&home, ".pi/agent/models.json", "{}\n");
    std::fs::write(kept.join("afkconvoy.js"), "// an older build's\n").unwrap();
    world.issue(8, &["flow:audit"]);
    let ticked = did(&tick(&world, 1100).ok());
    assert!(ticked.contains(&"dispatch 8 done".to_string()), "{ticked:?}");
    assert!(kept.join("auth.json").symlink_metadata().is_err(), "a link to no login is left");
    assert!(kept.join("extensions/orca-agent-status.ts").symlink_metadata().is_err());
    assert_eq!(link("models.json"), home.join(".pi/agent/models.json"));
    assert_eq!(std::fs::read_to_string(kept.join("afkconvoy.js")).unwrap(), EXTENSION);
    // A role whose roster names no model runs pi's user default: no model flag.
    let line = started(&world, &worktree(&world, 8));
    assert!(!line.contains("--model"), "{line}");
    assert!(!line.contains("AFKCONVOY_MODEL="), "{line}");
    assert!(line.contains(&format!("--extension '{}/afkconvoy.js'", kept.display())), "{line}");
}

#[test]
fn a_stage_that_names_no_harness_starts_its_worker_as_before() {
    let world = bound();
    // The first stage, on pi, passed: the second names no harness.
    world.issue(7, &["flow:feature"]);
    world.comment(
        7,
        "<!--convoy:stage stage=build verdict=pass claim=c0 role_commit=r team_commit=t \
         harness=pi-->\nbuilt",
    );
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[0], "dispatch 7 done", "{ticked}");
    let line = started(&world, &worktree(&world, 7));
    let (role, team) = (copy(&world, "acme/reviewer"), copy(&world, "acme/team"));
    let role = role.with_file_name(format!("{}-role+claude-code-", head(&world, "acme/reviewer")));
    let begins = format!(
        "cd '{}' && export CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 \
         CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 CLAUDE_CODE_SANDBOXED=1 \
         AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
         AFKCONVOY_STAGE='review' ",
        role.display()
    );
    assert!(line.starts_with(&begins), "{line}");
    let adds = format!(
        " AFKCONVOY_HARNESS='claude-code' && claude --dangerously-skip-permissions --settings \
         '{{\"skipDangerousModePermissionPrompt\":true}}' --add-dir \
         '{}' --add-dir '{}' --add-dir '{}' --setting-sources project --settings \
         '{{\"hooks\":",
        worktree(&world, 7).display(),
        team.display(),
        world.cache().join("afkconvoy/harness/claude-code-host-user").display(),
    );
    assert!(line.contains(&adds), "{line}");
    assert!(!line.contains("PI_CODING_AGENT_DIR") && !line.contains("AFKCONVOY_GUARD"), "{line}");
    // Nothing is kept for pi until it is started.
    assert!(!kept(&world).exists() && !kept_alone(&world).exists());
}

/// A stand-in pi: it takes the flags the fleet adds, loads each extension
/// named to it as pi does, and — in place of a model — hands the extensions
/// each tool call of `STAND_IN_CALLS` and starts two runs. It prints what it
/// was started with, what each call was answered, and the rules files loaded
/// by the second run: the first of pi's two in the directory it starts in,
/// and whatever an extension added.
const STAND_IN_PI: &str = r#"#!/usr/bin/env node
import { copyFileSync, existsSync, mkdtempSync, readFileSync, readdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { pathToFileURL } from "node:url"

const args = process.argv.slice(2)
const started = { cwd: process.cwd(), userDir: process.env.PI_CODING_AGENT_DIR, flags: [],
  skills: [], extensions: [], model: null }
while (args.length > 0) {
  const flag = args.shift()
  if (flag === "--skill") started.skills.push(args.shift())
  else if (flag === "--extension") started.extensions.push(args.shift())
  else if (flag === "--model") started.model = args.shift()
  else if (flag === "--approve" || flag === "--no-skills") started.flags.push(flag)
  else { console.error(`pi: unknown option ${flag}`); process.exit(1) }
}
// What pi finds by itself in its user directory. Found, not run: the
// stand-in runs no TypeScript.
const own = join(started.userDir, "extensions")
started.found = existsSync(own) ? readdirSync(own).sort() : []

const handlers = { tool_call: [], before_agent_start: [] }
const pi = { on: (name, handler) => handlers[name].push(handler) }
for (const extension of started.extensions) {
  // An extension that is not there, or does not load, is no session.
  const loadable = join(mkdtempSync(join(tmpdir(), "stand-in-pi-")), "extension.mjs")
  copyFileSync(extension, loadable)
  ;(await import(pathToFileURL(loadable).href)).default(pi)
}
const ctx = { cwd: started.cwd }
const answers = []
for (const [toolName, input] of JSON.parse(process.env.STAND_IN_CALLS ?? "[]")) {
  let answer = { ran: true }
  for (const handler of handlers.tool_call) {
    try {
      const said = await handler({ type: "tool_call", toolName, toolCallId: "call", input }, ctx)
      if (said?.block) answer = { ran: false, reason: said.reason }
    } catch (error) {
      // A handler that throws refuses the call, with what it threw.
      answer = { ran: false, reason: String(error.message), threw: true }
    }
    if (!answer.ran) break
  }
  answers.push(answer)
}
const contextFiles = []
for (const name of ["AGENTS.md", "CLAUDE.md"]) {
  const path = join(started.cwd, name)
  if (existsSync(path)) { contextFiles.push({ path, content: readFileSync(path, "utf8") }); break }
}
const event = { type: "before_agent_start", systemPrompt: "", systemPromptOptions: { contextFiles } }
for (let run = 0; run < 2; run++) for (const handler of handlers.before_agent_start) await handler(event, ctx)
console.log(JSON.stringify({ started, answers, rules: contextFiles }))
"#;

/// Whether this host has a `node` to run the stand-in pi with.
fn has_node() -> bool {
    Command::new("node").arg("--version").output().is_ok_and(|out| out.status.success())
}

/// Runs `line`, the line a pi worker was started with, with the stand-in pi
/// for `pi` and `calls` for the tool calls its model makes → what the
/// stand-in printed. Nothing of this process's environment is the worker's
/// but its PATH; the worker uses the world's cache.
fn under_pi(world: &World, line: &str, calls: &Value) -> Value {
    use std::os::unix::fs::PermissionsExt;
    let bin = world.root.join("stand-in");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("pi"), STAND_IN_PI).unwrap();
    std::fs::set_permissions(bin.join("pi"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new("/bin/sh")
        .args(["-c", line])
        .env_clear()
        .env("PATH", format!("{}:{}", bin.display(), world.path()))
        .env("XDG_CACHE_HOME", world.cache())
        .env("STAND_IN_CALLS", calls.to_string())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{line}\n{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

/// What the guard answers Claude Code for the shell line `shell`, asked as
/// its hook asks, under the facts of the worker `line` started: its exit
/// code and what it said.
fn claude_is_told(world: &World, line: &str, shell: &str) -> (i32, String) {
    use std::io::Write;
    let mut steps = line.split(" && ");
    let (enter, facts) = (steps.next().unwrap(), steps.next().unwrap());
    let guard = format!(
        "{enter} && {facts} && AFKCONVOY_HARNESS=claude-code '{}' guard",
        env!("CARGO_BIN_EXE_afkconvoy")
    );
    let mut child = Command::new("/bin/sh")
        .args(["-c", &guard])
        .env_clear()
        .envs(world.login_env())
        .env("XDG_CACHE_HOME", world.cache())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let described = json!({"tool_name": "Bash", "tool_input": {"command": shell}});
    child.stdin.take().unwrap().write_all(described.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stderr).unwrap().trim().to_string())
}

fn bash(line: &str) -> Value {
    json!(["bash", {"command": line}])
}

#[test]
fn a_pi_worker_is_refused_what_a_claude_code_worker_is_by_the_same_guard() {
    if !has_node() {
        eprintln!("skipped: no `node` on PATH to run the stand-in pi with");
        return;
    }
    let world = bound();
    world.issue(7, &["flow:audit"]);
    world.issue(8, &["flow:feature"]);
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[..2], ["dispatch 7 done", "dispatch 8 done"], "{ticked}");
    let (look, build) = (worktree(&world, 7), worktree(&world, 8));
    let (reviews, writes) = (started(&world, &look), started(&world, &build));

    // The stand-in was started as pi is: in the role's directory, under the
    // fleet's own user directory, its question answered, with the role's
    // skills, then the host user's, and no others, and the fleet's extension.
    // No model label is followed, so the user default names no model.
    let ran = under_pi(&world, &writes, &json!([]));
    let role = copy(&world, "acme/dev");
    assert_eq!(
        ran["started"],
        json!({"cwd": role, "userDir": kept(&world), "flags": ["--approve", "--no-skills"],
            "skills": [role.join(".claude/skills"), kept(&world).join("skills")],
            "extensions": [kept(&world).join("afkconvoy.js")],
            "model": null, "found": []})
    );
    // Its rules are its role's, which pi loads itself, and the worktree's
    // and the team's, which the extension adds — once, however many runs
    // there are.
    let rules: Vec<&str> = ran["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        rules,
        [
            role.join("CLAUDE.md").to_str().unwrap(),
            build.join("AGENTS.md").to_str().unwrap(),
            copy(&world, "acme/team").join("CLAUDE.md").to_str().unwrap()
        ]
    );
    assert_eq!(ran["rules"][1]["content"], "the rules of the project\n");
    assert_eq!(ran["rules"][2]["content"], "the rules of the team\n");

    // What every worker is refused, and — in a stage that writes nothing — a
    // commit: each for the reason Claude Code is given.
    let every = [
        "gh pr merge 70 --squash",
        "git push origin main",
        "gh issue edit 7 --add-label flow:feature",
        "gh issue edit 7 --remove-label flow:audit",
    ];
    let commit = "git add -A && git commit -m step";
    let allowed = ["ls", "git status"];
    for (line, refused) in
        [(&writes, &every[..]), (&reviews, &[&every[..], &[commit]].concat()[..])]
    {
        let calls: Vec<Value> = refused.iter().chain(&allowed).map(|shell| bash(shell)).collect();
        let ran = under_pi(&world, line, &json!(calls));
        let answers = ran["answers"].as_array().unwrap();
        for (shell, answer) in refused.iter().zip(answers) {
            let (code, said) = claude_is_told(&world, line, shell);
            assert_eq!(code, 2, "{shell}: Claude Code is not refused it");
            assert!(said.starts_with("afkconvoy guard: "), "{shell}: {said}");
            assert_eq!(answer, &json!({"ran": false, "reason": said}), "{shell}");
        }
        for (shell, answer) in allowed.iter().zip(&answers[refused.len()..]) {
            assert_eq!(answer, &json!({"ran": true}), "{shell}");
        }
    }
    // A stage that writes may commit; and a tool that runs no shell line is
    // the guard's to allow, whatever it is handed.
    let calls = json!([bash(commit), ["write", {"path": "a", "content": "git push origin main"}]]);
    let ran = under_pi(&world, &writes, &calls);
    assert_eq!(ran["answers"], json!([{"ran": true}, {"ran": true}]));
    let said = &under_pi(&world, &reviews, &json!([bash(commit)]))["answers"][0]["reason"];
    assert!(
        said.as_str().unwrap().contains("the stage `look` is marked `writes: false`: `git commit`"),
        "{said}"
    );
}

#[test]
fn a_pi_worker_of_a_team_with_no_rules_is_given_its_role_s_and_its_worktree_s() {
    if !has_node() {
        eprintln!("skipped: no `node` on PATH to run the stand-in pi with");
        return;
    }
    let world = bound_to(&[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let build = worktree(&world, 7);
    let ran = under_pi(&world, &started(&world, &build), &json!([]));
    let rules: Vec<&str> =
        ran["rules"].as_array().unwrap().iter().flat_map(|file| file["path"].as_str()).collect();
    let role = copy(&world, "acme/dev");
    assert_eq!(
        rules,
        [role.join("CLAUDE.md").to_str().unwrap(), build.join("AGENTS.md").to_str().unwrap()]
    );
}

#[test]
fn a_tool_call_is_refused_when_the_guard_cannot_be_reached() {
    if !has_node() {
        eprintln!("skipped: no `node` on PATH to run the stand-in pi with");
        return;
    }
    let world = bound();
    world.issue(8, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 8 done");
    let line = started(&world, &worktree(&world, 8));
    let calls = json!([bash("ls"), ["read", {"path": "README.md"}]]);
    let answers = |line: &str| under_pi(&world, line, &calls)["answers"].clone();
    assert_eq!(answers(&line), json!([{"ran": true}, {"ran": true}]));

    let guard = "AFKCONVOY_GUARD='afkconvoy guard'";
    assert!(line.contains(guard), "{line}");
    let refused = |(from, with): (&str, &str), why: &str| {
        assert!(line.contains(from), "{from}: {line}");
        let answers = answers(&line.replace(from, with));
        for answer in answers.as_array().unwrap() {
            assert_eq!(answer["ran"], false, "{with}: {answers}");
            assert!(answer["reason"].as_str().unwrap().contains(why), "{with}: {answers}");
        }
        answers
    };
    // A guard that is not there, one that fails saying nothing, one that
    // cannot read the stage, and one that never answers its name.
    refused((guard, "AFKCONVOY_GUARD='/nowhere/afkconvoy guard'"), "/nowhere/afkconvoy");
    refused((guard, "AFKCONVOY_GUARD='exit 1'"), "afkconvoy guard: it could not be asked");
    refused(("AFKCONVOY_STAGE='build' ", ""), "AFKCONVOY_STAGE is not set");
    refused(("AFKCONVOY_HARNESS='pi'", "AFKCONVOY_HARNESS='aider'"), "`aider` is no harness");
    // With no line to ask it by, the extension refuses by throwing.
    let unset = refused((guard, "AFKCONVOY_NO_GUARD=''"), "AFKCONVOY_GUARD is not set");
    assert_eq!(unset[0]["threw"], true);

    // An extension that is not there is no session at all.
    let gone = line.replace("/afkconvoy.js'", "/nothing.js'");
    let bin = world.root.join("stand-in");
    let out = Command::new("/bin/sh")
        .args(["-c", &gone])
        .env_clear()
        .env("PATH", format!("{}:{}", bin.display(), world.path()))
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn a_pi_worker_is_given_the_host_user_s_skills_and_rules_after_its_role_s() {
    let world = bound();
    // What this host's user has of pi's: a login, a rule, a skill, settings
    // and an extension of their own.
    let home = world.home();
    for (file, text) in [
        (".pi/agent/auth.json", "{\"a\": \"login\"}\n"),
        (".pi/agent/AGENTS.md", "the user's rules\n"),
        (".pi/agent/settings.json", "{}\n"),
        (".pi/agent/skills/implement/SKILL.md", "---\nname: implement\ndescription: x\n---\n"),
        (".pi/agent/extensions/theirs.ts", "// theirs\n"),
    ] {
        write(&home, file, text);
    }
    world.issue(7, &["flow:feature"]);
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[0], "dispatch 7 done", "{ticked}");

    // The user directory kept for it links to the host user's skills and to
    // their rules, where pi loads a user's from — and to no setting and no
    // extension of theirs.
    let kept = kept(&world);
    let link = |name: &str| std::fs::read_link(kept.join(name)).unwrap();
    assert_eq!(link("AGENTS.md"), home.join(".pi/agent/AGENTS.md"));
    assert_eq!(link("skills"), home.join(".pi/agent/skills"));
    assert!(kept.join("settings.json").symlink_metadata().is_err());
    assert!(kept.join("extensions/theirs.ts").symlink_metadata().is_err());
    assert!(!kept_alone(&world).exists(), "nothing is kept without them");

    // Their skills are named after the role's: of two of one name pi keeps
    // the first, the role's.
    let line = started(&world, &worktree(&world, 7));
    let skills = format!(
        " --no-skills --skill '{}/.claude/skills' --skill '{}/skills' ",
        copy(&world, "acme/dev").display(),
        kept.display()
    );
    assert!(line.contains(&skills), "{line}");
    assert!(line.contains(&format!("PI_CODING_AGENT_DIR='{}'", kept.display())), "{line}");
}

#[test]
fn a_pi_worker_of_a_role_that_keeps_from_the_host_user_s_skills_and_rules_is_given_none() {
    let own = "auditor: { dir: acme/auditor, host_user: false }";
    let world = bound_with(&TEAM.replace("auditor: acme/auditor", own));
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:audit"]);
    let ticked = tick(&world, 1000).ok();
    let did = did(&ticked);
    assert!(did.contains(&"dispatch 7 done".to_string()), "{ticked}");
    assert!(did.contains(&"dispatch 8 done".to_string()), "{ticked}");
    // `dev` takes the team's, and is given them; `auditor` says otherwise.
    let given = |issue: u64| {
        let line = started(&world, &worktree(&world, issue));
        let (with, alone) = (
            format!("'{}'", kept(&world).display()),
            format!("'{}'", kept_alone(&world).display()),
        );
        assert_ne!(line.contains(&with), line.contains(&alone), "{line}");
        line.contains(&with)
    };
    assert!(given(7), "dev is started on pi with the host user's");
    assert!(!given(8), "auditor is started on pi with none of the host user's");
}

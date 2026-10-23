//! Codex is a harness (docs/adr/0013): a stage that names it gets a worker
//! that is a Codex session, started in its role's directory under a user
//! directory of the fleet's own, with the hooks that ask the guard and tell
//! the worker backend said on the line that starts it. The line is read off a
//! stand-in orca, and then run with a stand-in Codex, which says what it was
//! started with; the hooks it was given are then run as Codex runs one: what
//! refuses a call is the guard itself, and what orca hears is what the fleet
//! then reads off it.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{World, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
/// A workflow whose first stage is on Codex and whose second names no harness.
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    harness: codex
    model: gpt-5.5
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: done, reject: build }
```
";
/// A workflow whose one stage reviews, on Codex: no model, and nothing written.
const AUDIT: &str = "\
```yaml
stages:
  - name: look
    by: reviewer
    do: review
    harness: codex
    writes: false
    on: { pass: done }
```
";
const SWEEP: &str =
    "```yaml\nby: dev\ndo: implement\nharness: codex\nevery: 1h\nmax_open: 1\n```\n";
const CONFIG: &str = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";

/// A world whose repo is bound to a team of two roles and two workflows.
/// The file of the skill `implement`.
const IMPLEMENT: &str = "---\nname: implement\ndescription: build\n---\n";

fn bound() -> World {
    bound_with(TEAM)
}

/// `bound`, of a team whose workers are given nothing of the host user's.
fn bound_alone() -> World {
    bound_with(&TEAM.replace("```\n", "host_user: false\n```\n"))
}

/// `bound`, of the team file `team`.
fn bound_with(team: &str) -> World {
    let world = World::new();
    world.kit_command("claude-code", afkconvoy_core::harness::Harness::ClaudeCode.stock());
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
        "acme/team",
        &[
            ("team.md", team),
            ("workflows/feature.md", FEATURE),
            ("workflows/audit.md", AUDIT),
            ("patrols/sweep.md", SWEEP),
        ],
    );
    configure(&world, CONFIG);
    world
}

/// Has the repo's config be `config`, on its base branch.
fn configure(world: &World, config: &str) {
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
}

const TICK: [&str; 7] = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];

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

/// Where the fleet keeps the copy of the role `repo` at its head that a
/// Codex worker starts in: Codex's own, with the link to the role's skills.
fn role_copy(world: &World, repo: &str) -> PathBuf {
    let copies = world.cache().join("afkconvoy/copies").join(repo);
    copies.join(format!("{}-role+codex-", head(world, repo)))
}

/// The directory the fleet keeps for Codex on this world's host, for a
/// worker of a team that turns the host user's skills and rules off.
fn kept(world: &World) -> PathBuf {
    world.cache().join("afkconvoy/harness/codex")
}

/// The directory the fleet keeps for Codex on this world's host, for a
/// worker given the host user's skills and rules.
fn kept_host_user(world: &World) -> PathBuf {
    world.cache().join("afkconvoy/harness/codex-host-user")
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

/// Each stage `check` reads, as "workflow/stage on harness", and the harness
/// of the one patrol.
fn harnesses(world: &World) -> (Vec<String>, Value) {
    let read = world.afk(&["check"]).ok();
    let stages = read["workflows"].as_array().unwrap().iter().flat_map(|workflow| {
        let name = workflow["name"].as_str().unwrap().to_string();
        let stages = workflow["stages"].as_array().unwrap().clone();
        stages.into_iter().map(move |stage| {
            format!("{name}/{}", stage["name"].as_str().unwrap())
                + " on "
                + stage["harness"].as_str().unwrap()
        })
    });
    let mut stages: Vec<String> = stages.collect();
    stages.sort();
    (stages, read["patrols"][0]["harness"].clone())
}

#[test]
fn a_stage_a_patrol_and_a_config_name_codex_and_codex_has_a_stock_worker_launch_command() {
    let world = bound();
    let (stages, patrol) = harnesses(&world);
    assert_eq!(
        stages,
        ["audit/look on codex", "feature/build on codex", "feature/review on claude-code"]
    );
    assert_eq!(patrol, "codex");
    let afk = env!("CARGO_BIN_EXE_afkconvoy");
    let shown = world.command(afk, &world.clone).args(["show", "feature"]).output().unwrap();
    let shown = String::from_utf8(shown.stdout).unwrap();
    assert!(shown.contains("build   dev: implement · harness codex · model gpt-5.5"), "{shown}");

    // Nobody said what starts a Codex worker: its stock command does, checked
    // by the start beside Claude Code's — each asked for its own flags.
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let asked = world.shell_asked();
    for question in [
        "type -- 'codex'",
        "codex --dangerously-bypass-approvals-and-sandbox --help",
        r#"test -f "$HOME/.codex/auth.json" && codex login status"#,
        r#"claude --dangerously-skip-permissions --settings '{"skipDangerousModePermissionPrompt":true}' --help"#,
    ] {
        assert!(asked.contains(&question.to_string()), "{question:?} is not in {asked:?}");
    }
    let line = started(&world, &worktree(&world, 7));
    assert!(
        line.contains(" && codex --dangerously-bypass-approvals-and-sandbox --add-dir "),
        "{line}"
    );

    // A repo's config names it as its default: a stage that names none is on it.
    let world = bound();
    configure(&world, &CONFIG.replace("team: acme/team\n", "team: acme/team\nharness: codex\n"));
    let (stages, _) = harnesses(&world);
    assert_eq!(stages[2], "feature/review on codex");
}

#[test]
fn a_command_that_does_not_take_what_the_fleet_adds_for_codex_starts_nothing() {
    let world = bound();
    world.issue(7, &["flow:audit"]);
    world.kit_command("codex", "agent");
    let given: [&str; 0] = [];
    let named = "`agent --help` names no --dangerously-bypass-hook-trust, no --config.";
    // …nor does the stock command of a Codex that is older than those flags,
    // which `plan` and `doctor` check as a start does.
    let stock = "codex --dangerously-bypass-approvals-and-sandbox";
    let old = format!("`{stock} --help` names no --dangerously-bypass-hook-trust, no --config.");
    let said = world.afk(&[&TICK[..], &given].concat()).error();
    assert!(said.contains(named), "{said}");
    world.kit_command("codex", stock);
    world.shell_help(Some("--add-dir --setting-sources --settings --model"));
    let refusals = [
        (named, said),
        (&old[..], tick(&world, 1000).error()),
        (&old[..], world.afk(&["plan", "--now", "1000"]).error()),
    ];
    for (named, why) in refusals {
        assert!(why.contains(named), "{why}");
        assert!(why.contains("must end in Codex taking flags"), "{why}");
        assert!(why.contains("the worker launch command of the harness `codex`"), "{why}");
        assert!(why.contains("the stage `look` of the workflow `audit`"), "{why}");
    }
    assert!(world.worktrees().is_empty());
    let found = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json.to_string();
    assert!(found.contains("names no --dangerously-bypass-hook-trust, no --config"), "{found}");
    assert!(found.contains("must end in Codex taking flags"), "{found}");
}

/// `plan`, a `tick` and a `run` → the answer of each, its last line: the
/// starts that probe.
fn starts(world: &World) -> Vec<common::Answer> {
    let run = "run --instance alpha --host h1 --now 1000 --ticks 1 --second-ms 0";
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

/// What `doctor` finds of Codex: each finding's `what`, and the line that fixes it.
fn doctor_finds(world: &World) -> Vec<(String, String)> {
    let answer = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
    let text = |value: &Value| value.as_str().unwrap().to_string();
    let findings = answer["findings"].as_array().unwrap().iter();
    let of_codex = findings.filter(|finding| finding.to_string().contains("codex"));
    of_codex.map(|finding| (text(&finding["what"]), text(&finding["fix"]))).collect()
}

const CALLED: &str = "called for by the stage `look` of the workflow `audit`, the stage `build` of \
                      the workflow `feature`";

#[test]
fn a_host_without_codex_or_with_no_login_a_worker_can_use_is_refused_before_a_worker_is_started() {
    let world = bound();
    world.shell_knows("codex", None);
    world.issue(7, &["flow:feature"]);
    let install = "brew install --cask codex";
    for start in starts(&world) {
        let error = start.error();
        let said: Vec<&str> = error.lines().skip(1).collect();
        assert_eq!(said, [format!("  codex is not installed, {CALLED}: {install}")], "{error}");
    }
    assert_eq!(
        doctor_finds(&world),
        [(
            "`codex` is not installed: the fleet cannot work without it".to_string(),
            install.to_string()
        )]
    );

    // A user with no login in the file a worker is linked to is told so.
    let world = bound();
    world.logged_out("codex");
    world.issue(7, &["flow:feature"]);
    let words = "is not logged in where a worker reads its login, `~/.codex/auth.json` (a login \
                 kept in a keyring is in no file)";
    for start in starts(&world) {
        let error = start.error();
        let said: Vec<&str> = error.lines().skip(1).collect();
        assert_eq!(said, [format!("  codex {words}, {CALLED}: codex login")], "{error}");
    }
    assert!(world.worktrees().is_empty());
    assert_eq!(world.claims(), []);
    assert_eq!(doctor_finds(&world), [(format!("`codex` {words}"), "codex login".to_string())]);

    // Logged in, the same starts go to work, and `doctor` finds nothing.
    std::fs::remove_file(world.root.join("logged-out-codex")).unwrap();
    for start in starts(&world) {
        start.ok();
    }
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    assert_eq!(doctor_finds(&world), []);
}

/// The line that tells orca of a Codex worker's state: orca's own hook for
/// Codex, where the host has one, and whatever it does, nothing said and no
/// failure.
const REPORT: &str = r#"hook="${HOME-}/.orca/agent-hooks/codex-hook.sh"; if [ -x "$hook" ]; then /bin/sh "$hook"; else cat; fi >/dev/null 2>&1; exit 0"#;
/// The events at which orca names its hook for Codex, the one before each
/// tool call apart.
const STATE_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "PostToolUse",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];

/// The hooks that ask the guard `guard` and tell orca, as the line that
/// starts a worker says them to Codex: the guard before each tool call, and
/// the report there and at each event at which a worker's state changes,
/// with a timeout.
fn hook(guard: &str) -> String {
    // A TOML string that holds nothing but these escapes is a JSON one too.
    let text = |line: &str| serde_json::to_string(line).unwrap();
    let guard = text(&format!(
        "said=$({guard} 2>&1 >/dev/null) && exit 0; printf '%s\\n' \"${{said:-afkconvoy guard: \
         it could not be asked}}\" >&2; exit 2"
    ));
    let report = format!("{{type=\"command\",command={},timeout=10}}", text(REPORT));
    let mut hooks =
        format!("hooks={{PreToolUse=[{{hooks=[{{type=\"command\",command={guard}}},{report}]}}]");
    for event in STATE_EVENTS {
        hooks.push_str(&format!(",{event}=[{{hooks=[{report}]}}]"));
    }
    hooks + "}"
}

#[test]
fn a_codex_worker_starts_in_its_role_under_a_user_directory_of_the_fleet_s_own() {
    let world = bound_alone();
    // What this host's user has of Codex's: a login, rules, a configuration,
    // hooks, a skill and memories.
    let home = world.home();
    for (file, text) in [
        (".codex/auth.json", "{\"a\": \"login\"}\n"),
        (".codex/AGENTS.md", "the user's rules\n"),
        (".codex/config.toml", "model = \"theirs\"\n"),
        (".codex/hooks.json", "{\"hooks\": {}}\n"),
        (".codex/skills/theirs/SKILL.md", "---\nname: theirs\ndescription: theirs\n---\n"),
        (".codex/memories/theirs.md", "theirs\n"),
    ] {
        write(&home, file, text);
    }
    world.issue(7, &["flow:feature"]);
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[0], "dispatch 7 done", "{ticked}");

    let (role, dir, kept) = (role_copy(&world, "acme/dev"), worktree(&world, 7), kept(&world));
    let guard = format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy"));
    let line = started(&world, &dir);
    assert_eq!(
        line,
        format!(
            "cd '{role}' && export CODEX_HOME='{kept}' \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='true' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='{dev}' AFKCONVOY_TEAM_COMMIT='{team}' \
             AFKCONVOY_HARNESS='codex' && codex --dangerously-bypass-approvals-and-sandbox \
             --add-dir '{worktree}' --dangerously-bypass-hook-trust \
             --config 'projects={{\"{role}\"={{trust_level=\"trusted\"}}}}' \
             --config 'project_doc_fallback_filenames=[\"CLAUDE.md\"]' \
             --config 'project_root_markers=[]' \
             --config '{hook}' --config features.memories=false \
             --config skills.bundled.enabled=false --model 'gpt-5.5'",
            role = role.display(),
            kept = kept.display(),
            worktree = dir.display(),
            dev = head(&world, "acme/dev"),
            team = head(&world, "acme/team"),
            hook = hook(&guard).replace('\'', "'\\''"),
        )
    );
    // Nothing was asked at its terminal: both of its questions are answered
    // on the line, and the worker backend was sent the brief's pointer alone.
    let sent: Vec<Vec<String>> = world
        .orca_calls()
        .into_iter()
        .filter(|call| call.starts_with(&["terminal".to_string(), "send".to_string()]))
        .collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].iter().any(|word| word.starts_with("Your task brief is the file ")));

    // The fleet's own user directory for Codex: the user's login, as a link
    // to theirs, and nothing else of the user's — and no `AGENTS.md`, which
    // Codex would load from there as every worker's rules.
    let held = |kept: &Path| -> Vec<String> {
        let entries = std::fs::read_dir(kept).unwrap().flatten();
        entries.map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()
    };
    assert_eq!(held(&kept), ["auth.json"]);
    assert_eq!(std::fs::read_link(kept.join("auth.json")).unwrap(), home.join(".codex/auth.json"));

    // The stand-in is started as Codex is: in the role's directory, under
    // that user directory, with each thing the fleet adds as one word.
    let ran = under_codex(&world, &line);
    let words = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
    assert_eq!(ran[..2], words(&[role.to_str().unwrap(), kept.to_str().unwrap()]));
    assert_eq!(
        ran[2..],
        words(&[
            "--dangerously-bypass-approvals-and-sandbox",
            "--add-dir",
            dir.to_str().unwrap(),
            "--dangerously-bypass-hook-trust",
            "--config",
            &format!("projects={{\"{}\"={{trust_level=\"trusted\"}}}}", role.display()),
            "--config",
            "project_doc_fallback_filenames=[\"CLAUDE.md\"]",
            "--config",
            "project_root_markers=[]",
            "--config",
            &hook(&guard),
            "--config",
            "features.memories=false",
            "--config",
            "skills.bundled.enabled=false",
            "--model",
            "gpt-5.5",
        ])
    );

    // The next start follows what the user has by then: a login gone. The
    // directory is there all the same, and a stage that names no model names
    // none to Codex.
    std::fs::remove_file(home.join(".codex/auth.json")).unwrap();
    world.issue(8, &["flow:audit"]);
    let ticked = did(&tick(&world, 1100).ok());
    assert!(ticked.contains(&"dispatch 8 done".to_string()), "{ticked:?}");
    assert_eq!(held(&kept), [] as [&str; 0], "a link to no login is left");
    let line = started(&world, &worktree(&world, 8));
    assert!(line.ends_with(" --config skills.bundled.enabled=false"), "{line}");
}

/// What the stand-in Codex was told in `ran` as a developer's: the rules the
/// fleet gives it from outside its role's directory. `None`: it was told none.
fn told(ran: &[String]) -> Option<String> {
    let said: Vec<&String> =
        ran.iter().filter(|word| word.starts_with("developer_instructions=")).collect();
    assert!(said.len() <= 1, "{ran:?}");
    // A TOML string that holds nothing but these escapes is a JSON one too.
    said.first().map(|said| serde_json::from_str(&said["developer_instructions=".len()..]).unwrap())
}

const TOLD_OPENS: &str = "afkconvoy started you, and gives you the rules below. You work under \
                          them as you do under your role's, which are those of the directory you \
                          started in.";

#[test]
fn a_codex_worker_is_shown_its_role_s_rules_the_repo_s_own_and_the_team_rules() {
    // A repo with no rules of its own, and a team with none: the worker is
    // told none, and starts all the same.
    let world = bound();
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let ran = under_codex(&world, &started(&world, &worktree(&world, 7)));
    assert_eq!(told(&ran), None);
    // Its role's rules are the `CLAUDE.md` of the directory it starts in,
    // which Codex is told to load — and nothing of a directory above it.
    let role = role_copy(&world, "acme/dev");
    assert_eq!(ran[0], role.to_str().unwrap());
    assert_eq!(std::fs::read_to_string(role.join("CLAUDE.md")).unwrap(), "the rules of a dev\n");
    for said in ["project_doc_fallback_filenames=[\"CLAUDE.md\"]", "project_root_markers=[]"] {
        assert!(ran.contains(&said.to_string()), "{said}: {ran:?}");
    }

    // The team gets rules, and the repo its own, in a `CLAUDE.md`: Codex
    // loads neither from a directory added to it, so each is said on the
    // line, as what it is.
    let team = world.root.join("github/acme/team");
    write(&team, "CLAUDE.md", "the rules of the team\n");
    git(&team, &["add", "."]);
    git(&team, &["commit", "-q", "-m", "team rules"]);
    write(&world.clone, "CLAUDE.md", "the rules of the project\n\nIt's \"ours\".\n");
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "the repo's own rules"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.issue(8, &["flow:audit"]);
    let ticked = did(&tick(&world, 1100).ok());
    assert!(ticked.contains(&"dispatch 8 done".to_string()), "{ticked:?}");
    let dir = worktree(&world, 8);
    let team_rules = copy(&world, "acme/team").join("CLAUDE.md");
    let ran = under_codex(&world, &started(&world, &dir));
    assert_eq!(
        told(&ran).unwrap(),
        format!(
            "{TOLD_OPENS}\n\n# The repo's own rules: {}\n\n<rules>\nthe rules of the \
             project\n\nIt's \"ours\".\n</rules>\n\n# The team rules: {}\n\n<rules>\nthe rules \
             of the team\n</rules>",
            dir.join("CLAUDE.md").display(),
            team_rules.display()
        )
    );

    // A repo that holds an `AGENTS.md` has its own rules there, as for pi.
    write(&world.clone, "AGENTS.md", "the rules of the project, for agents\n");
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "an AGENTS.md"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.issue(9, &["flow:audit"]);
    let ticked = did(&tick(&world, 1200).ok());
    assert!(ticked.contains(&"dispatch 9 done".to_string()), "{ticked:?}");
    let dir = worktree(&world, 9);
    let said = told(&under_codex(&world, &started(&world, &dir))).unwrap();
    let own = format!(
        "# The repo's own rules: {}\n\n<rules>\nthe rules of the project, for agents\n</rules>",
        dir.join("AGENTS.md").display()
    );
    assert!(said.contains(&own), "{said}");
    assert!(!said.contains("It's \"ours\""), "{said}");
    assert!(said.contains("# The team rules: "), "{said}");
    // None of it is kept in a file Codex would load as every worker's.
    assert!(!kept_host_user(&world).join("AGENTS.md").exists());
}

/// A stand-in Codex: it prints the directory it was started in, the user
/// directory it was given and each of its arguments, a NUL after each.
const STAND_IN_CODEX: &str = "#!/bin/sh\nprintf '%s\\0' \"$PWD\" \"$CODEX_HOME\" \"$@\"\n";

/// Runs `line`, the line a Codex worker was started with, with the stand-in
/// Codex for `codex` → what the stand-in printed. Nothing of this process's
/// environment is the worker's but its PATH.
fn under_codex(world: &World, line: &str) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let bin = world.root.join("stand-in");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("codex"), STAND_IN_CODEX).unwrap();
    std::fs::set_permissions(bin.join("codex"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new("/bin/sh")
        .args(["-c", line])
        .env_clear()
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{line}\n{}", String::from_utf8_lossy(&out.stderr));
    let printed = String::from_utf8(out.stdout).unwrap();
    printed.split_terminator('\0').map(str::to_string).collect()
}

/// The shell lines of the two hooks the stand-in Codex was given in `ran`,
/// which Codex runs before each tool call — the guard, then the report:
/// there is no other place they are kept, and a start that names none has
/// none.
fn hooked(ran: &[String]) -> (String, String) {
    let said: Vec<&String> = ran.iter().filter(|word| word.starts_with("hooks=")).collect();
    assert_eq!(said.len(), 1, "{ran:?}");
    // A TOML string that holds nothing but these escapes is a JSON one too.
    let text = |rest: &str| -> (String, usize) {
        let mut read = serde_json::Deserializer::from_str(rest).into_iter::<String>();
        (read.next().expect("a string").unwrap(), read.byte_offset())
    };
    let before = "hooks={PreToolUse=[{hooks=[{type=\"command\",command=";
    let rest = said[0].strip_prefix(before).expect("the guard, before each tool call");
    let (guard, read) = text(rest);
    let beside = "},{type=\"command\",command=";
    let (report, _) = text(rest[read..].strip_prefix(beside).expect("the report, beside it"));
    (guard, report)
}

/// What the shell line `hook` answers Codex for `event`, run as Codex runs a
/// hook — by a shell, in the environment of the worker `line` started, with
/// the event described on standard input: its exit code, and what it said on
/// stdout and on stderr. Of the host's environment a worker has its PATH,
/// and the home `home` where one is given.
fn hook_answers(
    line: &str,
    hook: &str,
    home: Option<&Path>,
    event: &Value,
) -> (i32, String, String) {
    use std::io::Write;
    let (worker, _) = line.split_once(" && codex ").expect("the stock command");
    let mut hook_run = Command::new("/bin/sh");
    hook_run
        .args(["-c", &format!("{worker} && exec /bin/sh -c \"$STAND_IN_HOOK\"")])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("STAND_IN_HOOK", hook);
    if let Some(home) = home {
        hook_run.env("HOME", home);
    }
    let mut child = hook_run
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(event.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let said = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap().trim().to_string();
    (out.status.code().unwrap(), said(out.stdout), said(out.stderr))
}

/// What the guard of the worker `line` started answers Codex for `call`: its
/// exit code and what it said.
fn codex_is_told(world: &World, line: &str, call: &Value) -> (i32, String) {
    let (guard, _) = hooked(&under_codex(world, line));
    let (code, out, said) = hook_answers(line, &guard, None, call);
    assert!(out.is_empty(), "a hook says nothing to Codex but on stderr");
    (code, said)
}

/// What the report of the worker `line` started answers Codex for `event`,
/// on this world's host.
fn report_answers(world: &World, line: &str, event: &Value) -> (i32, String, String) {
    let (_, report) = hooked(&under_codex(world, line));
    hook_answers(line, &report, Some(&world.home()), event)
}

/// Installs a stand-in for the hook orca installs for Codex on this world's
/// host: it keeps each event it is handed, and then does `then`.
fn orca_installs_its_hook(world: &World, then: &str) {
    use std::os::unix::fs::PermissionsExt;
    let hook = world.home().join(".orca/agent-hooks/codex-hook.sh");
    let kept = "#!/bin/sh\n{ cat; echo; } >>\"$HOME/told\"\n";
    write(&world.home(), ".orca/agent-hooks/codex-hook.sh", &format!("{kept}{then}\n"));
    std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The events the stand-in for orca's hook was handed since the last look.
fn orca_was_told(world: &World) -> Vec<String> {
    let kept = world.home().join("told");
    let events = std::fs::read_to_string(&kept).unwrap_or_default();
    let _ = std::fs::remove_file(kept);
    let events = events.lines().filter(|event| !event.is_empty());
    let named = events.map(|event| serde_json::from_str::<Value>(event).unwrap());
    named.map(|event| event["hook_event_name"].as_str().unwrap().to_string()).collect()
}

/// One event of a Codex session, as Codex describes it to a hook.
fn event(name: &str) -> Value {
    json!({"hook_event_name": name, "session_id": "a-session"})
}

fn bash(line: &str) -> Value {
    json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": line}})
}

#[test]
fn every_tool_call_of_a_codex_worker_is_put_to_the_guard_and_one_it_refuses_is_not_made() {
    let world = bound();
    world.issue(7, &["flow:audit"]);
    world.issue(8, &["flow:feature"]);
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[..2], ["dispatch 7 done", "dispatch 8 done"], "{ticked}");
    let (reviews, writes) =
        (started(&world, &worktree(&world, 7)), started(&world, &worktree(&world, 8)));
    // The hook is on the line and in no file: no worker starts without it,
    // whatever its role's repo and the fleet's directory hold.
    assert!(!kept_host_user(&world).join("hooks.json").exists());

    // What every worker is refused, and — in a stage that writes nothing — a
    // commit: each refused as Codex takes a refusal, with the guard's reason.
    let every = [
        "gh pr merge 70 --squash",
        "git push origin main",
        "gh issue edit 7 --add-label flow:feature",
        "gh issue edit 7 --remove-label flow:audit",
    ];
    let commit = "git add -A && git commit -m step";
    for (line, refused) in
        [(&writes, &every[..]), (&reviews, &[&every[..], &[commit]].concat()[..])]
    {
        for shell in refused {
            let (code, said) = codex_is_told(&world, line, &bash(shell));
            assert_eq!(code, 2, "{shell}: {said}");
            assert!(said.starts_with("afkconvoy guard: "), "{shell}: {said}");
            assert_eq!(said.lines().count(), 1, "{shell}: {said}");
        }
        for shell in ["ls", "git status"] {
            assert_eq!(codex_is_told(&world, line, &bash(shell)), (0, String::new()), "{shell}");
        }
    }
    assert_eq!(codex_is_told(&world, &writes, &bash(commit)), (0, String::new()));
    let (_, said) = codex_is_told(&world, &reviews, &bash(commit));
    assert!(said.contains("the stage `look` is marked `writes: false`: `git commit`"), "{said}");

    // A patch's text is no shell line, whatever it holds; nor is what any
    // other tool is handed.
    let patch = "*** Begin Patch\n*** Add File: a.sh\n+git push origin main\n*** End Patch\n";
    for other in [
        json!({"tool_name": "apply_patch", "tool_input": {"command": patch}}),
        json!({"tool_name": "mcp__shell__run", "tool_input": {"command": "git push origin main"}}),
        json!({"tool_name": "update_plan", "tool_input": {"plan": []}}),
    ] {
        assert_eq!(codex_is_told(&world, &reviews, &other), (0, String::new()), "{other}");
    }
}

#[test]
fn a_tool_call_is_refused_when_the_guard_cannot_be_reached() {
    let world = bound();
    world.issue(8, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 8 done");
    let line = started(&world, &worktree(&world, 8));
    let guard = format!("$({} guard ", env!("CARGO_BIN_EXE_afkconvoy"));
    assert!(line.contains(&guard), "{line}");
    // Codex makes a call whose hook fails in any way but one: each of these
    // ends in that one — the exit code that refuses, and a reason.
    let refused = |(from, with): (&str, &str), why: &str| {
        assert!(line.contains(from), "{from}: {line}");
        for call in [bash("ls"), json!({"tool_name": "apply_patch", "tool_input": {}})] {
            let (code, said) = codex_is_told(&world, &line.replace(from, with), &call);
            assert_eq!(code, 2, "{with}: {said}");
            assert!(said.contains(why), "{with}: {said}");
        }
    };
    // A guard that is not there, one that fails saying nothing, one that
    // cannot read the stage, and one that never answers its name.
    refused((&guard, "$(/nowhere/afkconvoy guard "), "/nowhere/afkconvoy");
    refused((&guard, "$(exit 1 "), "afkconvoy guard: it could not be asked");
    refused((&guard, "$(exit 2 "), "afkconvoy guard: it could not be asked");
    refused(("AFKCONVOY_STAGE='build' ", ""), "AFKCONVOY_STAGE is not set");
    refused(("AFKCONVOY_HARNESS='codex'", "AFKCONVOY_HARNESS='aider'"), "`aider` is no harness");
    // What is no description of a tool call is refused too.
    let (code, said) = codex_is_told(&world, &line, &json!({"toolName": "bash"}));
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("describes no tool call"), "{said}");
}

/// Every hook the role a worker started in holds for Codex at `event`, as
/// Codex reads them: from `.codex/hooks.json` in the directory it starts in.
fn role_s_hooks(ran: &[String], event: &str) -> Vec<String> {
    let held = std::fs::read_to_string(Path::new(&ran[0]).join(".codex/hooks.json")).unwrap();
    let held: Value = serde_json::from_str(&held).unwrap();
    let groups = held["hooks"][event].as_array().cloned().unwrap_or_default();
    let hooks = groups.iter().flat_map(|group| group["hooks"].as_array().unwrap().clone());
    hooks.map(|hook| hook["command"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_hook_of_a_role_s_limits_for_codex_runs_beside_the_guard_and_never_in_its_place() {
    let world = bound();
    // The role's limits for Codex, in Codex's form: one hook before each tool
    // call, which refuses a `curl`, and one as a session ends.
    let dev = world.root.join("github/acme/dev");
    let limits = json!({"hooks": {
        "PreToolUse": [{"matcher": "Bash", "hooks": [
            {"type": "command", "command": "sh .codex/hooks/refuse.sh"}]}],
        "Stop": [{"hooks": [{"type": "command", "command": "echo \"$AFKCONVOY_STAGE\" is over"}]}],
    }});
    write(
        &dev,
        ".codex/hooks/refuse.sh",
        "case \"$(cat)\" in *curl*) echo no curl here >&2; exit 2 ;; esac\n",
    );
    write(&dev, ".codex/hooks.json", &limits.to_string());
    git(&dev, &["add", "."]);
    git(&dev, &["commit", "-q", "-m", "its limits for Codex"]);
    assert_eq!(world.afk(&["check"]).ok()["roles"][0]["limits"], json!(["codex"]));

    world.issue(7, &["flow:feature"]);
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[0], "dispatch 7 done", "{ticked}");
    let line = started(&world, &worktree(&world, 7));
    let role = role_copy(&world, "acme/dev");

    // Codex loads a directory's hooks only where it trusts the directory and
    // its hooks were reviewed: both are said on the line, of the role's copy,
    // which is where the worker starts and holds the role's file as pushed.
    let ran = under_codex(&world, &line);
    assert_eq!(ran[0], role.to_str().unwrap());
    let trusted = format!("projects={{\"{}\"={{trust_level=\"trusted\"}}}}", role.display());
    assert!(ran.contains(&trusted) && ran.contains(&"--dangerously-bypass-hook-trust".to_string()));
    let held = std::fs::read_to_string(role.join(".codex/hooks.json")).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&held).unwrap(), limits);

    // The guard is said beside it, as configuration of the one run: Codex
    // adds that to the hooks a directory holds, and the fleet writes it into
    // no file — not the role's, and none of the user directory's.
    let (guard, report) = hooked(&ran);
    assert!(!guard.contains("refuse.sh") && !report.contains("refuse.sh"));
    assert!(!held.contains("afkconvoy"), "{held}");
    assert!(!kept_host_user(&world).join("hooks.json").exists());
    assert!(!kept_host_user(&world).join("config.toml").exists());

    // Each is run as Codex runs a hook: by a shell, in the directory the
    // worker started in and in its environment, with the call on stdin.
    let (worker, _) = line.split_once(" && codex ").expect("the stock command");
    let role_s = |event: &str, call: &Value| -> Vec<(i32, String)> {
        use std::io::Write;
        let run = |hook: String| {
            let mut child = Command::new("/bin/sh")
                .args(["-c", &format!("{worker} && exec /bin/sh -c \"$STAND_IN_HOOK\"")])
                .env_clear()
                .env("PATH", std::env::var("PATH").unwrap())
                .env("STAND_IN_HOOK", hook)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(call.to_string().as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            let said = [out.stdout, out.stderr].concat();
            (out.status.code().unwrap(), String::from_utf8(said).unwrap().trim().to_string())
        };
        role_s_hooks(&ran, event).into_iter().map(run).collect()
    };
    // What the role refuses and the guard does not is refused by the role's…
    let fetch = bash("curl https://example.com");
    assert_eq!(role_s("PreToolUse", &fetch), [(2, "no curl here".to_string())]);
    assert_eq!(codex_is_told(&world, &line, &fetch), (0, String::new()));
    // …what the guard refuses and the role does not, by the guard…
    let push = bash("git push origin main");
    assert_eq!(role_s("PreToolUse", &push), [(0, String::new())]);
    let (code, said) = codex_is_told(&world, &line, &push);
    assert_eq!(code, 2, "{said}");
    assert!(said.starts_with("afkconvoy guard: "), "{said}");
    // …and a hook at another event reads the stage's facts as any limit does.
    assert_eq!(role_s("Stop", &json!({})), [(0, "build is over".to_string())]);
}

#[test]
fn a_stage_record_of_a_codex_worker_names_codex() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let dir = worktree(&world, 7);
    let line = started(&world, &dir);
    let git_dir = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    let brief = std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap();
    let declare = brief.lines().find(|line| line.contains(" verdict --issue 7")).unwrap();
    // The worker declares with the line its brief gives it, in the
    // environment the start gave it.
    let (worker, _) = line.split_once(" && codex ").unwrap();
    let ran = world
        .command("sh", &world.root)
        .args(["-c", &format!("{worker} && {declare} pass --text built")])
        .output()
        .unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let comments = world.comments(7);
    let record = comments.iter().find(|comment| comment.contains(" stage=build ")).unwrap();
    assert!(record.contains(" verdict=pass ") && record.contains(" harness=codex-->"), "{record}");
    let shown = world.afk(&["show", "--issue", "7"]).ok();
    assert_eq!(shown["records"][0]["harness"], "codex", "{shown}");
}

#[test]
fn a_codex_worker_s_start_names_orca_s_hook_wherever_its_state_changes_and_it_refuses_nothing() {
    let world = bound();
    world.issue(8, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 8 done");
    let line = started(&world, &worktree(&world, 8));
    let ran = under_codex(&world, &line);
    let (_, report) = hooked(&ran);
    assert_eq!(report, REPORT);
    // At each event orca itself names its hook at, with the timeout orca
    // gives it; the guard, which must answer, has none.
    let said = ran.iter().find(|word| word.starts_with("hooks=")).unwrap();
    let named = format!("command={},timeout=10}}", serde_json::to_string(REPORT).unwrap());
    for event in [&["PreToolUse"][..], &STATE_EVENTS[..]].concat() {
        let at = said.split_once(&format!("{event}=[{{hooks=[")).expect(event).1;
        let hooks = at.split_once("]}]").unwrap().0;
        assert!(hooks.ends_with(&named), "{event}: {hooks}");
    }
    assert_eq!(said.matches("timeout=").count(), 8, "{said}");

    // On a host where orca installed no hook for Codex nothing is told, and
    // nothing fails: the event is read, and Codex is answered nothing.
    let events = [&["PreToolUse"][..], &STATE_EVENTS[..]].concat();
    let nothing = (0, String::new(), String::new());
    for name in &events {
        assert_eq!(report_answers(&world, &line, &event(name)), nothing, "{name}");
    }
    assert!(!world.home().join(".orca").exists());

    // Where it installed one, the hook is handed each event — and whatever it
    // says or ends in, the report says nothing and refuses nothing: Codex
    // would take a hook's exit code, or what it prints, for an answer.
    for then in ["", "echo '{\"decision\":\"block\"}'; echo refused >&2; exit 2", "kill -9 $$"] {
        orca_installs_its_hook(&world, then);
        for name in &events {
            assert_eq!(report_answers(&world, &line, &event(name)), nothing, "{name}: {then}");
        }
        assert_eq!(orca_was_told(&world), events, "{then}");
    }
    // The call the report let through is still the guard's to refuse.
    let (code, why) = codex_is_told(&world, &line, &bash("git push origin main"));
    assert_eq!(code, 2, "{why}");
}

/// orca's part of a report: what it has of the worker's terminal once its
/// hook was handed the event `heard`, at `at` — a session that stopped is
/// done, and any other event is of one at work.
fn orca_heard(world: &World, heard: &str, at: u64) {
    let state = if heard == "Stop" { "done" } else { "working" };
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                terminal["agent"] = json!({"state": state, "stateStartedAt": at * 1000});
            }
        }
    });
}

/// The worker's terminal last wrote at `at`.
fn wrote(world: &World, at: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                terminal["lastOutputAt"] = json!(at * 1000);
            }
        }
    });
}

#[test]
fn a_codex_worker_that_is_working_is_seen_as_busy_and_one_that_has_stopped_as_stopped() {
    let world = bound();
    orca_installs_its_hook(&world, "");
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let dir = worktree(&world, 7);
    let line = started(&world, &dir);
    let sent = || {
        let orca = world.orca();
        let terminals = orca["terminals"].as_array().unwrap();
        let here = terminals.iter().find(|t| t["worktreePath"] == dir.to_str().unwrap()).unwrap();
        here["sent"].as_array().unwrap().len()
    };
    // What its hooks hand orca's is all orca hears of the session: nothing
    // else of the worker's start names orca.
    let heard = |name: &str, at: u64| {
        assert_eq!(report_answers(&world, &line, &event(name)).0, 0, "{name}");
        let told = orca_was_told(&world);
        assert_eq!(told, [name]);
        orca_heard(&world, &told[0], at);
    };

    // Handed its brief and at its tools: at work, and left alone.
    heard("UserPromptSubmit", 1000);
    heard("PostToolUse", 1395);
    wrote(&world, 1395);
    assert_eq!(did(&tick(&world, 1400).ok())[0], "advance 7 nothing");
    assert_eq!(sent(), 1, "the brief alone");

    // Stopped, with no verdict: seen as stopped though its terminal wrote a
    // moment ago — which would read as at work, had orca heard nothing — and
    // nudged, once, as any other worker.
    heard("Stop", 1400);
    wrote(&world, 1695);
    assert_eq!(did(&tick(&world, 1700).ok())[0], "advance 7 nudged");
    assert_eq!(sent(), 2, "the brief, then the nudge");

    // At work again on the nudge: busy, and left alone.
    heard("UserPromptSubmit", 1700);
    wrote(&world, 1780);
    assert_eq!(did(&tick(&world, 1790).ok())[0], "advance 7 nothing");
    assert_eq!(sent(), 2);
}

/// The entries of the directory `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let entries = std::fs::read_dir(dir).unwrap().flatten();
    let mut names: Vec<String> =
        entries.map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn a_codex_worker_finds_its_role_s_skills_through_a_link_in_the_fleet_s_copy() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let line = started(&world, &worktree(&world, 7));
    let (role, repo) = (role_copy(&world, "acme/dev"), copy(&world, "acme/dev"));
    assert!(line.starts_with(&format!("cd '{}' && ", role.display())), "{line}");

    // Codex finds skills in `.agents/skills` of the directory it starts in:
    // there, the copy links to the role's skills, every one of them.
    let link = role.join(".agents/skills");
    assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("../.claude/skills"));
    assert_eq!(entries(&link), entries(&repo.join(".claude/skills")));
    assert_eq!(std::fs::read_to_string(link.join("implement/SKILL.md")).unwrap(), IMPLEMENT);
    // The role's repo holds nothing for Codex, and neither does the copy of
    // it another harness's worker starts in.
    assert_eq!(entries(&repo), [".claude", "CLAUDE.md"]);
    assert_eq!(entries(&role), [".agents", ".claude", "CLAUDE.md"]);
    // The skills Codex carries are left out, on the line.
    let ran = under_codex(&world, &line);
    assert!(ran.contains(&"skills.bundled.enabled=false".to_string()), "{ran:?}");

    // A second worker is started from the same copy, which is as it was.
    world.issue(8, &["flow:feature"]);
    let ticked = did(&tick(&world, 1100).ok());
    assert!(ticked.contains(&"dispatch 8 done".to_string()), "{ticked:?}");
    let again = started(&world, &worktree(&world, 8));
    assert!(again.starts_with(&format!("cd '{}' && ", role.display())), "{again}");
    assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("../.claude/skills"));
    let head = head(&world, "acme/dev");
    assert_eq!(entries(role.parent().unwrap()), [head.clone(), format!("{head}-role+codex-")]);
}

#[test]
fn a_role_that_holds_skills_for_codex_itself_is_left_as_it_is_and_noted_not_refused() {
    let world = bound();
    let dev = world.root.join("github/acme/dev");
    write(&dev, ".agents/skills/tidy/SKILL.md", "---\nname: tidy\ndescription: tidy\n---\n");
    git(&dev, &["add", "."]);
    git(&dev, &["commit", "-q", "-m", "a skill for Codex alone"]);

    // `check` passes, and notes each worker on Codex whose skill Codex would
    // not find: the fleet adds nothing where the role holds a directory.
    let checked = world.afk(&["check"]);
    let notes: Vec<&str> =
        checked.stderr.lines().filter(|line| line.contains("holds skills of its own")).collect();
    assert_eq!(notes.len(), 2, "{}", checked.stderr);
    assert!(
        notes[0].starts_with(
            "note: the stage `build` of the workflow `feature` runs on `codex`, and its role \
             `dev` holds skills of its own for Codex (`.agents/skills/` or `.codex/skills/`), \
             `implement` not among them: "
        ),
        "{}",
        notes[0]
    );
    assert!(notes[1].starts_with("note: the patrol `sweep` runs on `codex`"), "{}", notes[1]);
    checked.ok();

    // A worker starts all the same, in the copy every harness's worker
    // starts in: what the role holds is as it was pushed.
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let role = copy(&world, "acme/dev");
    let line = started(&world, &worktree(&world, 7));
    assert!(line.starts_with(&format!("cd '{}' && ", role.display())), "{line}");
    assert!(role.join(".agents/skills").symlink_metadata().unwrap().is_dir());
    assert_eq!(entries(&role.join(".agents/skills")), ["tidy"]);
    assert!(!role_copy(&world, "acme/dev").exists());

    // The skill held where Codex finds it, in either directory: nothing is noted.
    write(
        &dev,
        ".codex/skills/implement/SKILL.md",
        "---\nname: implement\ndescription: build\n---\n",
    );
    git(&dev, &["add", "."]);
    git(&dev, &["commit", "-q", "-m", "the skill, for Codex"]);
    let checked = world.afk(&["check"]);
    assert!(!checked.stderr.contains("holds skills of its own"), "{}", checked.stderr);
    checked.ok();
}

#[test]
fn a_codex_worker_is_given_the_host_user_s_skills_and_rules_and_its_role_s_skill_wins() {
    let world = bound();
    // What this host's user has of Codex's: a login, rules, a configuration,
    // hooks, a skill of their own, and one named as a skill of the role.
    let home = world.home();
    let skill = |name: &str| format!("---\nname: {name}\ndescription: theirs\n---\n");
    for (file, text) in [
        (".codex/auth.json", "{\"a\": \"login\"}\n".to_string()),
        (".codex/AGENTS.md", "the user's rules\n".to_string()),
        (".codex/config.toml", "model = \"theirs\"\n".to_string()),
        (".codex/hooks.json", "{\"hooks\": {}}\n".to_string()),
        (".codex/skills/theirs/SKILL.md", skill("theirs")),
        (".codex/skills/implement/SKILL.md", skill("implement")),
        (".agents/skills/implement/SKILL.md", skill("implement")),
    ] {
        write(&home, file, &text);
    }
    world.issue(7, &["flow:feature"]);
    let ticked = tick(&world, 1000).ok();
    assert_eq!(did(&ticked)[0], "dispatch 7 done", "{ticked}");

    // Its user directory is the one kept with the host user's skills and
    // rules: linked to, where Codex finds a user's, beside the login — and
    // still no configuration and no hook of theirs.
    let (alone, kept) = (kept(&world), kept_host_user(&world));
    let mut held: Vec<String> = std::fs::read_dir(&kept)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    held.sort();
    assert_eq!(held, ["AGENTS.md", "auth.json", "skills"]);
    let link = |name: &str| std::fs::read_link(kept.join(name)).unwrap();
    assert_eq!(link("AGENTS.md"), home.join(".codex/AGENTS.md"));
    assert_eq!(link("skills"), home.join(".codex/skills"));
    assert!(!alone.exists(), "nothing is kept without them");

    // The worker starts under it, and the host user's `implement` is turned
    // off wherever Codex would find it: the role's is the one it is shown.
    let ran = under_codex(&world, &started(&world, &worktree(&world, 7)));
    assert_eq!(ran[1], kept.to_str().unwrap());
    let off: Vec<&String> = ran.iter().filter(|word| word.starts_with("skills.config=")).collect();
    let [off] = off[..] else { panic!("one key turns skills off: {ran:?}") };
    for file in [
        home.join(".agents/skills/implement/SKILL.md"),
        home.join(".codex/skills/implement/SKILL.md"),
        kept.join("skills/implement/SKILL.md"),
    ] {
        let entry = format!("{{path=\"{}\",enabled=false}}", file.display());
        assert!(off.contains(&entry), "{entry}\n{off}");
    }
    assert!(!off.contains("theirs"), "{off}");
}

#[test]
fn a_codex_worker_of_a_role_that_keeps_from_the_host_user_s_skills_and_rules_is_given_none() {
    let own = "reviewer: { dir: acme/reviewer, host_user: false }";
    let world = bound_with(&TEAM.replace("reviewer: acme/reviewer", own));
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:audit"]);
    let ticked = tick(&world, 1000).ok();
    let did = did(&ticked);
    assert!(did.contains(&"dispatch 7 done".to_string()), "{ticked}");
    assert!(did.contains(&"dispatch 8 done".to_string()), "{ticked}");
    // `dev` takes the team's, and is given them; `reviewer` says otherwise.
    let given = |issue: u64| {
        let line = started(&world, &worktree(&world, issue));
        let (with, alone) = (
            format!("'{}'", kept_host_user(&world).display()),
            format!("'{}'", kept(&world).display()),
        );
        assert_ne!(line.contains(&with), line.contains(&alone), "{line}");
        line.contains(&with)
    };
    assert!(given(7), "dev is started on Codex with the host user's");
    assert!(!given(8), "reviewer is started on Codex with none of the host user's");
}

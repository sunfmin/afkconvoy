//! A bound repo's worker starts in its role: the fleet keeps a read-only copy
//! of the team and of each role per commit, opens the terminal on the issue's
//! worktree as ever, and has it change to the role's copy and start the
//! worker launch command with a fixed set added (docs/adr/0009).

use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git, write};
use serde_json::{Value, json};

/// A team whose `dev` runs opus; `reviewer` runs what the host kit holds.
const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\nroster:\n  \
                    dev: { run: [{ harness: claude-code, model: opus }] }\n```\n";
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
/// A workflow whose first stage reviews, and writes nothing.
const AUDIT: &str = "\
```yaml
stages:
  - name: look
    by: reviewer
    do: review
    writes: false
    on: { pass: done }
```
";
const CONFIG: &str = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
/// Claude Code's stock command: unattended, and its warning answered.
const STOCK: &str = r#"claude --dangerously-skip-permissions --settings '{"skipDangerousModePermissionPrompt":true}'"#;

/// A world whose repo is bound to a team of two roles and two workflows.
/// The file of the skill `implement`.
const IMPLEMENT: &str = "---\nname: implement\ndescription: build\n---\n";

fn bound() -> World {
    let world = World::new();
    world.kit_command("claude-code", afkconvoy_core::harness::Harness::ClaudeCode.stock());
    world.kit_models("claude-code", &["opus"]);
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
        &[("team.md", TEAM), ("workflows/feature.md", FEATURE), ("workflows/audit.md", AUDIT)],
    );
    write(&world.clone, "docs/agents/afkconvoy.md", CONFIG);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

#[test]
fn a_claude_worker_is_proved_by_its_role_session_even_when_orca_reports_no_turn() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    let role = copy(&world, "acme/dev", &head(&world, "acme/dev"));
    world.with_orca(|orca| {
        orca["claude_session_cwd"] = json!(role);
        orca["session_turn"] = json!(true);
        orca["raw_answers"] = json!({"terminal send": json!({"ok": false,
            "error": {"code": "agent_prompt_blocked", "details": {
                "reason": "the line started no turn within 30s",
                "prompt": {"provider": "claude", "observation": "supported",
                    "stages": ["input_accepted"]}}}}).to_string()});
    });
    let answer = dispatch(&world, 7).ok();
    assert_eq!(answer["started"], true, "{answer}");
    let terminals = open_terminals(&world, &worktree(&world, 7));
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["sent"].as_array().unwrap().len(), 1, "the brief is never resent");
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

fn dispatch(world: &World, issue: u64) -> common::Answer {
    let issue = issue.to_string();
    world.afk(&["dispatch", "--issue", &issue, "--instance", "alpha", "--host", "h1"])
}

/// The directory the fleet keeps for Claude Code on this world's host, with
/// the host user's skills and rules: added to a worker of a team that does
/// not turn them off.
fn host_user(world: &World) -> PathBuf {
    world.cache().join("afkconvoy/harness/claude-code-host-user")
}

fn head(world: &World, repo: &str) -> String {
    git(&world.root.join("github").join(repo), &["rev-parse", "HEAD"])
}

/// Where the fleet keeps its copy of `repo` at `commit`.
fn copy(world: &World, repo: &str, commit: &str) -> PathBuf {
    let name = if repo == "acme/team" {
        commit.to_string()
    } else {
        format!("{commit}-role+claude-code-")
    };
    world.cache().join("afkconvoy/copies").join(repo).join(name)
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// The terminals open in `dir`, as the stand-in orca has them.
fn open_terminals(world: &World, dir: &Path) -> Vec<Value> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    here.filter(|t| t["open"] == true).cloned().collect()
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

/// The repos `gh` was asked to clone since the last look.
/// The settings a start adds the guard with: a hook that runs this binary.
/// The settings the worker started by `command` was given beside its role's:
/// the guard before every tool call, orca told wherever its state changes, and
/// the warning Claude Code would stop at answered.
fn added_settings(command: &str) -> String {
    // The last `--settings` is the one Claude Code takes: the fleet's.
    let added = command.rsplit(" --settings '").next().unwrap().split('\'').next().unwrap();
    let settings: Value = serde_json::from_str(added).unwrap();
    let hooks = |event: &str| settings["hooks"][event][0]["hooks"].clone();
    let guard = "afkconvoy guard";
    assert_eq!(hooks("PreToolUse")[0], json!({"type": "command", "command": guard}));
    for event in ["PreToolUse", "SessionStart", "UserPromptSubmit", "PostToolUse", "Stop"] {
        let told = hooks(event);
        let report = told.as_array().unwrap().last().unwrap();
        let line = report["command"].as_str().unwrap();
        assert!(line.contains("/.orca/agent-hooks/claude-hook.sh"), "{event}: {settings}");
        assert_eq!(report["timeout"], 10, "{event}: {settings}");
    }
    assert_eq!(settings["skipDangerousModePermissionPrompt"], true, "{settings}");
    added.to_string()
}

/// What the guard a worker was started with by `command` answers the Bash
/// call `line`: the hook its settings name, run as Claude Code runs one — in
/// the worker's directory and environment, with the world's PATH that finds
/// its `afkconvoy`, the call described on stdin → its exit code and what it
/// said on stderr.
fn guarded(world: &World, command: &str, line: &str) -> (i32, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut steps = command.split(" && ");
    let (enter, facts, start) = (steps.next().unwrap(), steps.next().unwrap(), steps.next());
    let settings = start.unwrap().rsplit(" --settings '").next().unwrap();
    let settings: Value = serde_json::from_str(settings.split('\'').next().unwrap()).unwrap();
    let hooks = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(hooks.len(), 1, "{settings}");
    assert!(hooks[0]["matcher"].is_null(), "it is run before every tool: {settings}");
    let hook = hooks[0]["hooks"][0]["command"].as_str().unwrap();
    let mut child = Command::new("/bin/sh")
        .args(["-c", &format!("{enter} && {facts} && {hook}")])
        .env_clear()
        .envs(world.login_env())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let described = json!({"tool_name": "Bash", "tool_input": {"command": line}});
    child.stdin.take().unwrap().write_all(described.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stderr).unwrap())
}

fn cloned(world: &World) -> Vec<String> {
    let calls = world.gh_calls();
    let clones = calls.iter().filter(|call| call[..2] == ["repo", "clone"]);
    clones.map(|call| call[2].clone()).collect()
}

fn is_read_only(path: &Path) -> bool {
    std::fs::metadata(path).unwrap().permissions().readonly()
}

#[test]
fn a_flow_labelled_issue_is_dispatched_to_its_first_stage_with_the_worker_in_its_role() {
    let world = bound();
    world.issue(7, &["flow:feature", "bug"]);
    world.issue(8, &["ready-for-agent"]);
    world.orca_calls();

    let ticked = tick(&world, 1000).ok();
    assert_eq!(
        did(&ticked),
        ["dispatch 7 done", "stage_label 7 done", "heartbeat done"],
        "{ticked}"
    );
    assert_eq!(world.claims(), [(7, "alpha".to_string())], "no flow label: left alone");

    // The copies: the role's and team's files, and no Git metadata. The
    // role's empty marker stops Claude's ancestor skill discovery.
    let (dev, team) = (head(&world, "acme/dev"), head(&world, "acme/team"));
    let (role_dir, team_dir) = (copy(&world, "acme/dev", &dev), copy(&world, "acme/team", &team));
    assert_eq!(
        std::fs::read_to_string(role_dir.join("CLAUDE.md")).unwrap(),
        "the rules of a dev\n"
    );
    assert!(role_dir.join(".claude/skills/implement/SKILL.md").is_file());
    assert!(team_dir.join("workflows/feature.md").is_file());
    assert!(role_dir.join(".git").is_dir());
    assert_eq!(std::fs::read_dir(role_dir.join(".git")).unwrap().count(), 0);
    for kept in
        [&role_dir, &role_dir.join("CLAUDE.md"), &role_dir.join(".claude/skills"), &team_dir]
    {
        assert!(is_read_only(kept), "{}", kept.display());
    }
    assert!(std::fs::write(role_dir.join("CLAUDE.md"), "edited").is_err());
    assert!(std::fs::write(role_dir.join("new.md"), "added").is_err());

    // The terminal is opened on the issue's worktree, as ever…
    let dir = worktree(&world, 7);
    let terminals = open_terminals(&world, &dir);
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    let calls = world.orca_calls();
    let created = calls.iter().find(|call| common::asked(call) == "terminal create").unwrap();
    assert_eq!(created[2..4], ["--worktree".to_string(), format!("path:{}", dir.display())]);
    // …and what it runs changes to the role's copy first, then adds a fixed set.
    let command = format!(
        "cd '{role}' && export CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 \
         CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 CLAUDE_CODE_SANDBOXED=1 \
         AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
         AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
         AFKCONVOY_WRITES='true' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
         AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
         AFKCONVOY_ROLE_COMMIT='{dev}' AFKCONVOY_TEAM_COMMIT='{team}' \
         AFKCONVOY_HARNESS='claude-code' && \
         {STOCK} --add-dir '{worktree}' --add-dir '{team_dir}' --add-dir '{host_user}' \
         --setting-sources project --settings '{guard}'",
        guard = added_settings(terminals[0]["command"].as_str().unwrap()),
        role = role_dir.display(),
        worktree = dir.display(),
        team_dir = team_dir.display(),
        host_user = host_user(&world).display(),
    );
    assert_eq!(terminals[0]["command"], command);
    assert_eq!(created[4..], ["--command".to_string(), common::niced(&world, &dir, &command)]);

    // The brief is a fixed form, and the worker is pointed at it.
    let brief = brief(&dir);
    // The team has no rules: the brief has no line for them, and the worker
    // is started as it is under a team that has them.
    assert!(!team_dir.join("CLAUDE.md").exists() && !brief.contains("Team rules"), "{brief}");
    assert!(brief.contains("- **Your role's rules:** the rules file of the directory you were"));
    let branch = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    for said in [
        "- **Issue:** #7 — issue 7\n".to_string(),
        "- **Stage:** `build`, of the workflow `feature`\n".to_string(),
        "- **Skill to perform:** `implement`, of the role you were started in (`dev`)\n".into(),
        format!(
            "- **Worktree:** `{}`, on the branch `{branch}` (the base is `main`)",
            dir.display()
        ),
        "- **History:** the issue and every comment on it (`gh issue view 7 --comments`)".into(),
        " verdict --issue 7 --stage 'build'\n```".to_string(),
    ] {
        assert!(brief.contains(&said), "{said}\n{brief}");
    }
    for how in ["Implement", "gh pr create", "Join the landing train", "git commit"] {
        assert!(!brief.contains(how), "the brief says nothing of how: {how}");
    }
    // But for the gate: a stage that may commit hands on no red PR.
    let gate = format!("   cd '{}' && afkconvoy gate\n", dir.display());
    assert!(brief.contains("## The gate, before a `pass`\n"), "{brief}");
    assert!(brief.contains(&gate), "{gate}\n{brief}");
    let sent = terminals[0]["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0]["text"].as_str().unwrap().contains("convoy-worker-prompt.md"));
    assert_eq!(sent[0]["enter"], true);
}

#[test]
fn a_dispatch_answers_with_the_stage_and_the_commits_fixed_for_it() {
    let world = bound();
    world.issue(7, &["flow:audit"]);
    let started = dispatch(&world, 7).ok();
    let (reviewer, team) = (head(&world, "acme/reviewer"), head(&world, "acme/team"));
    assert_eq!(started["prompt"], "stage");
    assert_eq!(
        started["stage"],
        json!({"flow": "audit", "name": "look", "role": "reviewer", "skill": "review",
            "writes": false, "consult": false, "harness": "claude-code", "model": null,
            "skipped": [], "role_commit": reviewer, "role_dir": copy(&world, "acme/reviewer", &reviewer),
            "team_commit": team, "team_dir": copy(&world, "acme/team", &team)})
    );
    // A role the roster names no model for runs the user default, and its
    // stage says it writes nothing. A range that holds one model does not
    // name it.
    let command = open_terminals(&world, &worktree(&world, 7))[0]["command"].clone();
    let command = command.as_str().unwrap();
    // …but never without the guard: the reviewer's repo holds no hook of its own.
    let guarded = format!("--setting-sources project --settings '{}'", added_settings(command));
    assert!(command.ends_with(&guarded), "{command}");
    assert!(command.contains(" AFKCONVOY_STAGE='look' AFKCONVOY_ROLE='reviewer' "), "{command}");
    assert!(command.contains(" AFKCONVOY_WRITES='false' "), "{command}");
}

#[test]
fn every_worker_is_started_under_the_guard_whatever_its_role_s_repo_holds() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:audit"]);
    dispatch(&world, 7).ok();
    dispatch(&world, 8).ok();
    // Neither role's repo holds a hook: the guard is the fleet's to add.
    let command = |issue: u64| {
        let started = open_terminals(&world, &worktree(&world, issue));
        started[0]["command"].as_str().unwrap().to_string()
    };
    let (build, look) = (command(7), command(8));
    for worker in [&build, &look] {
        for (line, reason) in [
            ("gh issue edit 7 --remove-label flow:feature", "is a flow label"),
            ("gh issue edit 7 --add-label ready-for-human", "is the escalate label"),
            ("gh issue edit 7 --add-label fleet-lock", "is the lock label"),
            ("gh pr merge 70", "merging a PR is refused"),
            ("git push origin HEAD:main", "pushes the base branch `main`"),
        ] {
            let (code, said) = guarded(&world, worker, line);
            assert_eq!(code, 2, "{line}: {said}");
            assert!(said.contains(reason), "{line}: {said}");
        }
        assert_eq!(guarded(&world, worker, "cargo test").0, 0);
    }
    // The stage `build` may commit; `look` is marked `writes: false`.
    let committing = "git commit -am step && git push origin HEAD";
    assert_eq!(guarded(&world, &build, committing), (0, String::new()));
    let (code, said) = guarded(&world, &look, committing);
    assert_eq!(code, 2);
    assert!(said.contains("the stage `look` is marked `writes: false`"), "{said}");
}

#[test]
fn a_second_dispatch_at_the_same_commit_fetches_nothing() {
    let world = bound();
    for issue in [7, 8, 9] {
        world.issue(issue, &["flow:feature"]);
    }
    world.gh_calls();
    dispatch(&world, 7).ok();
    let mut first = cloned(&world);
    first.sort();
    assert_eq!(first, ["acme/dev", "acme/reviewer", "acme/team"], "each once");

    dispatch(&world, 8).ok();
    assert_eq!(cloned(&world), [] as [String; 0]);
    let old = head(&world, "acme/dev");
    let command = |issue| open_terminals(&world, &worktree(&world, issue))[0]["command"].clone();
    assert!(
        command(8)
            .as_str()
            .unwrap()
            .starts_with(&format!("cd '{}' && ", copy(&world, "acme/dev", &old).display()))
    );

    // A push is how a change to a role reaches a worker: the next stage that
    // starts runs under the new commit, and the old copy is as it was.
    let new = commit(&world.root.join("github/acme/dev"), "CLAUDE.md", "stricter rules\n");
    dispatch(&world, 9).ok();
    assert_eq!(cloned(&world), ["acme/dev"]);
    let kept = |commit: &str| {
        std::fs::read_to_string(copy(&world, "acme/dev", commit).join("CLAUDE.md")).unwrap()
    };
    assert_eq!(
        (kept(&old).as_str(), kept(&new).as_str()),
        ("the rules of a dev\n", "stricter rules\n")
    );
    assert!(command(9).as_str().unwrap().contains(&format!(" AFKCONVOY_ROLE_COMMIT='{new}' ")));
}

#[test]
fn a_start_refuses_a_launch_command_that_does_not_take_the_flags() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    world.shell_help(Some("Usage: agent [options]\n  --model <model>\n  --add-dir <dir>\n"));
    for start in [&["tick", "--instance", "alpha", "--host", "h1"][..], &["plan"]] {
        let refused = world.afk(start).error();
        assert!(refused.contains("cannot take the fleet's launch flags"), "{refused}");
        assert!(
            refused
                .contains(&format!("`{STOCK} --help` names no --setting-sources, no --settings.")),
            "{refused}"
        );
    }
    // A command that does not answer at all is refused too, in its own words.
    world.shell_help(None);
    let refused = {
        world.kit_command("claude-code", "agent -x");
        world.afk(&["tick", "--instance", "alpha", "--host", "h1"])
    };
    let refused = refused.error();
    assert!(refused.contains("\"agent -x\""), "{refused}");
    assert!(refused.contains("`agent -x --help` failed in a login shell"), "{refused}");
    // Nothing was started, and nothing claimed.
    assert_eq!(world.claims(), []);
    assert_eq!(world.orca()["terminals"], json!([]));
    assert!(world.shell_asked().contains(&format!("{STOCK} --help")));
}

#[test]
fn a_start_checks_every_harness_it_could_be_asked_for_and_names_what_calls_for_each() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    // A role's range holds it, and so does the host kit's first; the repo
    // runs one patrol of the two.
    let patrol = "```yaml\nby: reviewer\ndo: review\nevery: 1d\nmax_open: 1\n```\n";
    let team = world.root.join("github/acme/team");
    write(&team, "patrols/sweep.md", patrol);
    write(&team, "patrols/idle.md", patrol);
    git(&team, &["add", "-A"]);
    git(&team, &["commit", "-q", "-m", "name the harnesses"]);
    let config = CONFIG.replace("team: acme/team\n", "team: acme/team\npatrols: [sweep]\n");
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "run a patrol"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    let called = "it is the worker launch command of the harness `claude-code`, called for by the \
                  host kit's first, the range of the role `dev`, the range of the role \
                  `reviewer`";

    // One that takes no role flags: a start, `plan` and `doctor` each say so.
    world.shell_help(Some("Usage: agent [options]\n  --model <model>\n"));
    let tick = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    for start in [&tick[..], &["plan", "--now", "1000"]] {
        let refused = world.afk(start).error();
        assert!(refused.contains("names no --add-dir"), "{refused}");
        assert!(refused.contains("called for by startup candidate"), "{refused}");
    }
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]).json;
    let commands = doctor["host"]["worker_commands"].as_array().unwrap();
    assert_eq!(commands.len(), 1, "one for each harness: {commands:?}");
    assert_eq!(
        (&commands[0]["harness"], &commands[0]["command"], &commands[0]["takes_role_flags"]),
        (&json!("claude-code"), &json!(STOCK), &json!(false))
    );
    assert_eq!(
        commands[0]["called_for_by"],
        json!([
            "the host kit's first",
            "the range of the role `dev`",
            "the range of the role `reviewer`"
        ])
    );
    let findings = doctor["findings"].as_array().unwrap();
    let finding = findings.iter().find(|finding| {
        finding["what"].as_str().unwrap().contains("cannot start a worker in its role")
    });
    assert!(finding.unwrap()["evidence"].as_str().unwrap().ends_with(called), "{findings:?}");

    // One that resolves to nothing, given for the harness by its key.
    world.shell_help(Some("--add-dir --setting-sources --settings --model"));
    world.shell_knows("ckim", None);
    let refused = {
        world.kit_command("claude-code", "ckim --yolo");
        world.afk(&tick)
    };
    let refused = refused.error();
    assert!(refused.contains("\"ckim\" resolves to nothing in a login shell"), "{refused}");
    assert!(refused.contains("worker launch command"), "{refused}");

    // A start with a harness that cannot be started starts no worker at all.
    assert_eq!(world.claims(), []);
    assert_eq!(world.orca()["terminals"], json!([]));
    // …and with one that can, the worker of the stage is started with it.
    {
        world.kit_command("claude-code", "agent --fast");
        world.afk(&tick)
    }
    .ok();
    let command = open_terminals(&world, &worktree(&world, 7))[0]["command"].clone();
    assert!(command.as_str().unwrap().contains(" && agent --fast --add-dir "), "{command}");
}

#[test]
fn an_unbound_repo_s_launch_command_is_preserved_apart_from_worktree_trust() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let passed = "agent  --fast ";
    world.kit_command("claude-code", passed);
    let now = ["--now", "1000"];
    world.afk(&[&["tick", "--instance", "alpha", "--host", "h1"][..], &now].concat()).ok();
    assert_eq!(open_terminals(&world, &worktree(&world, 7))[0]["command"], common::trusted(passed));
    let asked = world.shell_asked();
    assert!(asked.iter().any(|script| script.ends_with("--help")), "{asked:?}");
    let copies = world.cache().join("afkconvoy/copies");
    assert!(!copies.exists(), "an unbound repo keeps no copy");
    assert!(brief(&worktree(&world, 7)).contains("**Implement**"));
}

#[test]
fn an_issue_no_worker_can_be_started_on_is_not_claimed() {
    let world = bound();
    world.issue(7, &["flow:chore"]);
    world.issue(8, &["flow:feature", "flow:audit"]);
    world.issue(9, &["bug"]);
    // What a human is handed names their topic; an issue that follows no
    // workflow, the one that says what a flow label is.
    let refused = dispatch(&world, 7);
    assert_eq!(refused.json["see"], "afkconvoy help humans");
    assert_eq!(dispatch(&world, 9).json["see"], "afkconvoy help workflows");
    let why = refused.error();
    assert!(why.contains("issue #7 cannot be started"), "{why}");
    assert!(why.contains("flow:chore names no workflow of the team"), "{why}");
    assert!(dispatch(&world, 8).error().contains("2 flow labels (flow:feature, flow:audit)"));
    assert!(dispatch(&world, 9).error().contains("it carries no flow label"));
    assert_eq!(world.claims(), []);
    assert_eq!(world.orca()["worktrees"], json!([]));

    // A team that does not load starts nobody, and says every fault.
    let team = world.root.join("github/acme/team");
    commit(&team, "workflows/audit.md", &AUDIT.replace("do: review", "do: deploy"));
    world.issue(10, &["flow:feature"]);
    let refused = dispatch(&world, 10);
    assert_eq!(refused.json["faults"][0]["file"], "workflows/audit.md", "{}", refused.json);
    assert_eq!(refused.json["faults"][0]["key"], "stages[0].do");
    assert_eq!(world.claims(), []);
}

/// The worker's terminal reports that it stopped at `since`.
fn stopped(world: &World, since: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": since * 1000});
                terminal["lastOutputAt"] = json!(since * 1000);
                terminal["screen"] = json!(["", "Shall I go on? [y/n]", ""]);
            }
        }
    });
}

/// orca has no report of the worker: its terminal last wrote at `at`.
fn wrote(world: &World, at: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                assert!(terminal["agent"].is_null(), "{terminal}");
                terminal["lastOutputAt"] = json!(at * 1000);
            }
        }
    });
}

#[test]
fn a_worker_orca_has_no_report_of_is_at_work_while_its_terminal_writes_or_it_just_started() {
    let world = bound();
    world.issue(7, &["flow:audit"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let dir = worktree(&world, 7);
    let nudges = || open_terminals(&world, &dir)[0]["sent"].as_array().unwrap().len() - 1;

    // Just started: left alone, whatever its terminal and its worktree show.
    wrote(&world, 1);
    assert_eq!(did(&tick(&world, 1060).ok())[0], "advance 7 nothing");
    assert_eq!(did(&tick(&world, 1299).ok())[0], "advance 7 nothing");
    // A stage that writes nothing leaves no commit and no file: its
    // terminal's output within grace is the sign that it is at work.
    wrote(&world, 1390);
    assert_eq!(did(&tick(&world, 1400).ok())[0], "advance 7 nothing");
    assert_eq!(did(&tick(&world, 1689).ok())[0], "advance 7 nothing");
    assert_eq!(nudges(), 0);

    // Quiet past grace: nudged once, as ever.
    assert_eq!(did(&tick(&world, 1690).ok())[0], "advance 7 nothing");
    assert_eq!(did(&tick(&world, 1780).ok())[0], "advance 7 nothing");
    assert_eq!(nudges(), 0);
}

#[test]
fn a_worker_in_its_role_is_observed_before_any_recovery_action() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    tick(&world, 1000).ok();
    let dir = worktree(&world, 7);
    let handle = open_terminals(&world, &dir)[0]["handle"].clone();
    stopped(&world, 1000);
    tick(&world, 1400).ok();
    tick(&world, 1800).ok();
    let terminals = open_terminals(&world, &dir);
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["handle"], handle);
    assert_eq!(terminals[0]["sent"].as_array().unwrap().len(), 1);
    assert_eq!(world.claims().len(), 1);
    let plan = world.afk(&["plan", "--now", "1801"]).ok();
    assert_eq!(plan["host_kit"]["worker_health"]["workers"][0]["action"], "waiting for operator");
    assert!(
        plan["host_kit"]["worker_health"]["workers"][0]["snapshots"]
            .to_string()
            .contains("Shall I go on?")
    );
}

#[test]
fn a_worker_that_is_gone_is_started_again_in_its_role() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let dir = worktree(&world, 7);
    commit(&dir, "seven.txt", "7\n");
    let first = open_terminals(&world, &dir);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });

    assert_eq!(did(&tick(&world, 1090).ok())[0], "advance 7 continued");
    let second = open_terminals(&world, &dir);
    assert_eq!(second.len(), 1);
    assert_ne!(second[0]["handle"], first[0]["handle"]);
    assert!(dir.join("seven.txt").is_file(), "where the one before it worked");
    assert_eq!(second[0]["command"], first[0]["command"], "the same role, the same commits");
    assert!(second[0]["command"].as_str().unwrap().starts_with("cd '"));
}

/// The topic that says how each harness's worker is started names, under each
/// harness, every flag a bound repo requires of its worker launch command.
#[test]
fn the_worker_launch_topic_names_what_a_bound_repo_requires_of_each_launch_command() {
    use afkconvoy_core::harness::Harness;
    use afkconvoy_core::role::{Facts, On};
    let mut topic = std::process::Command::new(env!("CARGO_BIN_EXE_afkconvoy"));
    let output = topic.args(["help", "worker-launch"]).output().unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    for harness in Harness::ALL {
        let heading = format!("\n## {}\n", harness.name());
        let (_, section) = help.split_once(&heading).unwrap_or_else(|| panic!("no {heading:?}"));
        let section = section.split("\n## ").next().unwrap();
        for flag in harness.flags() {
            let named = section.contains(&format!("`{flag}`"));
            assert!(named, "the topic does not name {flag} under {}", harness.name());
        }
        assert!(
            section.contains(harness.worktree_trust()),
            "the topic does not name worktree trust under {}",
            harness.name()
        );
    }
    let facts = Facts {
        on: On::Stage { issue: 7, flow: "f", stage: "s" },
        role: "r",
        skill: "k",
        writes: true,
        consult: false,
        base_branch: "m",
        target_branch: None,
        escalate_label: "e",
        lock_label: "l",
        role_commit: "a",
        team_commit: "b",
    };
    for (name, _) in facts.env() {
        assert!(help.contains(name), "the topic does not name {name}");
    }
    // `run --help` names the topic, and holds no harness's launch line.
    let mut run = std::process::Command::new(env!("CARGO_BIN_EXE_afkconvoy"));
    let run = String::from_utf8(run.args(["run", "--help"]).output().unwrap().stdout).unwrap();
    assert!(run.contains("`afkconvoy help worker-launch`"), "{run}");
    let lines = ["cd <the role's copy> && export", "PI_CODING_AGENT_DIR", "CODEX_HOME", "DSH_HOME"];
    for line in lines {
        assert!(!run.contains(line), "run --help still holds {line:?}");
    }
}

/// A team whose repo holds two of its roles, `dev` and `tester`, each in a
/// directory of its own, beside the team rules at its root and skills there
/// that are a human's.
fn one_repo() -> World {
    let world = bound();
    let team = "```yaml\nroles:\n  dev: ./roles/dev\n  tester: acme/team/roles/tester\n  \
                reviewer: acme/reviewer\n```\n";
    let hook = r#"{"hooks": {"Stop": [{"hooks": [{"type": "command",
        "command": "sh \"$CLAUDE_PROJECT_DIR\"/.claude/hooks/limit.sh"}]}]}}"#;
    let repo = world.root.join("github/acme/team");
    for (file, text) in [
        ("CLAUDE.md", "the rules of the team\n"),
        (".claude/skills/release/SKILL.md", "---\nname: release\ndescription: a human's\n---\n"),
        ("patrols/sweep.md", "```yaml\nby: dev\ndo: implement\nevery: 1h\nmax_open: 2\n```\n"),
        ("docs/notes.md", "whatever else\n"),
        ("roles/README.md", "the roles\n"),
        ("roles/dev/CLAUDE.md", "the rules of a dev\n"),
        (
            "roles/dev/.claude/skills/implement/SKILL.md",
            "---\nname: implement\ndescription: build\n---\n",
        ),
        ("roles/dev/.claude/settings.json", hook),
        ("roles/dev/.claude/hooks/limit.sh", "echo \"a limit of $(cat CLAUDE.md)\"\n"),
        ("roles/tester/CLAUDE.md", "the rules of a tester\n"),
        (
            "roles/tester/.claude/skills/implement/SKILL.md",
            "---\nname: implement\ndescription: test\n---\n",
        ),
    ] {
        write(&repo, file, text);
    }
    commit(&repo, "team.md", team);
    world
}

/// The names of what the directory `dir` holds, sorted.
fn holds(dir: &Path) -> Vec<String> {
    let entries = std::fs::read_dir(dir).unwrap();
    let mut names: Vec<String> =
        entries.map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    names
}

/// Where the worker started by `command` starts, and each directory added to it.
fn started_in(command: &str) -> (PathBuf, Vec<PathBuf>) {
    let quoted = |after: &str| -> Vec<PathBuf> {
        let found = command.split(after).skip(1);
        found.map(|rest| PathBuf::from(rest.split('\'').next().unwrap())).collect()
    };
    (quoted("cd '").remove(0), quoted(" --add-dir '"))
}

/// Every rules file and directory of skills and settings Claude Code loads
/// for the worker started by `command` from `world`: those of the directory
/// it starts in and of every directory above it in the world, and those of
/// each directory added to it. What is above the world is the host's that
/// runs the tests: a checkout of this repo, when a worker runs them.
fn loaded(world: &World, command: &str) -> Vec<PathBuf> {
    let (starts_in, added) = started_in(command);
    let mut dirs: Vec<&Path> =
        starts_in.ancestors().take_while(|dir| dir.starts_with(&world.root)).collect();
    dirs.extend(added.iter().map(PathBuf::as_path));
    let found = dirs.iter().flat_map(|dir| [dir.join("CLAUDE.md"), dir.join(".claude")]);
    found.filter(|path| path.exists()).collect()
}

#[test]
fn a_stage_whose_role_is_a_subdirectory_starts_its_worker_in_that_directory_alone() {
    let world = one_repo();
    world.issue(7, &["flow:feature"]);
    let started = dispatch(&world, 7).ok();
    assert_eq!(started["started"], true, "{started}");

    // Both commits are the repo's: the one a human can check out.
    let team = head(&world, "acme/team");
    let stage = &started["stage"];
    assert_eq!((&stage["role_commit"], &stage["team_commit"]), (&json!(team), &json!(team)));
    let kept = world.cache().join("afkconvoy/copies/acme/team");
    let (role_dir, team_dir) = (
        kept.join(format!("{team}-role+claude-code-roles%2Fdev")),
        kept.join(format!("{team}-team+rules-")),
    );
    assert_eq!((&stage["role_dir"], &stage["team_dir"]), (&json!(role_dir), &json!(team_dir)));

    // The role's copy is its directory and nothing else of its repo…
    assert_eq!(holds(&role_dir), [".claude", ".git", "CLAUDE.md"]);
    assert_eq!(
        std::fs::read_to_string(role_dir.join("CLAUDE.md")).unwrap(),
        "the rules of a dev\n"
    );
    assert_eq!(holds(&role_dir.join(".claude")), ["hooks", "settings.json", "skills"]);
    assert!(!role_dir.parent().unwrap().join("CLAUDE.md").exists());
    // …and the team's is its directory without those of its roles, and
    // without the skills and settings at its top: its rules are kept.
    assert_eq!(holds(&team_dir), ["CLAUDE.md", "docs", "patrols", "roles", "team.md", "workflows"]);
    assert_eq!(holds(&team_dir.join("roles")), ["README.md"]);
    for copied in [&role_dir, &role_dir.join(".claude/settings.json"), &team_dir] {
        assert!(is_read_only(copied), "{}", copied.display());
    }

    // The launch: the worker starts in the role's copy, with the worktree and
    // the team's copy added, and the rules of added directories on. It loads
    // the role's rules and the team rules and no others — no neighbour's, and
    // of the team's directory no skill and no setting.
    let dir = worktree(&world, 7);
    let command = open_terminals(&world, &dir)[0]["command"].as_str().unwrap().to_string();
    let added = vec![dir.clone(), team_dir.clone(), host_user(&world)];
    assert_eq!(started_in(&command), (role_dir.clone(), added));
    assert!(command.contains(" export CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 "));
    assert_eq!(
        loaded(&world, &command),
        [role_dir.join("CLAUDE.md"), role_dir.join(".claude"), team_dir.join("CLAUDE.md")]
    );
    assert_eq!(
        std::fs::read_to_string(team_dir.join("CLAUDE.md")).unwrap(),
        "the rules of the team\n"
    );
    // The brief names them, and copies none.
    let told = brief(&dir);
    let rules = format!("- **Team rules:** `{}`\n", team_dir.join("CLAUDE.md").display());
    assert!(told.contains(&rules) && !told.contains("the rules of the team"), "{told}");
    let roles = format!(" AFKCONVOY_ROLE_COMMIT='{team}' AFKCONVOY_TEAM_COMMIT='{team}' ");
    assert!(command.contains(&roles), "{command}");
    assert!(!brief(&dir).contains("precedents.md"));

    // A hook of the role runs as Claude Code runs it: from the directory the
    // worker starts in, which is the project's.
    let settings = std::fs::read_to_string(role_dir.join(".claude/settings.json")).unwrap();
    let settings: Value = serde_json::from_str(&settings).unwrap();
    let hook = settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
    let enter = command.split(" && ").next().unwrap();
    let ran = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("{enter} && CLAUDE_PROJECT_DIR=\"$PWD\" && {hook}")])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "a limit of the rules of a dev\n");

    // A neighbour starts in its own directory, out of the same fetch.
    world.gh_calls();
    world.issue(8, &["flow:audit"]);
    let repo = world.root.join("github/acme/team");
    let audit = AUDIT.replace("by: reviewer\n    do: review", "by: tester\n    do: implement");
    let moved = commit(&repo, "workflows/audit.md", &audit);
    let started = dispatch(&world, 8).ok();
    assert_eq!(cloned(&world), ["acme/team"], "one fetch of the new commit");
    let tester = kept.join(format!("{moved}-role+claude-code-roles%2Ftester"));
    assert_eq!(started["stage"]["role_dir"], json!(tester));
    assert_eq!(holds(&tester), [".claude", ".git", "CLAUDE.md"]);
}

/// One repo that holds a team and seven roles is fetched once per commit,
/// however many of its directories are read and started in.
#[test]
fn a_team_and_seven_roles_in_one_repo_are_loaded_with_one_fetch() {
    let world = bound();
    let repo = world.root.join("github/acme/team");
    let mut team = "```yaml\nroles:\n  reviewer: acme/reviewer\n".to_string();
    for (at, name) in ["dev", "b", "c", "d", "e", "f", "g"].iter().enumerate() {
        // Each way to name a directory of the team's own repo.
        let named = if at % 2 == 0 { "./roles" } else { "acme/team/roles" };
        team.push_str(&format!("  {name}: {named}/{name}\n"));
        write(&repo, &format!("roles/{name}/.claude/skills/implement/SKILL.md"), IMPLEMENT);
    }
    commit(&repo, "team.md", &format!("{team}```\n"));
    world.gh_calls();

    let read = world.afk(&["check"]).ok();
    assert_eq!(read["roles"].as_array().unwrap().len(), 8, "{read}");
    let mut first = cloned(&world);
    first.sort();
    assert_eq!(first, ["acme/reviewer", "acme/team"], "the team's repo once");
    // A second load at the same commit fetches nothing, and neither does a start.
    world.afk(&["check"]).ok();
    world.issue(7, &["flow:feature"]);
    let started = dispatch(&world, 7).ok();
    assert_eq!(cloned(&world), [] as [String; 0]);
    let commit = head(&world, "acme/team");
    assert_eq!(started["stage"]["role_commit"], commit);
    let role_dir = PathBuf::from(started["stage"]["role_dir"].as_str().unwrap());
    assert_eq!(holds(&role_dir), [".claude", ".git"]);
}

#[test]
fn claude_role_copies_bound_discovery_without_reusing_older_unbounded_copies() {
    use afkconvoy::copies::{Copies, Copy};
    use afkconvoy_core::config::RepoDir;
    use afkconvoy_core::harness::Harness;

    let world = World::new();
    let cache = world.cache().join("afkconvoy");
    let copies = Copies::under(&cache);
    let repo = cache.join("copies/acme/dev");
    let source = repo.join("abc");
    write(&source, "CLAUDE.md", "Whole role.\n");
    write(&source, "roles/dev/CLAUDE.md", "Nested role.\n");
    let older = repo.join("abc-role-roles%2Fdev");
    write(&older, "CLAUDE.md", "An older cached copy.\n");
    let fetched = Copy { dir: source.clone(), commit: "abc".into() };
    for (name, rules) in [("acme/dev", "Whole role.\n"), ("acme/dev/roles/dev", "Nested role.\n")] {
        let dir = RepoDir::read(name).unwrap();
        let role = copies.role(&fetched, &dir, Harness::ClaudeCode).unwrap();
        assert_ne!(role.dir, source);
        assert_ne!(role.dir, older);
        assert_eq!(std::fs::read_to_string(role.dir.join("CLAUDE.md")).unwrap(), rules);
        assert_eq!(holds(&role.dir.join(".git")), [] as [String; 0]);
        assert!(is_read_only(&role.dir.join(".git")));
        assert_eq!(copies.role(&fetched, &dir, Harness::ClaudeCode).unwrap().dir, role.dir);
    }
    assert!(!source.join(".git").exists() && !older.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(older.join("CLAUDE.md")).unwrap(),
        "An older cached copy.\n"
    );
    let dir = RepoDir::read("acme/dev").unwrap();
    assert_eq!(copies.role(&fetched, &dir, Harness::Pi).unwrap().dir, source);
}

#[test]
fn a_worker_is_given_the_host_user_s_skills_and_rules_unless_its_team_turns_them_off() {
    let world = bound();
    // What this host's user has of Claude Code's: skills, rules, and
    // settings with a hook in them.
    let home = world.home();
    for (file, text) in [
        (".claude/skills/theirs/SKILL.md", "---\nname: theirs\ndescription: theirs\n---\n"),
        (".claude/CLAUDE.md", "the user's rules\n"),
        (".claude/settings.json", "{\"hooks\": {\"Stop\": [{\"hooks\": []}]}}\n"),
    ] {
        write(&home, file, text);
    }
    world.issue(7, &["flow:feature"]);
    dispatch(&world, 7).ok();
    // A directory of the fleet's is added to the worker, which links to the
    // host user's skills and rules and holds nothing else of theirs: their
    // settings are still left out.
    let kept = host_user(&world);
    let command = open_terminals(&world, &worktree(&world, 7))[0]["command"].clone();
    let command = command.as_str().unwrap();
    assert!(command.contains(&format!(" --add-dir '{}' ", kept.display())), "{command}");
    assert!(command.contains(" --setting-sources project "), "{command}");
    let settings: Value = serde_json::from_str(&added_settings(command)).unwrap();
    assert!(settings["claudeMdExcludes"].is_null(), "{settings}");
    assert_eq!(holds(&kept), [".claude"]);
    assert_eq!(holds(&kept.join(".claude")), ["CLAUDE.md", "skills"]);
    let link = |name: &str| std::fs::read_link(kept.join(".claude").join(name)).unwrap();
    assert_eq!(link("skills"), home.join(".claude/skills"));
    assert_eq!(link("CLAUDE.md"), home.join(".claude/CLAUDE.md"));

    // A team that turns them off has its workers given none of it, as before.
    let team = world.root.join("github/acme/team");
    commit(&team, "team.md", &TEAM.replace("```\n", "host_user: false\n```\n"));
    world.issue(8, &["flow:audit"]);
    dispatch(&world, 8).ok();
    let command = open_terminals(&world, &worktree(&world, 8))[0]["command"].clone();
    let command = command.as_str().unwrap();
    assert!(!command.contains(host_user(&world).to_str().unwrap()), "{command}");
    let settings: Value = serde_json::from_str(&added_settings(command)).unwrap();
    assert_eq!(
        settings["claudeMdExcludes"],
        serde_json::json!([
            home.join(".claude/CLAUDE.md").display().to_string(),
            format!("{}/.claude/rules/**", home.display())
        ])
    );
    assert!(!world.cache().join("afkconvoy/harness/claude-code").exists());
}

#[test]
fn a_role_may_keep_from_the_host_user_s_skills_and_rules_while_the_team_s_others_keep_them() {
    let world = bound();
    write(&world.home(), ".claude/CLAUDE.md", "the user's rules\n");
    let team = world.root.join("github/acme/team");
    let own = "reviewer: { dir: acme/reviewer, host_user: false }";
    commit(&team, "team.md", &TEAM.replace("reviewer: acme/reviewer", own));
    let command = |issue: u64| {
        let command = open_terminals(&world, &worktree(&world, issue))[0]["command"].clone();
        command.as_str().unwrap().to_string()
    };
    let kept = format!(" --add-dir '{}' ", host_user(&world).display());

    // `dev` is named by its directory alone, and takes the team's: given them.
    world.issue(7, &["flow:feature"]);
    assert_eq!(dispatch(&world, 7).ok()["stage"]["role"], "dev");
    assert!(command(7).contains(&kept), "{}", command(7));
    // `reviewer` says it for itself: given none of it.
    world.issue(8, &["flow:audit"]);
    assert_eq!(dispatch(&world, 8).ok()["stage"]["role"], "reviewer");
    assert!(!command(8).contains(&kept), "{}", command(8));

    // Whatever the team says: one that turns them off still has `reviewer`
    // given them where it says so, and `dev` none.
    let own = "reviewer: { dir: acme/reviewer, host_user: true }";
    commit(
        &team,
        "team.md",
        &TEAM.replace("reviewer: acme/reviewer", own).replace("```\n", "host_user: false\n```\n"),
    );
    world.issue(9, &["flow:feature"]);
    dispatch(&world, 9).ok();
    assert!(!command(9).contains(&kept), "{}", command(9));
    world.issue(10, &["flow:audit"]);
    dispatch(&world, 10).ok();
    assert!(command(10).contains(&kept), "{}", command(10));
}

#[test]
fn a_team_can_leave_splitting_out_of_its_dispatched_stage_briefs() {
    for setting in ["", "split: true\n", "split: false\n"] {
        let world = bound();
        let team = world.root.join("github/acme/team");
        if !setting.is_empty() {
            commit(&team, "team.md", &TEAM.replace("```\n", &format!("{setting}```\n")));
        }
        world.issue(7, &["flow:feature"]);
        world.issue(8, &["flow:audit"]);
        tick(&world, 1000).ok();
        for issue in [7, 8] {
            let told = brief(&worktree(&world, issue));
            if setting == "split: false\n" {
                assert!(!told.contains("## Splitting the issue"), "{told}");
                assert!(!told.contains("may split it"), "{told}");
            } else {
                assert!(told.contains("## Splitting the issue"), "{told}");
                assert!(told.contains("afkconvoy intake"), "{told}");
            }
        }
    }
}

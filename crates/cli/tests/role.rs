//! A bound repo's worker starts in its role: the fleet keeps a read-only copy
//! of the team and of each role per commit, opens the terminal on the issue's
//! worktree as ever, and has it change to the role's copy and start the
//! worker launch command with a fixed set added (docs/adr/0009).

mod common;

use std::path::{Path, PathBuf};

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    model: opus
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: done, reject: build }
```
";
/// A workflow whose first stage reviews: no model, and nothing written.
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
const STOCK: &str = "claude --dangerously-skip-permissions";

/// A world whose repo is bound to a team of two roles and two workflows.
fn bound() -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[("CLAUDE.md", "the rules of a dev\n"), (".claude/skills/implement/SKILL.md", "build\n")],
    );
    world.repo("acme/reviewer", &[(".claude/skills/review/SKILL.md", "review\n")]);
    world.repo(
        "acme/team",
        &[("team.md", TEAM), ("workflows/feature.md", FEATURE), ("workflows/audit.md", AUDIT)],
    );
    write(&world.clone, "docs/agents/afkconvoy.md", CONFIG);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
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

fn dispatch(world: &World, issue: u64) -> common::Answer {
    let issue = issue.to_string();
    world.afk(&["dispatch", "--issue", &issue, "--instance", "alpha", "--host", "h1"])
}

fn head(world: &World, repo: &str) -> String {
    git(&world.root.join("github").join(repo), &["rev-parse", "HEAD"])
}

/// Where the fleet keeps its copy of `repo` at `commit`.
fn copy(world: &World, repo: &str, commit: &str) -> PathBuf {
    world.cache().join("afkconvoy/copies").join(repo).join(commit)
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
/// the guard before every tool call, and orca told wherever its state changes.
fn added_settings(command: &str) -> String {
    let added = command.split(" --settings '").nth(1).unwrap().split('\'').next().unwrap();
    let settings: Value = serde_json::from_str(added).unwrap();
    let hooks = |event: &str| settings["hooks"][event][0]["hooks"].clone();
    let guard = format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy"));
    assert_eq!(hooks("PreToolUse")[0], json!({"type": "command", "command": guard}));
    for event in ["PreToolUse", "SessionStart", "UserPromptSubmit", "PostToolUse", "Stop"] {
        let told = hooks(event);
        let report = told.as_array().unwrap().last().unwrap();
        let line = report["command"].as_str().unwrap();
        assert!(line.contains("/.orca/agent-hooks/claude-hook.sh"), "{event}: {settings}");
        assert_eq!(report["timeout"], 10, "{event}: {settings}");
    }
    added.to_string()
}

/// What the guard a worker was started with by `command` answers the Bash
/// call `line`: the hook its settings name, run as Claude Code runs one — in
/// the worker's directory and environment, the call described on stdin →
/// its exit code and what it said on stderr.
fn guarded(command: &str, line: &str) -> (i32, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut steps = command.split(" && ");
    let (enter, facts, start) = (steps.next().unwrap(), steps.next().unwrap(), steps.next());
    let settings = start.unwrap().split(" --settings '").nth(1).unwrap();
    let settings: Value = serde_json::from_str(settings.split('\'').next().unwrap()).unwrap();
    let hooks = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(hooks.len(), 1, "{settings}");
    assert!(hooks[0]["matcher"].is_null(), "it is run before every tool: {settings}");
    let hook = hooks[0]["hooks"][0]["command"].as_str().unwrap();
    let mut child = Command::new("/bin/sh")
        .args(["-c", &format!("{enter} && {facts} && {hook}")])
        .env_clear()
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

    // The copies: the role's and the team's files at their heads, and no git dir.
    let (dev, team) = (head(&world, "acme/dev"), head(&world, "acme/team"));
    let (role_dir, team_dir) = (copy(&world, "acme/dev", &dev), copy(&world, "acme/team", &team));
    assert_eq!(
        std::fs::read_to_string(role_dir.join("CLAUDE.md")).unwrap(),
        "the rules of a dev\n"
    );
    assert!(role_dir.join(".claude/skills/implement/SKILL.md").is_file());
    assert!(team_dir.join("workflows/feature.md").is_file());
    assert!(!role_dir.join(".git").exists());
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
         CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
         AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
         AFKCONVOY_WRITES='true' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
         AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
         AFKCONVOY_ROLE_COMMIT='{dev}' AFKCONVOY_TEAM_COMMIT='{team}' && \
         {STOCK} --add-dir '{worktree}' --add-dir '{team_dir}' --setting-sources project \
         --settings '{guard}' --model 'opus'",
        guard = added_settings(terminals[0]["command"].as_str().unwrap()),
        role = role_dir.display(),
        worktree = dir.display(),
        team_dir = team_dir.display(),
    );
    assert_eq!(terminals[0]["command"], command);
    assert_eq!(created[4..], ["--command".to_string(), command]);

    // The brief is a fixed form, and the worker is pointed at it.
    let brief = brief(&dir);
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
        format!("- **Precedents:** `{}`\n", team_dir.join("precedents.md").display()),
        " verdict --issue 7 --stage 'build'\n```".to_string(),
    ] {
        assert!(brief.contains(&said), "{said}\n{brief}");
    }
    for how in ["Implement", "gh pr create", "Join the landing train", "git commit"] {
        assert!(!brief.contains(how), "the brief says nothing of how: {how}");
    }
    // But for the gate: a stage that may commit hands on no red PR.
    let gate =
        format!("   cd '{}' && {} gate --train\n", dir.display(), env!("CARGO_BIN_EXE_afkconvoy"));
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
            "writes": false, "consult": false, "model": null,
            "role_commit": reviewer, "role_dir": copy(&world, "acme/reviewer", &reviewer),
            "team_commit": team, "team_dir": copy(&world, "acme/team", &team)})
    );
    // A stage that names no model starts with none, and says it writes nothing.
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
            let (code, said) = guarded(worker, line);
            assert_eq!(code, 2, "{line}: {said}");
            assert!(said.contains(reason), "{line}: {said}");
        }
        assert_eq!(guarded(worker, "cargo test").0, 0);
    }
    // The stage `build` may commit; `look` is marked `writes: false`.
    let committing = "git commit -am step && git push origin HEAD";
    assert_eq!(guarded(&build, committing), (0, String::new()));
    let (code, said) = guarded(&look, committing);
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
        assert!(refused.contains("bound to a team"), "{refused}");
        assert!(
            refused
                .contains(&format!("`{STOCK} --help` names, of Claude Code's flags, no --setting-sources, no --settings; of pi's flags, no --no-extensions,")),
            "{refused}"
        );
    }
    // A command that does not answer at all is refused too, in its own words.
    world.shell_help(None);
    let refused =
        world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--worker-command", "agent -x"]);
    let refused = refused.error();
    assert!(refused.contains("\"agent -x\""), "{refused}");
    assert!(refused.contains("`agent -x --help` failed in a login shell"), "{refused}");
    // Nothing was started, and nothing claimed.
    assert_eq!(world.claims(), []);
    assert_eq!(world.orca()["terminals"], json!([]));
    assert!(world.shell_asked().contains(&format!("{STOCK} --help")));
}

#[test]
fn an_unbound_repo_s_launch_command_is_still_passed_to_orca_exactly_as_given() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    // It need take no flag at all: it is never asked.
    world.shell_help(None);
    let passed = "agent  --fast ; true ";
    let now = ["--now", "1000", "--worker-command", passed];
    world.afk(&[&["tick", "--instance", "alpha", "--host", "h1"][..], &now].concat()).ok();
    assert_eq!(open_terminals(&world, &worktree(&world, 7))[0]["command"], passed);
    let asked = world.shell_asked();
    assert!(!asked.iter().any(|script| script.ends_with("--help")), "{asked:?}");
    assert!(!world.cache().exists(), "an unbound repo keeps no copy");
    assert!(brief(&worktree(&world, 7)).contains("**Implement**"));
}

#[test]
fn an_issue_no_worker_can_be_started_on_is_not_claimed() {
    let world = bound();
    world.issue(7, &["flow:chore"]);
    world.issue(8, &["flow:feature", "flow:audit"]);
    world.issue(9, &["bug"]);
    let why = dispatch(&world, 7).error();
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
    assert_eq!(did(&tick(&world, 1690).ok())[0], "advance 7 nudged");
    assert_eq!(did(&tick(&world, 1780).ok())[0], "advance 7 nothing");
    assert_eq!(nudges(), 1);
}

#[test]
fn a_worker_in_its_role_is_nudged_and_its_screen_is_read() {
    let world = bound();
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000).ok())[0], "dispatch 7 done");
    let dir = worktree(&world, 7);
    let handle = open_terminals(&world, &dir)[0]["handle"].clone();

    // The nudge is typed at the one terminal of the worktree: the worker's.
    stopped(&world, 1000);
    assert_eq!(did(&tick(&world, 1400).ok())[0], "advance 7 nudged");
    let terminals = open_terminals(&world, &dir);
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["handle"], handle);
    let sent = terminals[0]["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 2, "the brief, then the nudge: {sent:?}");
    assert!(
        sent[1]["text"].as_str().unwrap().starts_with("You stopped without declaring a verdict.")
    );

    // Silent past the nudge: escalated, with what its screen showed.
    assert_eq!(did(&tick(&world, 1800).ok())[0], "advance 7 done");
    let said = world.comments(7).join("\n");
    assert!(said.contains("Shall I go on? [y/n]"), "{said}");
    assert_eq!(world.claims(), []);
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

#[test]
fn the_readme_names_what_a_bound_repo_requires_of_the_launch_command() {
    use afkconvoy_core::role::{CodingAgent, Facts, On};
    let readme = include_str!("../../../README.md");
    let facts = Facts {
        on: On::Stage { issue: 7, flow: "f", stage: "s" },
        role: "r",
        skill: "k",
        writes: true,
        consult: false,
        base_branch: "m",
        escalate_label: "e",
        lock_label: "l",
        role_commit: "a",
        team_commit: "b",
    };
    let flags = CodingAgent::ALL.into_iter().flat_map(CodingAgent::flags);
    let named = flags.map(|flag| flag.to_string());
    for name in named.chain(facts.env().into_iter().map(|(name, _)| name.to_string())) {
        assert!(readme.contains(&format!("`{name}`")), "the README does not name {name}");
    }
}

/// What pi's `--help` names of the flags a start adds to it.
const PI_HELP: &str = "Usage:\n  pi [options]\n  --model <pattern>\n  --extension, -e <path>\n  \
                       --no-extensions, -ne\n  --skill <path>\n  --no-skills, -ns\n  \
                       --no-prompt-templates, -np\n  --no-themes\n  --no-context-files, -nc\n  \
                       --approve, -a\n  --no-approve, -na\n";

fn tick_with_pi(world: &World, now: u64) -> common::Answer {
    let now = now.to_string();
    let who = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&[&who[..], &["--worker-command", "pi"]].concat())
}

#[test]
fn a_command_that_starts_pi_puts_the_worker_in_its_role_in_pi_s_way() {
    let world = bound();
    world.shell_help(Some(PI_HELP));
    // orca's own extension for pi, where orca installs it.
    let report = world.home().join(".pi/agent/extensions/orca-agent-status.ts");
    write(&world.home(), ".pi/agent/extensions/orca-agent-status.ts", "// orca's\n");
    world.issue(7, &["flow:feature"]);

    let ticked = tick_with_pi(&world, 1000).ok();
    assert_eq!(did(&ticked)[0], "dispatch 7 done", "{ticked}");
    let asked = world.shell_asked();
    assert!(
        asked.contains(&"pi --list-models".to_string()),
        "pi is asked for its login: {asked:?}"
    );

    let (dev, team) = (head(&world, "acme/dev"), head(&world, "acme/team"));
    let (role_dir, team_dir) = (copy(&world, "acme/dev", &dev), copy(&world, "acme/team", &team));
    let dir = worktree(&world, 7);
    let git_dir = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    let extension = git_dir.join("convoy-pi-extension.ts");
    let terminals = open_terminals(&world, &dir);
    assert_eq!(
        terminals[0]["command"],
        format!(
            "cd '{role}' && export AFKCONVOY_GUARD='{guard} guard' AFKCONVOY_WORKTREE='{worktree}' \
             AFKCONVOY_TEAM_DIR='{team_dir}' AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='true' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='{dev}' AFKCONVOY_TEAM_COMMIT='{team}' && \
             pi --no-extensions --no-skills --no-prompt-templates --no-themes \
             --no-context-files --no-approve --extension '{extension}' --extension '{report}' \
             --skill '{role}/.claude/skills' --model 'opus'",
            guard = env!("CARGO_BIN_EXE_afkconvoy"),
            role = role_dir.display(),
            worktree = dir.display(),
            team_dir = team_dir.display(),
            extension = extension.display(),
            report = report.display(),
        )
    );
    // The fleet's extension is beside the brief: git never stages it.
    assert_eq!(
        std::fs::read_to_string(&extension).unwrap(),
        include_str!("../src/pi-extension.ts")
    );
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
}

#[test]
fn a_pi_worker_on_a_host_where_orca_installed_no_extension_is_started_with_the_fleet_s_alone() {
    let world = bound();
    world.shell_help(Some(PI_HELP));
    world.issue(7, &["flow:audit"]);
    assert_eq!(did(&tick_with_pi(&world, 1000).ok())[0], "dispatch 7 done");
    let terminals = open_terminals(&world, &worktree(&world, 7));
    let command = terminals[0]["command"].as_str().unwrap();
    assert_eq!(command.matches(" --extension ").count(), 1, "{command}");
    assert!(command.ends_with("/.claude/skills'"), "a stage that names no model: {command}");
}

#[test]
fn a_host_where_pi_has_credentials_for_no_model_starts_nothing() {
    let world = bound();
    world.shell_help(Some(PI_HELP));
    world.logged_out("pi");
    world.issue(7, &["flow:feature"]);
    let why = tick_with_pi(&world, 1000).error();
    assert!(why.contains("  pi is not logged in: pi  (then /login at its prompt)"), "{why}");
    assert!(world.worktrees().is_empty());
}

/// Whether this host's `node` runs a TypeScript file as it is, which is how
/// the extension is run here in place of pi.
fn node_runs_typescript() -> bool {
    use std::process::Command;
    let probe = "const n: number = 1; console.log(n)";
    let ran = Command::new("node").args(["--input-type=module-typescript", "-e", probe]).output();
    ran.is_ok_and(|out| out.status.success())
}

/// Stands in for pi: loads the extension, hands it each tool call it was
/// given, then asks it for the system prompt → what it answered.
const DRIVE: &str = r#"
import { pathToFileURL } from "node:url"
const [extension, calls] = process.argv.slice(2)
const handlers = {}
const loaded = await import(pathToFileURL(extension).href)
loaded.default({ on: (name, handler) => { handlers[name] = handler } })
const ctx = { cwd: process.cwd() }
const answer = (call) => {
  try {
    return handlers.tool_call({ toolName: call.tool, input: call.input }, ctx) ?? null
  } catch (error) {
    return { threw: String(error.message) }
  }
}
const answers = JSON.parse(calls).map(answer)
const prompt = handlers.before_agent_start({ systemPrompt: "PI'S OWN" }, ctx)
console.log(JSON.stringify({ answers, prompt: prompt?.systemPrompt ?? null }))
"#;

/// What the extension a pi worker was started with by `command` answers the
/// tool calls `calls` and makes of the system prompt: run as pi runs it, in
/// the worker's directory and environment — but for `without`, a variable
/// taken out of it.
fn under_pi(world: &World, command: &str, calls: &Value, without: Option<&str>) -> Value {
    use std::process::Command;
    let mut steps = command.split(" && ");
    let (enter, facts, start) = (steps.next().unwrap(), steps.next().unwrap(), steps.next());
    let extension = start.unwrap().split(" --extension '").nth(1).unwrap();
    let extension = extension.split('\'').next().unwrap();
    let drive = world.root.join("drive.mjs");
    std::fs::write(&drive, DRIVE).unwrap();
    let unset = without.map(|name| format!(" && unset {name}")).unwrap_or_default();
    let line = format!("{enter} && {facts}{unset} && node \"$DRIVE\" \"$EXTENSION\" \"$CALLS\"");
    let out = Command::new("/bin/sh")
        .args(["-c", &line])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("DRIVE", &drive)
        .env("EXTENSION", extension)
        .env("CALLS", calls.to_string())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_extension_puts_a_pi_worker_under_the_guard_its_role_s_limits_and_the_rules() {
    if !node_runs_typescript() {
        eprintln!("skipped: no `node` on PATH that runs TypeScript, to stand in for pi");
        return;
    }
    let world = World::new();
    // A role whose own limits are two hooks: one refuses by its exit code,
    // on Bash alone; one denies in what it prints, on every tool.
    let hooks = json!({"hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command":
            "if grep -q role-forbidden; then echo THE-ROLE-SAYS-NO >&2; exit 2; fi"}]},
        {"hooks": [{"type": "command", "command": "if grep -q '\"file_path\":\"/etc/' ; then \
            echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\",\
            \"permissionDecisionReason\":\"NOT-IN-ETC\"}}'; fi"}]},
    ]}});
    world.repo(
        "acme/reviewer",
        &[
            ("CLAUDE.md", "the rules of a reviewer\n"),
            (".claude/skills/review/SKILL.md", "review\n"),
            (".claude/settings.json", &hooks.to_string()),
        ],
    );
    world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", "build\n")]);
    world.repo(
        "acme/team",
        &[("team.md", TEAM), ("workflows/audit.md", AUDIT), ("AGENTS.md", "the team's rules\n")],
    );
    write(&world.clone, "docs/agents/afkconvoy.md", CONFIG);
    write(&world.clone, "CLAUDE.md", "the project's rules\n");
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.shell_help(Some(PI_HELP));
    world.issue(7, &["flow:audit"]);
    assert_eq!(did(&tick_with_pi(&world, 1000).ok())[0], "dispatch 7 done");
    let dir = worktree(&world, 7);
    let terminals = open_terminals(&world, &dir);
    let command = terminals[0]["command"].as_str().unwrap();

    let bash = |line: &str| json!({"tool": "bash", "input": {"command": line}});
    let calls = json!([
        bash("ls"),
        // The guard's: `look` is marked `writes: false`.
        bash("git commit -am step"),
        // The role's own, by exit code and by what a hook prints.
        bash("echo role-forbidden"),
        {"tool": "write", "input": {"path": "/etc/hosts", "content": "x"}},
        // A hook that matches Bash is not asked about another tool.
        {"tool": "read", "input": {"path": "role-forbidden.md"}},
    ]);
    let said = under_pi(&world, command, &calls, None);
    let answers = said["answers"].as_array().unwrap();
    assert_eq!(answers[0], Value::Null, "{said}");
    assert_eq!(answers[1]["block"], true, "{said}");
    let why = answers[1]["reason"].as_str().unwrap();
    assert!(why.contains("afkconvoy guard: the stage `look` is marked `writes: false`"), "{why}");
    assert_eq!(answers[2], json!({"block": true, "reason": "THE-ROLE-SAYS-NO"}));
    assert_eq!(answers[3], json!({"block": true, "reason": "NOT-IN-ETC"}));
    assert_eq!(answers[4], Value::Null, "{said}");

    // The rules: the role's, the worktree's and the team's, after pi's own
    // prompt — and nothing else.
    let team = copy(&world, "acme/team", &head(&world, "acme/team"));
    let role = copy(&world, "acme/reviewer", &head(&world, "acme/reviewer"));
    assert_eq!(
        said["prompt"],
        format!(
            "PI'S OWN\n\n# Rules: {}/CLAUDE.md\n\nthe rules of a reviewer\n\n\
             # Rules: {}/CLAUDE.md\n\nthe project's rules\n\n\
             # Rules: {}/AGENTS.md\n\nthe team's rules",
            role.display(),
            dir.display(),
            team.display()
        )
    );

    // A guard that cannot be asked allows nothing.
    let said = under_pi(&world, command, &json!([bash("ls")]), Some("AFKCONVOY_GUARD"));
    let threw = said["answers"][0]["threw"].as_str().unwrap();
    assert!(threw.contains("AFKCONVOY_GUARD is not set"), "{said}");
    let said = under_pi(&world, command, &json!([bash("ls")]), Some("AFKCONVOY_STAGE"));
    assert_eq!(said["answers"][0]["block"], true, "{said}");
}

#[test]
fn the_extension_reads_its_facts_where_a_start_puts_them() {
    use afkconvoy_core::role::{GUARD_ENV, TEAM_DIR_ENV, WORKTREE_ENV};
    let extension = include_str!("../src/pi-extension.ts");
    for name in [GUARD_ENV, WORKTREE_ENV, TEAM_DIR_ENV] {
        assert!(extension.contains(&format!("\"{name}\"")), "the extension reads no {name}");
    }
}

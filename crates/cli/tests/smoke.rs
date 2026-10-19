//! The one test against real Claude Code. Every other test starts workers in
//! a stand-in orca, which proves the fleet builds the right line — not that
//! Claude Code answers it as assumed (docs/adr/0009). This one runs that line
//! for real, in a throwaway role with a throwaway project added to it, and
//! has the worker try what its role's hook and the guard refuse
//! (docs/adr/0012).
//!
//! It is off by default and outside the gate: it needs a logged-in Claude
//! Code, takes seconds and costs a little. Run it by hand when what the fleet
//! adds to the worker launch command changes, or Claude Code is upgraded:
//!
//!   cargo test -p afkconvoy --test cli smoke:: -- --ignored --nocapture
//!
//! `AFKCONVOY_SMOKE_COMMAND` is the worker launch command to try, when it is
//! not the stock one. Nothing of the user's own configuration is touched: it
//! is only read, to know what must not show up.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use afkconvoy_core::harness::Harness;

use afkconvoy_core::role::{Facts, On, Start, Told};
use serde_json::Value;

const ROLE_WORD: &str = "ROLE-7391";
const PROJECT_WORD: &str = "PROJECT-5528";
const ROLE_SKILL: &str = "convoy-smoke-role-skill";
const PROJECT_SKILL: &str = "convoy-smoke-project-skill";

/// What the role's own hook refuses, and what it says when it does.
const ROLE_REFUSES: &str = "role-forbidden";
const ROLE_SAYS: &str = "ROLE-SAYS-NO-4417";

const PROMPT: &str = "Run these three Bash commands, each exactly once and in this order, and \
    carry on when one is refused, without trying it again or another way: `echo smoke`, then \
    `echo role-forbidden`, then `git commit --allow-empty -m smoke`. Then answer with exactly \
    these three lines and nothing else: ROLE=<the role word your instructions give, or NONE> / \
    PROJECT=<the project word your instructions give, or NONE> / FILES=<the path of every \
    CLAUDE.md or other instruction file that was loaded into your context, comma-separated>";

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: A throwaway skill of afkconvoy's smoke test\n---\nSay {name}.\n"
    )
}

/// A hook on every Bash call that appends what `says` expands to to `log`,
/// and refuses a call that names `ROLE_REFUSES`.
fn hook(log: &Path, says: &str) -> String {
    let command = format!(
        "echo {says} >> '{}'; if grep -q {ROLE_REFUSES}; then echo {ROLE_SAYS} >&2; exit 2; fi",
        log.display()
    );
    serde_json::json!({"hooks": {"PreToolUse": [{"matcher": "Bash",
        "hooks": [{"type": "command", "command": command}]}]}})
    .to_string()
}

fn lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

/// The names of the skills the user has installed for themselves.
fn user_skills(home: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(home.join(".claude/skills")) else { return vec![] };
    entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()
}

/// Whether the user's own settings define a hook.
fn user_has_hooks(home: &Path) -> bool {
    let settings = std::fs::read_to_string(home.join(".claude/settings.json")).unwrap_or_default();
    let settings: Value = serde_json::from_str(&settings).unwrap_or(Value::Null);
    settings["hooks"].as_object().is_some_and(|hooks| !hooks.is_empty())
}

#[test]
#[ignore = "starts real Claude Code: needs a login, costs a little; run by hand"]
fn real_claude_code_started_in_a_role_sees_the_role_and_the_project_and_nothing_of_the_user() {
    let scratch = tempfile::TempDir::new().unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let (role, project, team) = (root.join("role"), root.join("project"), root.join("team"));
    let (role_log, project_log) = (root.join("role-hook.log"), root.join("project-hook.log"));
    write(&role, "CLAUDE.md", &format!("# Role rules\n\nThe role word is {ROLE_WORD}.\n"));
    write(&role, &format!(".claude/skills/{ROLE_SKILL}/SKILL.md"), &skill(ROLE_SKILL));
    write(&role, ".claude/settings.json", &hook(&role_log, "\"$AFKCONVOY_STAGE\""));
    write(
        &project,
        "CLAUDE.md",
        &format!("# Project rules\n\nThe project word is {PROJECT_WORD}.\n"),
    );
    write(&project, &format!(".claude/skills/{PROJECT_SKILL}/SKILL.md"), &skill(PROJECT_SKILL));
    write(&project, ".claude/settings.json", &hook(&project_log, "project"));
    write(&team, "team.md", "a team\n");

    // The very line a bound repo's dispatch hands orca, and what makes it one
    // answer instead of a session.
    let launch = std::env::var("AFKCONVOY_SMOKE_COMMAND")
        .unwrap_or_else(|_| Harness::ClaudeCode.stock().to_string());
    let text = |path: &Path| path.display().to_string();
    let start = Start {
        role_dir: &text(&role),
        worktree: &text(&project),
        team_dir: &text(&team),
        model: Some("haiku"),
        guard: &format!("{} guard", env!("CARGO_BIN_EXE_afkconvoy")),
        kept: &text(&root.join("kept")),
        host_user: false,
        shadowed: &[],
        told: Told::default(),
        facts: Facts {
            on: On::Stage { issue: 7, flow: "feature", stage: "smoke-stage" },
            role: "dev",
            skill: ROLE_SKILL,
            writes: false,
            consult: false,
            base_branch: "main",
            escalate_label: "ready-for-human",
            lock_label: "fleet-lock",
            role_commit: "abc",
            team_commit: "def",
        },
    };
    let line = format!(
        "{} -p '{PROMPT}' --output-format stream-json --verbose --include-hook-events",
        Harness::ClaudeCode.in_role(&launch, &start)
    );
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let out = Command::new(shell).args(["-lic", &line]).stdin(Stdio::null()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let events: Vec<Value> =
        stdout.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    let of = |kind: &str, subtype: &str| -> Vec<&Value> {
        events.iter().filter(|event| event["type"] == kind && event["subtype"] == subtype).collect()
    };
    let (init, result) = (of("system", "init"), of("result", "success"));
    assert!(
        out.status.success() && init.len() == 1 && result.len() == 1,
        "Claude Code did not answer: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let init = init[0];
    let said = result[0]["result"].as_str().unwrap_or_default();
    let skills: Vec<&str> =
        init["skills"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    let home = PathBuf::from(std::env::var("HOME").unwrap());

    // The worker is in its role, on the stage's model.
    assert_eq!(init["cwd"], text(&role));
    assert!(init["model"].as_str().unwrap().contains("haiku"), "{}", init["model"]);
    // The role's rules and skill are seen…
    assert!(said.contains(&format!("ROLE={ROLE_WORD}")), "{said}");
    assert!(skills.contains(&ROLE_SKILL), "{skills:?}");
    // …and the added directory's…
    assert!(said.contains(&format!("PROJECT={PROJECT_WORD}")), "{said}");
    assert!(skills.contains(&PROJECT_SKILL), "{skills:?}");
    // …and the user's are not: no skill of theirs, no rule of theirs, no memory.
    // The user's skills load all together or not at all, and Claude Code ships
    // skills of its own under names a user may also have: one of theirs that
    // is absent says theirs were not loaded.
    let theirs = user_skills(&home);
    let absent = theirs.iter().filter(|name| !skills.contains(&name.as_str())).count();
    assert!(theirs.is_empty() || absent > 0, "the user's skills were loaded: {skills:?}");
    let files = said.lines().find(|line| line.starts_with("FILES=")).unwrap_or_default();
    assert!(!files.contains(&text(&home.join(".claude"))), "the user's rules were loaded: {files}");
    assert!(init["memory_paths"].is_null(), "{}", init["memory_paths"]);

    // The role's hook fired on every Bash call, and read the stage from the
    // environment; the added directory's did not; and beside it fired the
    // guard the fleet added and no other hook — none of the user's.
    let logged = lines(&role_log);
    assert!(logged.len() >= 3 && logged.iter().all(|line| line == "smoke-stage"), "{logged:?}");
    assert_eq!(lines(&project_log), [] as [String; 0]);
    let fired: Vec<&str> =
        of("system", "hook_started").iter().filter_map(|e| e["hook_name"].as_str()).collect();
    let on_bash = fired.iter().filter(|name| **name == "PreToolUse:Bash").count();
    assert_eq!(on_bash, logged.len() * 2, "the role's hook and the guard, and no other: {fired:?}");

    // What the worker was shown for each call: the first ran, the role's hook
    // refused the second, and the guard refused the third — reading from the
    // environment that the stage writes nothing.
    let shown: Vec<String> = events
        .iter()
        .filter(|event| event["type"] == "user")
        .flat_map(|event| event["message"]["content"].as_array().cloned().unwrap_or_default())
        .filter(|part| part["type"] == "tool_result")
        .map(|part| part["content"].to_string())
        .collect();
    let shown_once = |what: &str| shown.iter().filter(|text| text.contains(what)).count() == 1;
    assert!(shown.iter().any(|text| text.contains("smoke")), "{shown:?}");
    assert!(shown_once(ROLE_SAYS), "the role's hook refused nothing: {shown:?}");
    assert!(
        shown_once("the stage `smoke-stage` is marked `writes: false`: `git commit` is refused"),
        "the guard refused nothing: {shown:?}"
    );

    // What this run could not show, on this host.
    for (shown, what) in [
        (!theirs.is_empty(), "skills"),
        (home.join(".claude/CLAUDE.md").is_file(), "rules"),
        (user_has_hooks(&home), "hooks"),
    ] {
        let verdict =
            if shown { "has, and none was loaded" } else { "has none: nothing was shown" };
        println!("the user's own {what}: this host {verdict}");
    }
}

//! The guard (docs/adr/0012), fed tool calls the way Claude Code feeds a
//! hook: one described call on stdin, the stage's facts in the environment.
//! Claude Code is not started. Which commands are recognised is the core's
//! to test (`afkconvoy_core::guard`); here it is what the subcommand answers.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::json;

/// The facts of a stage `build` that may commit, as a start exports them.
const FACTS: [(&str, &str); 6] = [
    ("AFKCONVOY_STAGE", "build"),
    ("AFKCONVOY_WRITES", "true"),
    ("AFKCONVOY_CONSULT", "false"),
    ("AFKCONVOY_BASE_BRANCH", "trunk"),
    ("AFKCONVOY_ESCALATE_LABEL", "stuck"),
    ("AFKCONVOY_LOCK_LABEL", "halt"),
];

/// What the guard answered: its exit code, and what it said on each stream.
struct Answered {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs the guard on `described` with `facts` — and nothing else — as its
/// environment.
fn guard(described: &str, facts: &[(&str, &str)]) -> Answered {
    let home = tempfile::tempdir().unwrap();
    guard_at(described, facts, home.path(), false)
}

fn guard_at(
    described: &str,
    facts: &[(&str, &str)],
    home: &std::path::Path,
    terminal_only: bool,
) -> Answered {
    let mut command = Command::new(env!("CARGO_BIN_EXE_afkconvoy"));
    command.arg("guard").env_clear().env("HOME", home).env("XDG_CACHE_HOME", home);
    if terminal_only {
        command.arg("--terminal-input-only");
    }
    let mut child = command
        .envs(facts.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(described.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    Answered {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

#[test]
fn every_harness_refuses_worker_and_watch_input_but_allows_the_asking_session() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("afkconvoy/asking");
    std::fs::create_dir_all(&directory).unwrap();
    let record = json!({
        "session": {"id":"asking-1", "directory":home.path(), "stock":false,
            "harness":"codex", "model":null, "terminal":"asks", "transcript":home.path().join("session.log"), "session_started":1},
        "status":"running", "question":null, "terminal":null, "problem":null, "command":"",
    });
    std::fs::write(directory.join("terminal.json"), record.to_string()).unwrap();
    for harness in afkconvoy_core::harness::Harness::ALL {
        let describe = |line| {
            match harness {
                afkconvoy_core::harness::Harness::Pi => {
                    json!({"toolName":"bash", "input":{"command":line}})
                }
                afkconvoy_core::harness::Harness::Dsh => {
                    json!({"name":"bash", "arguments":{"command":line}})
                }
                afkconvoy_core::harness::Harness::Grok => {
                    json!({"toolName":"run_terminal_command", "toolInput":{"command":line}})
                }
                _ => json!({"tool_name":"Bash", "tool_input":{"command":line}}),
            }
            .to_string()
        };
        for terminal_only in [false, true] {
            for watch in [false, true] {
                let mut facts = FACTS.to_vec();
                facts.push(("AFKCONVOY_HARNESS", harness.key()));
                facts.push(("ORCA_TERMINAL_HANDLE", "worker"));
                if watch {
                    facts.push(("AFKCONVOY_WATCH", "host"));
                }
                let refused = guard_at(
                    &describe("orca terminal send --terminal asks --text yes --enter"),
                    &facts,
                    home.path(),
                    terminal_only,
                );
                assert_eq!(refused.code, 2, "{harness:?}: {}", refused.stderr);
                assert!(refused.stderr.contains("asking session's Terminal"), "{}", refused.stderr);
                assert_eq!(
                    guard_at(
                        &describe("orca terminal send --terminal another --text yes --enter"),
                        &facts,
                        home.path(),
                        terminal_only
                    )
                    .code,
                    0
                );
                facts.push(("ORCA_TERMINAL_HANDLE", "asks"));
                assert_eq!(
                    guard_at(
                        &describe("orca terminal send --terminal asks --text yes --enter"),
                        &facts,
                        home.path(),
                        terminal_only
                    )
                    .code,
                    0
                );
            }
        }
        // Outside a role, no stage facts are invented just to ask this guard.
        assert_eq!(
            guard_at(
                &describe("git commit -am step"),
                &[("AFKCONVOY_HARNESS", harness.key())],
                home.path(),
                true
            )
            .code,
            0
        );
    }
    // Existing sessions read the new identity at each call, not a launch-time copy.
    let mut record = record;
    record["session"]["terminal"] = json!("replacement");
    std::fs::write(directory.join("terminal.json"), record.to_string()).unwrap();
    assert_eq!(
        guard_at(
            &bash("orca terminal send --terminal replacement --enter"),
            &FACTS,
            home.path(),
            true
        )
        .code,
        2
    );
    assert_eq!(
        guard_at(&bash("orca terminal send --terminal asks --enter"), &FACTS, home.path(), true)
            .code,
        0
    );
}

/// A Bash call running `line`, as Claude Code describes one to a hook.
fn bash(line: &str) -> String {
    json!({"session_id": "s", "cwd": "/role", "hook_event_name": "PreToolUse",
        "tool_name": "Bash", "tool_input": {"command": line, "description": "a step"}})
    .to_string()
}

/// The facts with `name` set to `value`.
fn with(name: &str, value: &'static str) -> Vec<(&'static str, &'static str)> {
    FACTS.iter().map(|(n, v)| if *n == name { (*n, value) } else { (*n, *v) }).collect()
}

/// The facts of a stage `review` its workflow marks `writes: false`.
fn reviewing() -> Vec<(&'static str, &'static str)> {
    let facts = with("AFKCONVOY_WRITES", "false").into_iter();
    facts.map(|(n, v)| if n == "AFKCONVOY_STAGE" { (n, "review") } else { (n, v) }).collect()
}

#[test]
fn a_targeted_worker_cannot_push_either_destination_or_rewrite_its_target_record() {
    let mut facts = FACTS.to_vec();
    facts.push(("AFKCONVOY_TARGET_BRANCH", "delivery/ABC-123"));
    for line in [
        "git push origin HEAD:trunk",
        "git push origin HEAD:delivery/ABC-123",
        "git push origin :refs/heads/delivery/ABC-123",
        "git push origin HEAD:refs/convoy/target/7",
        "git push origin HEAD:convoy-target/7",
        "git push origin HEAD:refs/convoy/delivery/abcd",
        "git push origin HEAD:convoy-delivery/abcd",
        "git push origin HEAD:refs/convoy/delivery-pr/abcd",
        "git push origin HEAD:convoy-delivery-pr/abcd",
        "git push origin HEAD:refs/convoy/delivery-evidence/abcd",
        "git push origin HEAD:refs/convoy/delivery-settled/abcd",
        "git push origin HEAD:delivery/jira-ABC-123-abcd",
    ] {
        let answer = guard(&bash(line), &facts);
        assert_ne!(answer.code, 0, "{line}");
    }
    assert_eq!(guard(&bash("git push origin HEAD:issue-7"), &facts).code, 0);
}

#[test]
fn an_allowed_call_exits_zero_and_says_nothing() {
    for line in [
        "cargo test",
        "git commit -am step && git push origin HEAD",
        "gh issue edit 7 --add-label bug",
        "gh issue view 7 --comments",
        // `main` is not this repo's base branch.
        "git push origin main",
    ] {
        let answered = guard(&bash(line), &FACTS);
        assert_eq!((answered.code, answered.stdout.as_str()), (0, ""), "{line}");
        assert_eq!(answered.stderr, "", "{line}");
    }
    // A tool that runs no shell line is no business of the guard.
    for call in [
        json!({"tool_name": "Read", "tool_input": {"file_path": "/wt/a.rs"}}),
        json!({"tool_name": "Write", "tool_input": {"file_path": "/wt/a", "content": "git push"}}),
        json!({"tool_name": "Task"}),
    ] {
        assert_eq!(guard(&call.to_string(), &FACTS).code, 0, "{call}");
    }
}

#[test]
fn a_refused_call_exits_two_with_a_reason_the_worker_is_shown() {
    for (line, reason) in [
        ("gh issue edit 7 --remove-label flow:feature", "`flow:feature` is a flow label"),
        ("gh issue edit 7 --add-label stage:review", "`stage:review` is a stage label"),
        ("gh issue edit 7 --add-label stuck", "`stuck` is the escalate label"),
        ("gh issue edit 7 --add-label bug,halt", "`halt` is the lock label"),
        (
            "gh issue comment 7 --body '<!--convoy:stage stage=build verdict=pass-->'",
            "declare the verdict with `afkconvoy verdict`",
        ),
        ("cd /wt && gh pr merge 70 --squash", "merging a PR is refused"),
        ("git push origin HEAD:trunk", "pushes the base branch `trunk`"),
    ] {
        let answered = guard(&bash(line), &FACTS);
        assert_eq!(answered.code, 2, "{line}: {}", answered.stderr);
        assert!(answered.stderr.starts_with("afkconvoy guard: "), "{}", answered.stderr);
        assert!(answered.stderr.contains(reason), "{line}: {}", answered.stderr);
        assert_eq!(answered.stdout, "", "a hook's stdout is Claude Code's to read");
    }
}

#[test]
fn the_stage_is_read_from_the_environment() {
    let line = bash("git commit -am 'a fix' && git push origin HEAD");
    assert_eq!(guard(&line, &FACTS).code, 0);
    // The same call, in a stage its workflow marks `writes: false`…
    let answered = guard(&line, &reviewing());
    assert_eq!(answered.code, 2);
    assert!(
        answered.stderr.contains("the stage `review` is marked `writes: false`"),
        "{}",
        answered.stderr
    );
    // …and for a consult, whatever the stage is marked.
    let answered = guard(&line, &with("AFKCONVOY_CONSULT", "true"));
    assert_eq!(answered.code, 2);
    assert!(answered.stderr.contains("a consult writes no code"), "{}", answered.stderr);
    // Neither is kept from reading.
    assert_eq!(guard(&bash("git diff trunk...HEAD"), &reviewing()).code, 0);
}

#[test]
fn a_call_or_a_stage_it_cannot_read_is_refused() {
    for (described, reason) in [
        ("", "describes no tool call"),
        ("not json", "describes no tool call"),
        ("{\"tool_input\": {\"command\": \"ls\"}}", "describes no tool call"),
    ] {
        let answered = guard(described, &FACTS);
        assert_eq!(answered.code, 2, "{described:?}");
        assert!(answered.stderr.contains(reason), "{}", answered.stderr);
    }
    // Started by hand, outside a stage: no facts.
    let answered = guard(&bash("ls"), &[]);
    assert_eq!(answered.code, 2);
    assert!(answered.stderr.contains("AFKCONVOY_STAGE is not set"), "{}", answered.stderr);
    let answered = guard(&bash("ls"), &with("AFKCONVOY_WRITES", "maybe"));
    assert_eq!(answered.code, 2);
    assert!(answered.stderr.contains("neither true nor false"), "{}", answered.stderr);
}

#[test]
fn a_repo_s_heavy_command_is_refused_outside_afkconvoy_exclusive_for_bound_and_unbound_workers() {
    let home = tempfile::tempdir().unwrap();
    let declares = |dir: &std::path::Path, config: &str| {
        let worktree = dir.join("worktree");
        std::fs::create_dir_all(worktree.join(".afkconvoy-tmp")).unwrap();
        std::fs::create_dir_all(worktree.join("docs/agents")).unwrap();
        std::fs::write(worktree.join("docs/agents/afkconvoy.md"), config).unwrap();
        let tmp = dir.join("tmp-link");
        std::os::unix::fs::symlink(worktree.join(".afkconvoy-tmp"), &tmp).unwrap();
        tmp
    };
    let declaring = tempfile::tempdir().unwrap();
    let declaring = declares(
        declaring.path(),
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\n```\n\n\
         ```yaml heavy-commands\ncommands:\n  - cargo nextest\n  - ./gate.sh\n```\n",
    );
    let silent = tempfile::tempdir().unwrap();
    let silent =
        declares(silent.path(), "```yaml\nbase_branch: main\ngate_command: ./gate.sh\n```\n");
    let call = |line: &str| json!({"tool_name":"Bash", "tool_input":{"command":line}}).to_string();
    let bare = call("cd cli && cargo nextest run -p x");
    let wrapped = call("cd cli && afkconvoy exclusive -- cargo nextest run -p x");
    for terminal_only in [false, true] {
        let tmp = declaring.to_str().unwrap();
        let mut facts = FACTS.to_vec();
        facts.push(("TMPDIR", tmp));
        let refused = guard_at(&bare, &facts, home.path(), terminal_only);
        assert_eq!(refused.code, 2, "{}", refused.stderr);
        assert!(
            refused.stderr.contains("`cargo nextest` is a heavy command"),
            "{}",
            refused.stderr
        );
        assert!(
            refused.stderr.contains("`afkconvoy exclusive -- cargo nextest run -p x`"),
            "{}",
            refused.stderr
        );
        let allowed = guard_at(&wrapped, &facts, home.path(), terminal_only);
        assert_eq!(allowed.code, 0, "{}", allowed.stderr);
        for line in ["afkconvoy gate --fast", "afkconvoy land --issue 577", "afkconvoy cut"] {
            let allowed = guard_at(&call(line), &facts, home.path(), terminal_only);
            assert_eq!(allowed.code, 0, "{line}: {}", allowed.stderr);
        }
        let mut facts = FACTS.to_vec();
        facts.push(("TMPDIR", silent.to_str().unwrap()));
        for described in [&bare, &wrapped] {
            let allowed = guard_at(described, &facts, home.path(), terminal_only);
            assert_eq!(allowed.code, 0, "{}", allowed.stderr);
        }
    }
}

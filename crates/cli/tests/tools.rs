use crate::common;

use std::path::Path;

use common::{Answer, World};
use serde_json::json;

const ORCA: &str = "  orca is not installed: brew install --cask stablyai/orca/orca, or connect to \
     this host from Orca over SSH, whose remote runtime puts its CLI at ~/.orca/cli/bin/orca";
const CLAUDE: &str = "  claude is not installed: curl -fsSL https://claude.ai/install.sh | bash";

/// The three starts → the answer of each.
fn starts(world: &World) -> Vec<Answer> {
    let mut answers = vec![world.afk(&["plan", "--now", "1000"])];
    answers.extend(worker_starts(world, &[]));
    answers
}

/// The two starts that start workers, each with `extra` after it → the
/// answer of each: its last line.
fn worker_starts(world: &World, extra: &[&str]) -> Vec<Answer> {
    let run = "run --tracker default --foreground --instance alpha --host h1 --now 1000 --ticks 1 --second-ms 0";
    let tick = "tick --instance alpha --host h1 --now 1000";
    let answer = |start: &str| {
        let out = world
            .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
            .args(start.split(' '))
            .args(extra)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let last = stdout.lines().last().unwrap_or_default();
        Answer {
            json: serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {stdout}")),
            code: out.status.code().unwrap(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    };
    vec![answer(run), answer(tick)]
}

/// A repo with a backlog, whose remote logs every push.
fn backlog() -> World {
    let world = World::new();
    world.kit_command("claude-code", afkconvoy_core::harness::Harness::ClaudeCode.stock());
    world.issue(7, &["ready-for-agent"]);
    world.watch_pushes(None);
    world
}

/// The lines of a refusal that each name one fault.
fn faults(error: &str) -> Vec<&str> {
    error.lines().skip(1).collect()
}

fn nothing_was_done(world: &World) {
    assert_eq!(world.probes(), [] as [String; 0], "no probe was pushed");
    assert_eq!(world.pushes(), [] as [String; 0], "no ref was written");
    assert_eq!(world.remote(&["for-each-ref", "--format=%(refname)"]), "refs/heads/main");
    let calls: Vec<Vec<String>> = serde_json::from_value(world.gh()["calls"].clone()).unwrap();
    assert!(
        calls.iter().all(|call| call.starts_with(&["config".into(), "get".into(), "user".into()])
            || call.starts_with(&["api".into(), "rate_limit".into()])),
        "a refused start may inspect local login/quota observations, never issue/PR facts or writes: {calls:?}"
    );
    let asked = world.orca_calls();
    let only_asked = asked.iter().all(|call| call == &["status"] || call == &["repo", "list"]);
    assert!(only_asked, "orca was asked what it is, and nothing more: {asked:?}");
    assert_eq!(world.orca()["worktrees"], json!([]), "no worktree was cut");
    assert_eq!(world.orca()["terminals"], json!([]), "and no worker started");
    assert!(!world.clone.join(".git/convoy-layout").exists(), "no layout was noted");
}

#[test]
fn a_start_with_a_command_missing_refuses_naming_the_line_that_installs_it() {
    for (name, line) in [
        ("git", "  git is not installed: brew install git"),
        ("gh", "  gh is not installed: brew install gh"),
        ("orca", ORCA),
    ] {
        let world = backlog();
        world.take_off_path(name);
        for start in starts(&world) {
            assert_eq!(start.code, 1);
            let error = start.error();
            assert_eq!(faults(&error), [line], "{error}");
            assert!(error.contains("nothing was started"), "{error}");
        }
        // the checks below run git, gh and orca themselves
        std::fs::remove_dir_all(world.root.join("path")).unwrap();
        nothing_was_done(&world);
    }
}

/// Puts the stand-in orca where Orca's SSH remote runtime puts its CLI, in
/// the home directory → that file.
fn remote_cli(world: &World) -> std::path::PathBuf {
    let dir = world.home().join(".orca/cli/bin");
    std::fs::create_dir_all(&dir).unwrap();
    let built = Path::new(env!("CARGO_BIN_EXE_afkconvoy")).parent().unwrap().join("examples");
    // a wrapper of absolute paths, as the runtime's is: a file of its own
    let wrapper = format!("#!/bin/sh\nexec '{}' \"$@\"\n", built.join("fake_orca").display());
    std::fs::write(dir.join("orca"), wrapper).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.join("orca"), std::fs::Permissions::from_mode(0o755)).unwrap();
    dir.join("orca")
}

/// The orca `doctor` says it found: {name, state, path, from}.
fn doctor_orca(world: &World) -> serde_json::Value {
    let answer = world.afk(&["doctor"]).ok();
    let tools = answer["host"]["tools"].as_array().unwrap().clone();
    tools.into_iter().find(|tool| tool["name"] == "orca").unwrap()
}

#[test]
fn an_orca_only_where_the_ssh_remote_runtime_puts_its_cli_is_found_and_called() {
    let world = backlog();
    world.take_off_path("orca");
    let remote = remote_cli(&world);
    let orca = doctor_orca(&world);
    assert_eq!(orca["state"], "ready", "{orca}");
    assert_eq!(orca["from"], "remote_cli", "{orca}");
    assert_eq!(orca["path"], remote.to_str().unwrap(), "{orca}");
    // every start goes to work, its orca calls to that CLI
    for start in starts(&world) {
        start.ok();
    }
    assert_eq!(world.orca()["terminals"].as_array().unwrap().len(), 1, "a worker started");
}

#[test]
fn an_orca_on_the_path_wins_over_the_ssh_remote_runtimes_cli() {
    let world = backlog();
    remote_cli(&world);
    let orca = doctor_orca(&world);
    assert_eq!(orca["state"], "ready", "{orca}");
    assert_eq!(orca["from"], "path", "{orca}");
    let on_path = orca["path"].as_str().unwrap();
    assert!(!on_path.contains(".orca/cli/bin"), "{orca}");
}

#[test]
fn a_start_without_claude_names_its_native_installer() {
    let world = backlog();
    world.shell_knows("claude", None);
    world.logged_out("claude");
    for start in starts(&world) {
        let error = start.error();
        assert_eq!(faults(&error), [CLAUDE], "missing, and not also logged out: {error}");
    }
    assert!(!world.shell_asked().contains(&"claude auth status".to_string()));
    nothing_was_done(&world);
}

#[test]
fn every_fault_is_named_in_the_one_run() {
    let world = backlog();
    for name in ["git", "gh", "orca"] {
        world.take_off_path(name);
    }
    world.shell_knows("claude", None);
    for start in starts(&world) {
        let error = start.error();
        assert_eq!(
            faults(&error),
            [
                "  git is not installed: brew install git",
                "  gh is not installed: brew install gh",
                ORCA,
                CLAUDE
            ],
            "{error}"
        );
    }
    assert_eq!(world.shell_asked(), ["type -- 'claude'"; 3], "nothing missing was asked its login");
    std::fs::remove_dir_all(world.root.join("path")).unwrap();
    nothing_was_done(&world);
}

#[test]
fn a_start_logged_out_refuses_naming_the_line_that_logs_in() {
    let gh = "  gh is not logged in: gh auth login";
    let claude = "  claude is not logged in: claude auth login";
    for (out, lines) in
        [(&["gh"][..], &[gh][..]), (&["claude"], &[claude]), (&["gh", "claude"], &[gh, claude])]
    {
        let world = backlog();
        for tool in out {
            world.logged_out(tool);
        }
        for start in starts(&world) {
            assert_eq!(start.code, 1);
            let error = start.error();
            assert_eq!(faults(&error), lines, "{error}");
        }
        nothing_was_done(&world);
    }
}

#[test]
fn a_missing_command_and_a_logged_out_one_are_named_together() {
    let world = backlog();
    world.take_off_path("orca");
    world.logged_out("gh");
    for start in starts(&world) {
        let error = start.error();
        assert_eq!(faults(&error), ["  gh is not logged in: gh auth login", ORCA], "{error}");
    }
}

#[test]
fn a_start_in_a_checkout_orca_does_not_know_refuses_naming_the_line_that_adds_it() {
    let world = backlog();
    world.with_orca(|orca| orca["repos"] = json!([]));
    let checkout = common::git(&world.clone, &["rev-parse", "--show-toplevel"]);
    let line = format!("  orca does not know this checkout: orca repo add --path '{checkout}'");
    for start in starts(&world) {
        assert_eq!(start.code, 1);
        let error = start.error();
        assert_eq!(faults(&error), [line.as_str()], "{error}");
        assert!(error.contains("nothing was started"), "{error}");
    }
    nothing_was_done(&world);
    assert_eq!(world.claims(), []);

    // the line was run: the next start goes to work
    world.with_orca(|orca| orca["repos"] = json!([{"id": "repo-1", "path": checkout}]));
    for start in starts(&world) {
        start.ok();
    }
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
}

#[test]
fn a_start_with_orca_not_running_refuses_naming_the_line_that_starts_it() {
    let world = backlog();
    // …whatever it would say of this checkout, were it asked
    world.with_orca(|orca| {
        orca["stopped"] = json!(true);
        orca["repos"] = json!([]);
    });
    for start in starts(&world) {
        assert_eq!(start.code, 1);
        let error = start.error();
        assert_eq!(faults(&error), ["  orca is not running: orca open"], "{error}");
    }
    assert_eq!(world.orca_calls(), [["status"]; 3], "it was asked nothing more");
    nothing_was_done(&world);
    assert_eq!(world.claims(), []);
}

#[test]
fn an_orca_fault_is_named_beside_the_others_in_the_one_run() {
    let world = backlog();
    world.with_orca(|orca| orca["stopped"] = json!(true));
    world.logged_out("gh");
    for start in starts(&world) {
        let error = start.error();
        assert_eq!(
            faults(&error),
            ["  gh is not logged in: gh auth login", "  orca is not running: orca open"],
            "{error}"
        );
    }
}

#[test]
fn claude_is_checked_only_as_the_first_word_of_the_worker_launch_command() {
    let world = backlog();
    world.shell_knows("claude", None);
    world.logged_out("claude");
    world.shell_knows("kim", Some("kim is an alias for claude --dangerously-skip-permissions"));
    for command in ["agent", "kim"] {
        world.kit_command("claude-code", command);
        for start in worker_starts(&world, &[]) {
            // all a start says is that the probe that passed still stands
            let said: Vec<&str> =
                start.stderr.lines().filter(|line| !line.starts_with("note: ")).collect();
            assert_eq!(said, [] as [&str; 0], "{command}");
            start.ok();
        }
    }
    let asked = world.shell_asked();
    assert!(!asked.contains(&"claude auth status".to_string()), "{asked:?}");
    assert_eq!(world.claims(), [(7, "alpha".to_string())], "the fleet went to work");
}

#[test]
fn on_a_custom_provider_plan_reads_the_host_kit_and_checks_its_command() {
    let world = backlog();
    world.kit_command("claude-code", "agent");
    world.shell_knows("claude", None);
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("ANTHROPIC_BASE_URL", "https://provider.example/v1")
        .args(["plan", "--now", "1000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let asked = world.shell_asked();
    assert_eq!(asked.len(), 4, "command eligibility, GitHub login and proxy exports are read");
    let mut once = asked.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), asked.len(), "each question is asked once a look: {asked:?}");
    assert_eq!(asked[0], "type -- 'agent'");
    assert!(asked.contains(&"gh auth status".to_string()), "{asked:?}");
    assert!(asked.iter().any(|line| line.starts_with("printf 'AFKCONVOY_PROXY")));
}

#[test]
fn a_wrapper_that_resolves_to_nothing_is_refused_with_no_install_line_invented() {
    let world = backlog();
    world.shell_knows("ckim", None);
    world.kit_command("claude-code", "ckim --yolo");
    let passed: [&str; 0] = [];
    for start in worker_starts(&world, &passed) {
        let error = start.error();
        assert!(error.starts_with("the worker launch command \"ckim --yolo\" cannot"), "{error}");
        assert!(!error.contains("install"), "{error}");
    }

    // …and beside a command that is missing, in the one run
    world.take_off_path("orca");
    for start in worker_starts(&world, &passed) {
        let error = start.error();
        let said = faults(&error);
        assert_eq!(said[0], ORCA, "{error}");
        assert!(said[1].contains("\"ckim\" resolves to nothing in a login shell"), "{error}");
        assert_eq!(said.len(), 2, "{error}");
    }
    std::fs::remove_dir_all(world.root.join("path")).unwrap();
    nothing_was_done(&world);
}

#[test]
fn the_commands_that_make_one_transition_by_hand_check_no_tool() {
    let world = backlog();
    world.logged_out("gh");
    world.afk(&["claim", "--issue", "7", "--instance", "alpha", "--now", "1000"]).ok();
    world.afk(&["scan"]).ok();
    assert_eq!(world.shell_asked(), [] as [String; 0]);
}

/// The install lines are written out where a human reads them before any
/// start: each must be the line a refusal gives (the formula's: `release.rs`).
#[test]
fn the_readme_gives_the_install_lines_a_refusal_gives() {
    use afkconvoy_core::tools::Tool;
    let readme = include_str!("../../../README.md");
    assert!(readme.contains(&format!("`{}`", Tool::Orca.install())));
    assert!(readme.contains(&format!("`{}`", Tool::Claude.install())));
    assert!(readme.contains(&format!("`{}`", Tool::Pi.install())));
    assert!(!readme.contains("Homebrew does not"), "orca has an install line now");
    // …and the two an installed orca is refused with.
    let said = afkconvoy_core::tools::refusal(
        &[(Tool::Orca, afkconvoy_core::tools::Found::NotRunning)],
        None,
        None,
    );
    let start = said.unwrap().rsplit(": ").next().unwrap().to_string();
    let add = afkconvoy_core::tools::add_checkout("\"$PWD\"").replace('\'', "");
    for line in [start, add] {
        assert!(readme.contains(&format!("`{line}`")), "the README lost `{line}`");
        assert!(
            include_str!("../src/lib.rs").contains(&format!("`{line}`")),
            "the guide lost `{line}`"
        );
    }
}

use crate::common;

use std::path::Path;

use common::{Answer, World};
use serde_json::{Value, json};

const PROVIDER: &str = "https://provider.example/v1";
const SECRET: &str = "sk-not-for-workers";

/// Runs the CLI as a fleet started through a provider wrapper would be: the
/// provider and its key in the environment.
fn wrapped(world: &World, args: &[&str]) -> Answer {
    answer(world, args, true)
}

fn answer(world: &World, args: &[&str], wrapped: bool) -> Answer {
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    if wrapped {
        command.env("ANTHROPIC_BASE_URL", PROVIDER).env("ANTHROPIC_AUTH_TOKEN", SECRET);
    }
    let out = command.args(args).output().unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().last().unwrap_or_default();
    Answer {
        json: serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {stdout}")),
        code: out.status.code().unwrap(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

const RUN: [&str; 11] = [
    "run",
    "--instance",
    "alpha",
    "--host",
    "h1",
    "--now",
    "1000",
    "--ticks",
    "1",
    "--second-ms",
    "0",
];
const TICK: [&str; 7] = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];

/// The command each worker's terminal was started with.
fn started(world: &World) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    terminals.iter().map(|t| t["command"].as_str().unwrap().to_string()).collect()
}

fn warnings(answer: &Answer) -> Vec<&str> {
    answer.stderr.lines().filter(|line| line.starts_with("warning: ")).collect()
}

fn backlog() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world
}

/// Whether any file under `dir` holds `needle`.
fn written_anywhere(dir: &Path, needle: &str, skip: &[&str]) -> Vec<String> {
    let mut found = vec![];
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip.contains(&name.as_str()) || path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            found.extend(written_anywhere(&path, needle, skip));
        } else if let Ok(bytes) = std::fs::read(&path)
            && String::from_utf8_lossy(&bytes).contains(needle)
        {
            found.push(path.display().to_string());
        }
    }
    found
}

#[test]
fn on_stock_claude_the_stock_command_is_used_and_nobody_is_asked() {
    let world = backlog();
    world.kit_command("claude-code", afkconvoy_core::harness::Harness::ClaudeCode.stock());
    let ran = answer(&world, &RUN, false);
    assert_eq!(warnings(&ran), [] as [&str; 0], "{}", ran.stderr);
    assert_eq!(ran.ok()["stopped"], true);
    assert_eq!(
        started(&world),
        [
            r#"claude --dangerously-skip-permissions --settings '{"skipDangerousModePermissionPrompt":true}'"#
        ]
    );
    // checked to resolve like any other, and no alias was listed
    assert_eq!(world.shell_asked(), ["type -- 'claude'", "gh auth status", "claude auth status"]);
}

#[test]
fn on_stock_claude_a_command_that_was_passed_is_still_the_answer() {
    let world = backlog();
    world.kit_command("claude-code", "agent --fast");
    answer(&world, &TICK, false).ok();
    assert_eq!(started(&world), ["agent --fast"]);
}

#[test]
fn a_command_given_for_a_harness_by_its_key_is_that_harness_s() {
    let world = backlog();
    let passed = "agent  --model=fast ";
    world.kit_command("claude-code", passed);
    answer(&world, &TICK, false).ok();
    assert_eq!(started(&world), ["agent  --model=fast "], "what follows the `=`, as given");
}

#[test]
fn on_a_custom_provider_the_passed_command_is_handed_over_exactly_as_passed() {
    let world = backlog();
    // Everything past the first word is the human's business.
    let passed = "kim  --model 'big one' \"$HOME\" ; true ";
    world.shell_knows(
        "kim",
        Some("kim is an alias for ANTHROPIC_BASE_URL=x claude --dangerously-skip-permissions"),
    );
    world.kit_command("claude-code", passed);
    let ran = wrapped(&world, &RUN);
    assert_eq!(warnings(&ran), [] as [&str; 0], "{}", ran.stderr);
    ran.ok();

    assert_eq!(started(&world), [passed], "not trimmed, not composed, not appended to");
    assert_eq!(
        world.shell_asked(),
        ["type -- 'kim'", "gh auth status"],
        "only its first word was looked at, and what it logs in to is its own business"
    );

    // nothing of the fleet's own environment goes to orca, on any call
    let calls = serde_json::to_string(&world.orca()["calls"]).unwrap();
    assert!(!calls.contains(SECRET) && !calls.contains(PROVIDER), "{calls}");
    let create = world.orca()["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|call| call[0] == "terminal" && call[1] == "create")
        .cloned()
        .unwrap();
    let create: Vec<String> = serde_json::from_value(create).unwrap();
    // …but for starting it under `nice`, with the worktree's own `TMPDIR`
    let dir = Path::new(create[3].strip_prefix("path:").unwrap());
    let niced = common::niced(dir, passed);
    assert_eq!(create[create.len() - 3..create.len() - 1], ["--command", &niced], "{create:?}");
    assert!(create.len() <= 7, "a worktree, a command, and nothing else: {create:?}");

    // …and the command is in no file: not the brief, not a note, not the repo
    // (orca.json is the stand-in orca's own memory of what it was handed)
    assert_eq!(
        written_anywhere(&world.root, "big one", &["orca.json", "kit.json"]),
        [] as [String; 0]
    );
    assert_eq!(written_anywhere(&world.root, SECRET, &[]), [] as [String; 0]);
}

#[test]
fn a_command_that_resolves_to_nothing_is_refused_before_any_worker_is_started() {
    let world = backlog();
    world.shell_knows("ckim", None);
    world.kit_command("claude-code", "ckim --dangerously-skip-permissions");
    let dispatch =
        ["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1", "--now", "1000"];
    for start in [&RUN[..], &TICK[..], &dispatch[..]] {
        let error = wrapped(&world, start).error();
        assert!(error.contains("\"ckim\" resolves to nothing in a login shell"), "{error}");
    }
    assert_eq!(world.orca()["worktrees"], json!([]));
    assert_eq!(started(&world), [] as [String; 0]);
    assert_eq!(world.claims(), [], "no claim was taken for a worker that cannot start");
}

#[test]
fn an_empty_command_is_refused() {
    let world = backlog();
    world.kit_command("claude-code", "  ");
    let error = wrapped(&world, &TICK).error();
    assert!(error.contains("is empty"), "{error}");
    assert_eq!(world.claims(), []);
}

#[test]
fn a_command_with_no_visible_unattended_flag_is_warned_about_not_refused() {
    let world = backlog();
    world.shell_knows("kim", Some("kim is an alias for ANTHROPIC_BASE_URL=x claude --model k"));
    world.kit_command("claude-code", "kim");
    let ran = wrapped(&world, &TICK);
    let said = warnings(&ran).join("\n");
    assert!(said.contains("\"kim\" shows no unattended flag"), "{}", ran.stderr);
    assert!(said.contains("claude --model k"), "what it resolves to is shown: {said}");
    let ticked: Value = ran.ok();
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(started(&world), ["kim"]);
}

#[test]
fn a_script_that_could_carry_the_flag_inside_is_not_warned_about() {
    let world = backlog();
    world.kit_command("claude-code", "agent");
    let ran = wrapped(&world, &TICK);
    assert_eq!(warnings(&ran), [] as [&str; 0], "{}", ran.stderr);
    ran.ok();
    assert_eq!(started(&world), ["agent"]);
}

#[test]
fn a_custom_provider_still_uses_the_host_kit_without_a_terminal() {
    let world = backlog();
    wrapped(&world, &TICK).ok();
    assert_eq!(started(&world), ["agent"]);
}

#[test]
fn removed_worker_command_names_the_kit() {
    let world = backlog();
    let error = world.afk(&["tick", "--instance", "alpha", "--worker-command", "agent"]).error();
    assert!(error.contains("afkconvoy kit"), "{error}");
    assert!(started(&world).is_empty());
}

#[test]
fn no_host_kit_refuses_noninteractive_starts_before_claiming() {
    let world = backlog();
    std::fs::remove_file(world.kit_path()).unwrap();
    for start in [&RUN[..], &TICK[..]] {
        let error = world.afk(start).error();
        assert!(error.contains("afkconvoy kit"), "{error}");
    }
    assert!(started(&world).is_empty());
    assert!(world.claims().is_empty());
}

#[test]
fn kit_without_terminal_writes_nothing() {
    let world = backlog();
    let before = std::fs::read(world.kit_path()).unwrap();
    assert!(world.afk(&["kit"]).error().contains("terminal"));
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), before);
}

#[test]
fn plan_and_doctor_show_the_host_kit() {
    let world = backlog();
    world.kit_command("pi", "pi");
    let mut kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    kit["harnesses"][1]["models"] = json!([
        {"model": "a/fast", "probed": {"at": 900, "seconds": 1.5}},
        {"model": "a/slow", "probed": {"at": 901, "seconds": 9.0}},
    ]);
    std::fs::write(world.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    for args in [&["plan", "--now", "1000"][..], &["doctor", "--host", "h1", "--now", "1000"][..]] {
        let answer = world.afk(args).json;
        let kit = if args[0] == "plan" { &answer["host_kit"] } else { &answer["host"]["host_kit"] };
        assert_eq!(kit["path"], world.kit_path().display().to_string());
        assert_eq!(kit["harnesses"][0]["harness"], "claude-code");
        assert_eq!(kit["harnesses"][0]["command"], "agent");
        assert_eq!(
            kit["stock_consult"],
            json!({"harness": "claude-code", "model": null, "named": false, "unmatched": null})
        );
        assert_eq!(kit["harnesses"][1]["harness"], "pi");
        assert_eq!(kit["harnesses"][1]["models"][0]["model"], "a/fast");
        assert_eq!(kit["harnesses"][1]["models"][1]["probed"]["at"], 901);
    }
    // The stock consult the human named, on a model the host kit holds.
    kit["stock_consult"] = json!({"harness": "pi", "model": "a/slow"});
    std::fs::write(world.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    let shown = world.afk(&["plan", "--now", "1000"]).json;
    assert_eq!(
        shown["host_kit"]["stock_consult"],
        json!({"harness": "pi", "model": "a/slow", "named": true, "unmatched": null})
    );
    // …and one it does not hold is not followed: the host kit's first, and
    // said.
    kit["stock_consult"] = json!({"harness": "pi", "model": "a/none"});
    std::fs::write(world.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    let shown = world.afk(&["plan", "--now", "1000"]).json;
    let stock = &shown["host_kit"]["stock_consult"];
    assert_eq!(stock["harness"], "claude-code", "{shown}");
    assert!(stock["unmatched"].as_str().unwrap().contains("`a/none`"), "{stock}");
}

#[test]
fn a_missing_required_harness_refuses_the_entire_start() {
    let world = backlog();

    // The feature uses Claude Code; the audit workflow also calls for it.
    let mut kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    kit["harnesses"].as_array_mut().unwrap().retain(|held| held["harness"] != "claude-code");
    std::fs::write(world.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    let error = world.afk(&TICK).error();
    assert!(error.contains("claude-code") && error.contains("afkconvoy kit"), "{error}");
    assert!(world.claims().is_empty());
    assert!(started(&world).is_empty());
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]);
    assert_ne!(doctor.code, 0, "{}", doctor.json);
    assert!(doctor.json["findings"].to_string().contains("host kit"));
}

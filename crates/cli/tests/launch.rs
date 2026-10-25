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

const RUN: [&str; 12] = [
    "run",
    "--foreground",
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
    // Only resolution and authentication are checked, with no model discovery.
    let asked = world.shell_asked();
    assert!(asked.iter().any(|line| line == "gh auth status"));
    assert!(
        asked.iter().all(|line| ["type -- 'claude'", "gh auth status", "claude auth status"]
            .contains(&line.as_str())),
        "{asked:?}"
    );
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
    let asked = world.shell_asked();
    assert!(asked.iter().any(|line| line == "type -- 'kim'"));
    assert!(
        asked.iter().all(|line| ["type -- 'kim'", "gh auth status"].contains(&line.as_str())),
        "opaque wrapper credentials are never inspected: {asked:?}"
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
fn an_empty_command_discovers_the_harness_default() {
    let world = backlog();
    world.kit_command("claude-code", "  ");
    let ran = wrapped(&world, &TICK);
    assert_eq!(ran.json["errors"], json!([]));
    assert!(started(&world)[0].starts_with("claude --dangerously-skip-permissions"));
    assert!(ran.stderr.contains("discovered claude-code"), "{}", ran.stderr);
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

/// A host with Claude Code (`default opus`), pi (`openai/gpt-5.5`) and Codex
/// installed, Codex listing no model, and DSH not installed; every model
/// answers its probe.
fn host_to_kit(world: &World) {
    world.shell_claude_models(&["default", "opus"]);
    world.shell_models(&["openai/gpt-5.5"]);
    world.shell_knows("dsh-tui", None);
    world.probes_answer();
}

/// The host kit as written, without when each model was probed.
fn kit_unprobed(world: &World) -> Value {
    let mut kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    for held in kit["harnesses"].as_array_mut().unwrap() {
        for seen in held["models"].as_array_mut().unwrap() {
            assert!(seen["probed"]["at"].is_u64(), "every model held was probed: {seen}");
            seen.as_object_mut().unwrap().remove("probed");
        }
    }
    kit
}

#[test]
fn no_host_kit_has_run_write_it_and_start_its_first_tick_while_a_tick_refuses() {
    let world = backlog();
    std::fs::remove_file(world.kit_path()).unwrap();
    host_to_kit(&world);
    let error = world.afk(&TICK).error();
    assert!(error.contains("afkconvoy kit"), "{error}");
    assert!(world.claims().is_empty());
    // `run` with no terminal asks nothing: it writes the host kit, says so,
    // and ticks.
    let ran = answer(&world, &RUN, false);
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    assert!(ran.stderr.contains("wrote the host kit at"), "{}", ran.stderr);
    assert!(ran.stderr.contains("left out: dsh: not installed"), "{}", ran.stderr);
    let kit = kit_unprobed(&world);
    assert_eq!(kit["harnesses"][0]["harness"], "claude-code");
    assert_eq!(
        kit["harnesses"][0]["models"],
        json!([{"model": "default", "good_at": "not researched"}, {"model": "opus", "good_at": "not researched"}])
    );
    assert_eq!(started(&world).len(), 1, "its first tick started a worker");
}

/// What the run log of `world`'s repo says of the host kit's refreshes, as
/// `run` would print it.
fn refreshes(world: &World) -> Vec<String> {
    let Ok(files) = std::fs::read_dir(world.cache().join("afkconvoy/runs")) else { return vec![] };
    let mut said = vec![];
    for file in files.map(|entry| entry.unwrap().path()) {
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let entry: afkconvoy::log::Entry = serde_json::from_str(line).unwrap();
            if entry.said().starts_with("refresh_kit") {
                said.push(entry.said());
            }
        }
    }
    said
}

/// A tick of `instance` at `now`, in a process of its own.
fn tick_at(world: &World, instance: &str, now: u64) -> std::process::Child {
    let now = now.to_string();
    let args = ["tick", "--instance", instance, "--host", "h1", "--now", &now];
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    command.args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    command.spawn().unwrap()
}

#[test]
fn a_tick_a_day_later_writes_the_host_kit_again_once_for_every_fleet_of_the_host() {
    let world = backlog();
    std::fs::remove_file(world.kit_path()).unwrap();
    host_to_kit(&world);
    assert_eq!(answer(&world, &RUN, false).code, 0);
    let models = |world: &World| kit_unprobed(world)["harnesses"][0]["models"].clone();
    assert_eq!(
        models(&world),
        json!([{"model": "default", "good_at": "not researched"}, {"model": "opus", "good_at": "not researched"}])
    );
    assert_eq!(world.afk(&["plan", "--now", "1000"]).json["host_kit"]["refreshed"], 1000);
    // Claude Code lists `sonnet` now, and no longer `opus`.
    world.shell_claude_models(&["default", "sonnet"]);
    let day = 86_400;
    let mut tick = tick_at(&world, "alpha", 1000 + day - 1);
    tick.wait().unwrap();
    assert!(refreshes(&world).is_empty(), "within the day: {:?}", refreshes(&world));
    // Several fleets tick a day later at once: one of them writes it again.
    let ticks: Vec<_> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|instance| tick_at(&world, instance, 1000 + day))
        .collect();
    for mut tick in ticks {
        tick.wait().unwrap();
    }
    assert_eq!(
        refreshes(&world),
        ["refresh_kit (fleet): the host kit was written again: added claude-code sonnet; \
          dropped claude-code opus"]
    );
    assert_eq!(
        models(&world),
        json!([{"model": "default", "good_at": "not researched"}, {"model": "sonnet", "good_at": "not researched"}])
    );
    let plan = world.afk(&["plan", "--now", &(1000 + day).to_string()]);
    assert_eq!(plan.json["host_kit"]["refreshed"], 1000 + day);
    // The rest of that day, no tick writes it again.
    tick_at(&world, "beta", 1000 + 2 * day - 1).wait().unwrap();
    assert_eq!(refreshes(&world).len(), 1);
}

#[test]
fn kit_with_no_terminal_writes_what_this_host_can_start_and_says_what_it_left_out() {
    let world = backlog();
    world.kit_command("dsh", "dsh");
    world.with_kit(|kit| {
        kit["harnesses"][0]["models"] = json!([{"model": "opus", "good_at": "hard work"}]);
        kit["stock_consult"] = json!({"harness": "claude-code", "model": "opus"});
    });
    host_to_kit(&world);
    let written = world.afk(&["kit"]);
    assert_eq!(written.json["written"], true);
    assert_eq!(written.json["path"], world.kit_path().display().to_string());
    let kit = kit_unprobed(&world);
    assert_eq!(written.json["kit"]["harnesses"].as_array().unwrap().len(), 2);
    assert_eq!(
        kit,
        json!({
            "harnesses": [
                {"harness": "claude-code", "command": "agent",
                    "models": [{"model": "default", "good_at": "not researched"}, {"model": "opus", "good_at": "not researched"}]},
                {"harness": "pi", "command": "pi", "models": [{"model": "openai/gpt-5.5", "good_at": "not researched"}]},
            ],
            "stock_consult": {"harness": "claude-code", "model": "opus"},
        })
    );
    let left_out: Vec<(&str, &str)> = written.json["left_out"]
        .as_array()
        .unwrap()
        .iter()
        .map(|out| (out["harness"].as_str().unwrap(), out["why"].as_str().unwrap()))
        .collect();
    assert_eq!(left_out.len(), 2, "{left_out:?}");
    assert!(left_out[0].0 == "dsh" && left_out[0].1.starts_with("not installed"), "{left_out:?}");
    assert!(
        left_out[1].0 == "codex" && left_out[1].1.starts_with("listing failed"),
        "{left_out:?}"
    );
    assert!(written.stderr.contains("left out: codex: listing failed"), "{}", written.stderr);
    // Written again with nothing changed, it is the same but for when.
    assert_eq!(world.afk(&["kit"]).json["written"], true);
    assert_eq!(kit_unprobed(&world), kit);
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
        json!({"harness": "claude-code", "model": null, "named": true, "unmatched": null})
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
    world.shell_knows("claude", None);
    let error = world.afk(&TICK).error();
    assert!(error.contains("claude") && error.contains("install"), "{error}");
    assert!(world.claims().is_empty());
    assert!(started(&world).is_empty());
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1000"]);
    assert_ne!(doctor.code, 0, "{}", doctor.json);
    assert!(doctor.json["findings"].to_string().contains("claude"));
}

#[test]
fn kit_holds_the_names_that_answered_as_one_model_once_by_its_id() {
    let world = backlog();
    world.kit_command("codex", afkconvoy_core::harness::Harness::Codex.stock());
    world.with_kit(|kit| {
        kit["harnesses"][1]["models"] = json!([{"model": "gpt-6-sol", "good_at": "hard work"}]);
    });
    // Claude Code lists `default` and `opus`, which it shows as `Opus 5.5`.
    let initialized = json!({"type": "control_response", "response": {"subtype": "success",
        "response": {"models": [{"value": "default", "displayName": "Default (recommended)"},
            {"value": "opus", "displayName": "Opus 5.5"}]}}});
    std::fs::write(world.root.join("shell-claude-models"), initialized.to_string()).unwrap();
    world.shell_models(&["openai-codex/gpt-6-sol"]);
    world.shell_server_models(&["gpt-6-sol"]);
    world.shell_knows("dsh-tui", None);
    // Each probe says which model answered in its harness's own way: Claude
    // Code's JSON, pi's events, and the session file Codex's thread names.
    let claude = r#"{"result":"OK","modelUsage":{"claude-opus-5-5":{"outputTokens":4}}}"#;
    world.probes_answer_as(&[
        ("default", claude, None),
        ("opus", claude, None),
        (
            "openai-codex/gpt-6-sol",
            r#"{"type":"message_end","message":{"role":"assistant","provider":"openai-codex","model":"gpt-6-sol"}}"#,
            None,
        ),
        (
            "gpt-6-sol",
            r#"{"type":"thread.started","thread_id":"t-gpt-6-sol"}"#,
            Some(r#"{"type":"turn_context","payload":{"cwd":"/","model":"gpt-6-sol"}}"#),
        ),
    ]);
    let written = world.afk(&["kit"]);
    assert_eq!(written.json["written"], true, "{}", written.stderr);
    let kit = kit_unprobed(&world);
    let models = |harness: &str| {
        let held = kit["harnesses"].as_array().unwrap().iter().find(|h| h["harness"] == harness);
        held.map(|held| held["models"].clone()).unwrap_or_default()
    };
    assert_eq!(
        models("claude-code"),
        json!([{"model": "claude-opus-5-5", "good_at": "not researched",
            "names": ["default", "opus", "Default (recommended)", "Opus 5.5"]}])
    );
    // One model on two harnesses is held once on each, with one `good_at`.
    assert_eq!(models("codex"), json!([{"model": "gpt-6-sol", "good_at": "not researched"}]));
    assert_eq!(
        models("pi"),
        json!([{"model": "openai-codex/gpt-6-sol", "good_at": "not researched"}])
    );
    // The session file the probe kept is gone.
    let sessions = written_anywhere(&world.cache(), "turn_context", &[]);
    assert_eq!(sessions, [] as [String; 0]);
    // A worker is started on the id, whatever name its issue's label gives.
    world.issue(8, &["ready-for-agent", "harness:claude-code", "model:Opus 5.5"]);
    world.issue(9, &["ready-for-agent", "harness:pi", "model:GPT-6-SOL"]);
    let planned = world.afk(&["plan", "--now", "1000"]).ok();
    let cast = |number: u64| {
        let frontier = planned["frontier"].as_array().unwrap();
        frontier.iter().find(|issue| issue["number"] == number).unwrap()["cast"].clone()
    };
    assert_eq!(cast(8)["model"], "claude-opus-5-5", "{planned}");
    assert_eq!(cast(9)["model"], "claude-opus-5-5", "{planned}");
}

#[test]
fn kit_research_is_written_from_a_fake_session_and_is_not_repeated() {
    let world = backlog();
    std::fs::remove_file(world.kit_path()).unwrap();
    host_to_kit(&world);
    let source = json!({"url":"https://vendor.example/card", "date":"2026-10-24"});
    std::fs::write(world.root.join("research-answer"), json!({
        "harness_order":["claude-code","pi"],
        "model_order":[{"harness":"claude-code","models":["opus","default"]},{"harness":"pi","models":["openai/gpt-5.5"]}],
        "models":[
            {"harness":"claude-code","model":"opus","good_at":"wide refactors; costly","sources":[source]},
            {"harness":"claude-code","model":"default","good_at":"small fixes","sources":[source]},
            {"harness":"pi","model":"openai/gpt-5.5","good_at":"review","sources":[source]}
        ]
    }).to_string()).unwrap();
    let written = world.afk(&["kit", "--now", "1000"]).ok();
    let models = &written["kit"]["harnesses"][0]["models"];
    assert_eq!(models[0]["model"], "opus");
    assert_eq!(models[0]["researched_at"], 1000);
    assert_eq!(models[0]["sources"], json!([source]));
    let asked = std::fs::read_to_string(world.root.join("research-asked")).unwrap();
    assert!(asked.contains("--allowedTools WebSearch WebFetch"), "{asked}");
    let again = world.afk(&["kit", "--now", "200000"]).ok();
    assert_eq!(again["kit"]["harnesses"][0]["models"][0]["model"], "opus");
    assert_eq!(again["kit"]["harnesses"][0]["models"][0]["researched_at"], 1000);
    assert_eq!(std::fs::read_to_string(world.root.join("research-asked")).unwrap(), asked);
}

#[test]
fn a_codex_casting_discovers_a_missing_entry_without_model_probes() {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "harness:codex"]);
    let before = std::fs::read(world.kit_path()).unwrap();
    let ran = world.afk(&TICK);
    assert_eq!(ran.json["errors"], json!([]), "{}", ran.stderr);
    assert_eq!(started(&world), ["agent"], "discovery appends without overriding priority");
    assert!(ran.stderr.contains("discovered codex"), "{}", ran.stderr);
    let kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    let mut before: Value = serde_json::from_slice(&before).unwrap();
    before["harnesses"][0]["models"] = json!([]);
    assert_eq!(kit["harnesses"][0], before["harnesses"][0]);
    assert_eq!(kit["harnesses"][1]["harness"], "codex");
    assert_eq!(kit["harnesses"][1]["models"], json!([]));
    assert!(!world.root.join("research-asked").exists());
    assert!(
        !world.shell_asked().iter().any(|asked| asked.contains("Reply with the single word OK.")
            || asked.contains("app-server"))
    );
}

#[test]
fn unavailable_later_casting_does_not_block_the_first_kit_harness() {
    for missing in [true, false] {
        let world = World::new();
        world.issue(7, &["ready-for-agent", "harness:codex"]);
        if missing {
            world.shell_knows("codex", None);
        } else {
            world.logged_out("codex");
        }
        let ran =
            world.afk(&["dispatch", "7", "--instance", "alpha", "--host", "h1", "--now", "1000"]);
        ran.ok();
        assert_eq!(world.claims(), [(7, "alpha".to_string())]);
        assert_eq!(started(&world), ["agent"]);
        let kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
        assert_eq!(kit["harnesses"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn a_resident_launches_reads_added_and_changed_commands() {
    use afkconvoy::git::Repo;
    use afkconvoy::launch::Launches;
    use afkconvoy_core::harness::Harness;
    const CHILD: &str = "AFKCONVOY_TEST_RESIDENT_LAUNCHES";
    if std::env::var_os(CHILD).is_some() {
        let repo = Repo::new(std::env::current_dir().unwrap());
        let launches = Launches::new(&repo, true).unwrap();
        launches.command(Harness::ClaudeCode).unwrap();
        let file = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
            .join(".config/afkconvoy/kit.json");
        let mut kit: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        kit["harnesses"]
            .as_array_mut()
            .unwrap()
            .push(json!({"harness": "codex", "command": "agent --new-codex", "models": []}));
        std::fs::write(&file, serde_json::to_vec(&kit).unwrap()).unwrap();
        assert_eq!(launches.command(Harness::Codex).unwrap().as_str(), "agent --new-codex");
        kit["harnesses"][1]["command"] = json!("agent --changed-codex");
        std::fs::write(&file, serde_json::to_vec(&kit).unwrap()).unwrap();
        assert_eq!(launches.command(Harness::Codex).unwrap().as_str(), "agent --changed-codex");
        kit["harnesses"][1]["command"] = json!("missing-custom");
        std::fs::write(&file, serde_json::to_vec(&kit).unwrap()).unwrap();
        assert!(
            launches
                .command(Harness::Codex)
                .unwrap_err()
                .to_string()
                .contains("resolves to nothing")
        );
        return;
    }
    let world = World::new();
    world.shell_knows("missing-custom", None);
    let out = world
        .command(std::env::current_exe().unwrap(), &world.clone)
        .args([
            "--exact",
            "launch::a_resident_launches_reads_added_and_changed_commands",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn every_known_harness_discovers_its_existing_stock_command() {
    for harness in afkconvoy_core::harness::Harness::ALL {
        let world = World::new();
        world.with_kit(|kit| kit["harnesses"] = json!([{"harness": harness.key()}]));
        world.issue(7, &["ready-for-agent"]);
        let ran = world.afk(&TICK);
        assert_eq!(ran.json["errors"], json!([]), "{}: {} {}", harness.key(), ran.json, ran.stderr);
        let commands = started(&world);
        assert_eq!(commands.len(), 1, "{}", harness.key());
        assert!(commands[0].ends_with(harness.stock()), "{}: {commands:?}", harness.key());
        let kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
        assert_eq!(kit["harnesses"][0]["command"], harness.stock());
    }
}

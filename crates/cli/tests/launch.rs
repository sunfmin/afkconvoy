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

const RUN: [&str; 14] = [
    "run",
    "--tracker",
    "default",
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

/// The command each worker's terminal was started with. A host watch is not
/// one: `run` writes a kit of unplaced models and its look can start that
/// watch in the same process, beside the first tick's worker.
fn started(world: &World) -> Vec<String> {
    worker_commands(&world.orca()["terminals"])
}

/// `terminals` as orca keeps them, without a host watch (`afkconvoy watch`,
/// or a command that exports `AFKCONVOY_WATCH=host`).
fn worker_commands(terminals: &Value) -> Vec<String> {
    let watch = format!("{}=host", afkconvoy_core::host_watch::WATCH_ENV);
    terminals
        .as_array()
        .unwrap()
        .iter()
        .filter(|terminal| {
            terminal["title"] != "afkconvoy watch"
                && !terminal["command"].as_str().unwrap_or("").contains(&watch)
        })
        .map(|terminal| terminal["command"].as_str().unwrap().to_string())
        .collect()
}

/// The gate counted two terminals when a fresh kit's look started a research
/// watch beside the first tick's worker. A watch is not that worker. Two
/// worker commands still are two.
#[test]
fn a_host_watch_beside_the_worker_is_not_a_second_worker() {
    let worker = "export CLAUDE_CODE_SANDBOXED=1 && claude --dangerously-skip-permissions";
    let watch = format!("export {}=host; {worker}", afkconvoy_core::host_watch::WATCH_ENV);
    let beside = json!([
        {"command": worker, "title": null},
        {"command": watch, "title": "afkconvoy watch"},
    ]);
    assert_eq!(worker_commands(&beside), [worker]);
    let untitled = json!([
        {"command": worker, "title": null},
        {"command": watch, "title": null},
    ]);
    assert_eq!(worker_commands(&untitled), [worker]);
    let two = json!([
        {"command": worker, "title": null},
        {"command": format!("{worker} --model opus"), "title": null},
    ]);
    assert_eq!(worker_commands(&two).len(), 2, "{two}");
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
        [common::trusted(afkconvoy_core::harness::Harness::ClaudeCode.stock())]
    );
    // Resolution, authentication and the trust flags are checked, with no model discovery.
    let asked = world.shell_asked();
    assert!(asked.iter().any(|line| line == "gh auth status"));
    let harness = afkconvoy_core::harness::Harness::ClaudeCode;
    let flags = harness.flags_question(harness.stock());
    assert!(
        asked.iter().all(|line| [
            "type -- 'claude'",
            "gh auth status",
            "claude auth status",
            &flags
        ]
        .contains(&line.as_str())),
        "{asked:?}"
    );
}

#[test]
fn on_stock_claude_a_command_that_was_passed_is_still_the_answer() {
    let world = backlog();
    world.kit_command("claude-code", "agent --fast");
    answer(&world, &TICK, false).ok();
    assert_eq!(started(&world), [common::trusted("agent --fast")]);
}

#[test]
fn a_command_given_for_a_harness_by_its_key_is_that_harness_s() {
    let world = backlog();
    let passed = "agent  --model=fast ";
    world.kit_command("claude-code", passed);
    answer(&world, &TICK, false).ok();
    assert_eq!(started(&world), [common::trusted(passed)], "what follows the `=`, as given");
}

#[test]
fn on_a_custom_provider_the_passed_command_is_preserved_and_worktree_trust_is_added() {
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

    assert_eq!(started(&world), [common::trusted(passed)], "the command itself is unchanged");
    let asked = world.shell_asked();
    assert!(asked.iter().any(|line| line == "type -- 'kim'"));
    let flags = afkconvoy_core::harness::Harness::ClaudeCode.flags_question(passed);
    assert!(
        asked
            .iter()
            .all(|line| ["type -- 'kim'", "gh auth status", &flags].contains(&line.as_str())),
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
    // …but for its issue, and starting it under `nice` with its own `TMPDIR`
    let dir = Path::new(create[3].strip_prefix("path:").unwrap());
    let line = format!("export AFKCONVOY_ISSUE=7 && {}", common::trusted(passed));
    let niced = common::niced(&world, dir, &line);
    assert_eq!(create[create.len() - 3..create.len() - 1], ["--command", &niced], "{create:?}");
    assert!(create.len() <= 7, "a worktree, a command, and nothing else: {create:?}");

    // …and the command is in no file: not the brief, not a note, not the repo
    // (the stand-ins' own files record what orca and the login shell were asked)
    assert_eq!(
        written_anywhere(&world.root, "big one", &["orca.json", "kit.json", "shell-asked"]),
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
    assert_eq!(
        started(&world),
        [common::trusted(afkconvoy_core::harness::Harness::ClaudeCode.stock())]
    );
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
    assert_eq!(started(&world), [common::trusted("kim")]);
}

#[test]
fn a_script_that_could_carry_the_flag_inside_is_not_warned_about() {
    let world = backlog();
    world.kit_command("claude-code", "agent");
    let ran = wrapped(&world, &TICK);
    assert_eq!(warnings(&ran), [] as [&str; 0], "{}", ran.stderr);
    ran.ok();
    assert_eq!(started(&world), [common::trusted("agent")]);
}

#[test]
fn a_custom_provider_still_uses_the_host_kit_without_a_terminal() {
    let world = backlog();
    wrapped(&world, &TICK).ok();
    assert_eq!(started(&world), [common::trusted("agent")]);
}

#[test]
fn removed_worker_command_names_the_kit() {
    let world = backlog();
    let error = world.afk(&["tick", "--instance", "alpha", "--worker-command", "agent"]).error();
    assert!(error.contains("afkconvoy kit"), "{error}");
    assert!(started(&world).is_empty());
}

/// A host with Claude Code (`default opus`), pi (`openai/gpt-5.5`) and Codex
/// installed, Codex listing no model, and DSH not installed.
fn host_to_kit(world: &World) {
    world.shell_claude_models(&["default", "opus"]);
    world.shell_models(&["openai/gpt-5.5"]);
    world.shell_knows("dsh-tui", None);
}

/// The host kit as written.
fn kit_on_disk(world: &World) -> Value {
    serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap()
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
    let kit = kit_on_disk(&world);
    assert_eq!(kit["harnesses"][0]["harness"], "claude-code");
    assert_eq!(
        kit["harnesses"][0]["models"],
        json!([{"model": "default", "good_at": "research pending"}, {"model": "opus", "good_at": "research pending"}])
    );
    let workers = started(&world);
    assert_eq!(workers.len(), 1, "its first tick started a worker: {workers:?}");
    assert!(!workers[0].contains("--model"), "the default model starts with no flag");
    assert!(
        !world.shell_asked().iter().any(|asked| asked.contains("Reply with the single word OK."))
    );
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
    let models = |world: &World| kit_on_disk(world)["harnesses"][0]["models"].clone();
    assert_eq!(
        models(&world),
        json!([{"model": "default", "good_at": "research pending"}, {"model": "opus", "good_at": "research pending"}])
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
        json!([{"model": "default", "good_at": "research pending"}, {"model": "sonnet", "good_at": "research pending"}])
    );
    let plan = world.afk(&["plan", "--now", &(1000 + day).to_string()]);
    assert_eq!(plan.json["host_kit"]["refreshed"], 1000 + day);
    // The rest of that day, no tick writes it again.
    tick_at(&world, "beta", 1000 + 2 * day - 1).wait().unwrap();
    assert_eq!(refreshes(&world).len(), 1);
    assert!(
        !world.shell_asked().iter().any(|asked| asked.contains("Reply with the single word OK."))
    );
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
    let kit = kit_on_disk(&world);
    assert_eq!(written.json["kit"]["harnesses"].as_array().unwrap().len(), 2);
    assert_eq!(
        kit,
        json!({
            "harnesses": [
                {"harness": "claude-code", "command": "agent",
                    "models": [{"model": "opus"}, {"model": "default", "good_at": "research pending"}]},
                {"harness": "pi", "command": "pi", "models": [{"model": "openai/gpt-5.5", "good_at": "research pending"}]},
            ],
        })
    );
    let left_out: Vec<(&str, &str)> = written.json["left_out"]
        .as_array()
        .unwrap()
        .iter()
        .map(|out| (out["harness"].as_str().unwrap(), out["why"].as_str().unwrap()))
        .collect();
    assert_eq!(left_out.len(), 4, "{left_out:?}");
    assert!(left_out[0].0 == "dsh" && left_out[0].1.starts_with("not installed"), "{left_out:?}");
    assert!(
        left_out[1].0 == "codex" && left_out[1].1.starts_with("listing failed"),
        "{left_out:?}"
    );
    assert!(left_out[2].0 == "kimi" && left_out[2].1.starts_with("listing failed"), "{left_out:?}");
    assert!(left_out[3].0 == "grok" && left_out[3].1.starts_with("listing failed"), "{left_out:?}");
    assert!(written.stderr.contains("left out: codex: listing failed"), "{}", written.stderr);
    // Written again with nothing changed, it is exactly the same.
    assert_eq!(world.afk(&["kit"]).json["written"], true);
    assert_eq!(kit_on_disk(&world), kit);
    assert!(
        !world.shell_asked().iter().any(|asked| asked.contains("Reply with the single word OK."))
    );
}

#[test]
fn plan_and_doctor_show_the_host_kit() {
    let world = backlog();
    world.kit_command("pi", "pi");
    let mut kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    kit["harnesses"][1]["models"] = json!([
        {"model": "a/fast", "probed": {"at": 900, "seconds": 1.5}},
        {"model": "a/slow", "probed": {"at": 901, "seconds": 9.0}},
        {"model": "a/wait", "good_at": "not researched"},
    ]);
    std::fs::write(world.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    for args in [&["plan", "--now", "1000"][..], &["doctor", "--host", "h1", "--now", "1000"][..]] {
        let answer = world.afk(args).json;
        let kit = if args[0] == "plan" { &answer["host_kit"] } else { &answer["host"]["host_kit"] };
        assert_eq!(kit["path"], world.kit_path().display().to_string());
        assert_eq!(kit["harnesses"][0]["harness"], "claude-code");
        assert_eq!(kit["harnesses"][0]["command"], "agent");
        assert_eq!(kit["harnesses"][0]["default_model"], true);
        assert_eq!(kit["harnesses"][0]["research_pending"], false);
        assert_eq!(kit["harnesses"][1]["harness"], "pi");
        assert_eq!(kit["harnesses"][1]["default_model"], false);
        assert_eq!(kit["harnesses"][1]["research_pending"], true);
        assert_eq!(kit["harnesses"][1]["models"][0]["model"], "a/fast");
        assert!(kit["harnesses"][1]["models"][1].get("probed").is_none());
    }
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
fn kit_holds_each_listed_id_with_its_display_name_and_never_probes() {
    let world = backlog();
    world.kit_command("codex", afkconvoy_core::harness::Harness::Codex.stock());
    let initialized = json!({"type": "control_response", "response": {"subtype": "success",
        "response": {"models": [{"value": "default", "displayName": "Default (recommended)"},
            {"value": "opus", "displayName": "Opus 5.5"}]}}});
    std::fs::write(world.root.join("shell-claude-models"), initialized.to_string()).unwrap();
    world.shell_models(&["openai-codex/gpt-6-sol"]);
    world.shell_server_models(&["gpt-6-sol"]);
    world.shell_knows("dsh-tui", None);
    let written = world.afk(&["kit"]);
    assert_eq!(written.json["written"], true, "{}", written.stderr);
    let kit = kit_on_disk(&world);
    let models = |harness: &str| {
        let held = kit["harnesses"].as_array().unwrap().iter().find(|h| h["harness"] == harness);
        held.map(|held| held["models"].clone()).unwrap_or_default()
    };
    assert_eq!(
        models("claude-code"),
        json!([
            {"model": "default", "names": ["Default (recommended)"], "good_at": "research pending"},
            {"model": "opus", "names": ["Opus 5.5"], "good_at": "research pending"}
        ])
    );
    assert_eq!(models("codex"), json!([{"model": "gpt-6-sol", "good_at": "research pending"}]));
    assert_eq!(
        models("pi"),
        json!([{"model": "openai-codex/gpt-6-sol", "good_at": "research pending"}])
    );
    world.issue(8, &["ready-for-agent", "model:Opus 5.5"]);
    let planned = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(planned["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(planned["host_kit"]["harnesses"][0]["default_model"], true);
    assert_eq!(planned["host_kit"]["harnesses"][0]["research_pending"], true);
    assert!(
        !world.shell_asked().iter().any(|asked| asked.contains("Reply with the single word OK."))
    );
}

#[test]
fn kit_research_is_written_from_a_checked_file_and_survives_relisting() {
    let world = backlog();
    std::fs::remove_file(world.kit_path()).unwrap();
    host_to_kit(&world);
    let source = json!({"url":"https://vendor.example/card", "date":"2026-10-24"});
    std::fs::write(world.root.join("research.json"), json!({
        "models":[
            {"harness":"claude-code","model":"opus","good_at":"wide refactors; costly","sources":[source]},
            {"harness":"claude-code","model":"default","good_at":"small fixes","sources":[source]},
            {"harness":"pi","model":"openai/gpt-5.5","good_at":"review","sources":[source]}
        ]
    }).to_string()).unwrap();
    world.afk(&["kit", "--now", "900"]).ok();
    assert!(!world.root.join("research-asked").exists(), "listing calls no model");
    let file = world.root.join("research.json");
    let written = world.afk(&["kit", "--research", file.to_str().unwrap(), "--now", "1000"]).ok();

    let models = &written["kit"]["harnesses"][0]["models"];
    assert_eq!(models[0]["model"], "opus");
    assert_eq!(models[0]["researched_at"], 1000);
    assert_eq!(models[0]["sources"], json!([source]));
    let again = world.afk(&["kit", "--now", "200000"]).ok();
    assert_eq!(again["kit"]["harnesses"][0]["models"][0]["model"], "opus");
    assert_eq!(again["kit"]["harnesses"][0]["models"][0]["researched_at"], 1000);
    assert!(!world.root.join("research-asked").exists(), "relisting calls no model");
}

#[test]
fn a_codex_casting_discovers_a_missing_entry_without_model_probes() {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "harness:codex"]);
    let before = std::fs::read(world.kit_path()).unwrap();
    let ran = world.afk(&TICK);
    assert_eq!(ran.json["errors"], json!([]), "{}", ran.stderr);
    assert_eq!(
        started(&world),
        [common::trusted("agent")],
        "discovery appends without overriding priority"
    );
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
        assert_eq!(started(&world), [common::trusted("agent")]);
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
    world.shell_help(Some(
        "--add-dir --config --model --dangerously-bypass-hook-trust --setting-sources --settings",
    ));
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
        world.with_kit(|kit| {
            kit["harnesses"] = json!([{"harness": harness.key()}]);
            if harness == afkconvoy_core::harness::Harness::Codex {
                kit["harnesses"][0]["models"] = json!([{"model":"gpt-x"}]);
            }
        });
        world.issue(7, &["ready-for-agent"]);
        let ran = world.afk(&TICK);
        assert_eq!(ran.json["errors"], json!([]), "{}: {} {}", harness.key(), ran.json, ran.stderr);
        let commands = started(&world);
        assert_eq!(commands.len(), 1, "{}", harness.key());
        if harness == afkconvoy_core::harness::Harness::Codex {
            assert_eq!(commands, [common::codex_line(&world, &world.worktrees()[0])]);
        } else {
            assert!(commands[0].contains(harness.stock()), "{}: {commands:?}", harness.key());
        }
        assert!(
            commands[0].contains(harness.worktree_trust()),
            "{} starts without worktree trust: {commands:?}",
            harness.key()
        );
        let kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
        assert_eq!(kit["harnesses"][0]["command"], harness.stock());
    }
}

#[test]
fn an_operator_removal_survives_relisting_daily_refresh_and_fallback() {
    let world = backlog();
    world.kit_command("pi", "pi");
    host_to_kit(&world);
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "899"]).ok();
    let removed = world.afk(&["kit", "--remove", "pi", "--now", "900"]).ok();
    assert_eq!(removed["removed"], "pi");
    assert_eq!(removed["kit"]["removed"], json!(["pi"]));
    assert_eq!(removed["kit"]["harnesses"].as_array().unwrap().len(), 1);
    assert_eq!(removed["left_out"], json!([{"harness":"pi", "why":"removed by the operator"}]));

    for args in [&["plan", "--now", "901"][..], &["doctor", "--host", "h1", "--now", "901"][..]] {
        let answer = world.afk(args).json;
        let kit = if args[0] == "plan" { &answer["host_kit"] } else { &answer["host"]["host_kit"] };
        assert_eq!(
            kit["left_out"],
            json!([{"harness":"pi", "why":"removed by the operator"}]),
            "{args:?}: {answer}"
        );
    }
    let relisted = world.afk(&["kit", "--now", "1000"]).ok();
    assert_eq!(relisted["kit"]["harnesses"].as_array().unwrap().len(), 1);
    assert!(
        relisted["left_out"]
            .as_array()
            .unwrap()
            .iter()
            .any(|out| out["harness"] == "pi" && out["why"] == "removed by the operator")
    );

    let now = (1000 + 86_400).to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok();
    let refreshed = kit_on_disk(&world);
    assert_eq!(refreshed["removed"], json!(["pi"]));
    assert_eq!(refreshed["harnesses"].as_array().unwrap().len(), 1);

    // Claude Code is paused: the next worker falls back to Codex, never to
    // the still-installed pi whose entry launch discovery must not recreate.
    world.kit_command("codex", "codex --yolo");
    world.pause("claude-code", 87_399, "test pause");
    world.issue(8, &["ready-for-agent"]);
    world.list_checkout(&world.clone);
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "87401"]).ok();
    let commands = started(&world);
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert!(commands[1].contains("codex"), "{commands:?}");
    assert!(!commands[1].contains("pi --"), "{commands:?}");
    assert_eq!(kit_on_disk(&world)["removed"], json!(["pi"]));
}

#[test]
fn removing_a_harness_with_a_running_worker_is_refused_without_ending_it() {
    let world = backlog();
    world.kit_command("pi", "pi");
    world.afk(&TICK).ok();
    let before = std::fs::read(world.kit_path()).unwrap();
    let error = world.afk(&["kit", "--remove", "claude-code"]).error();
    assert!(error.contains("issue #7"), "{error}");
    assert!(error.contains("afkconvoy switch --all --harness pi"), "{error}");
    assert!(error.contains("no worker was ended"), "{error}");
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), before);
    assert_eq!(started(&world).len(), 1);
    assert_eq!(world.claims().len(), 1);
}

#[test]
fn a_hand_edited_absence_is_read_as_is_but_relisting_brings_it_back() {
    let world = backlog();
    world.kit_command("pi", "pi");
    world.with_kit(|kit| {
        kit["harnesses"].as_array_mut().unwrap().retain(|held| held["harness"] != "pi")
    });
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["host_kit"]["harnesses"].as_array().unwrap().len(), 1);
    assert_eq!(plan["host_kit"]["left_out"], json!([]));

    host_to_kit(&world);
    let relisted = world.afk(&["kit", "--now", "1001"]).ok();
    assert!(
        relisted["kit"]["harnesses"].as_array().unwrap().iter().any(|held| held["harness"] == "pi")
    );
    assert!(relisted["kit"].get("removed").is_none());
}

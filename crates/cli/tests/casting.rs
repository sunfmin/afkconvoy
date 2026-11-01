//! A worker's model is its one `model:` label of the harness already selected,
//! or that harness's user default. Automatic casting is gone.

use crate::common;

use afkconvoy_core::harness::Harness;
use common::World;
use serde_json::{Value, json};

const TICK: [&str; 7] = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1000"];
const PLAN: [&str; 3] = ["plan", "--now", "1000"];

/// The command each worker's terminal was started with.
fn started(world: &World) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    terminals.iter().map(|t| t["command"].as_str().unwrap().to_string()).collect()
}

/// An unbound repo whose issue 7 carries `labels` beside the ready label, on
/// a host kit that holds Claude Code on `opus` then `haiku`, pi on `a/fast`
/// then `a/slow`, and Codex and DSH on no model.
fn cast(labels: &[&str]) -> World {
    let world = World::new();
    world.hold_every_harness();
    world.issue(7, &[&["ready-for-agent"], labels].concat());
    let mut kit: Value = serde_json::from_slice(&std::fs::read(world.kit_path()).unwrap()).unwrap();
    let models = |names: &[&str]| -> Value {
        names
            .iter()
            .map(|name| json!({"model": name, "probed": {"at": 900, "seconds": 1.0}}))
            .collect()
    };
    for held in kit["harnesses"].as_array_mut().unwrap() {
        held["models"] = match held["harness"].as_str().unwrap() {
            "claude-code" => models(&["opus", "haiku"]),
            "pi" => models(&["a/fast", "a/slow"]),
            _ => json!([]),
        };
    }
    std::fs::write(world.kit_path(), serde_json::to_vec(&kit).unwrap()).unwrap();
    world
}

fn followed(harness: &str, model: &str) -> Value {
    json!({"harness": harness, "model": model, "why": "model_label"})
}

fn user_default(harness: &str) -> Value {
    json!({"harness": harness, "model": null, "why": "user_default"})
}

/// Ticks as `alpha` at `now`.
fn tick_at(world: &World, now: u64) -> Value {
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok()
}

fn claude(model: Option<&str>) -> String {
    match model {
        Some(model) => common::trusted(&Harness::ClaudeCode.with_model("agent", model)),
        None => common::trusted("agent"),
    }
}

fn pi(model: Option<&str>) -> String {
    let stock = Harness::Pi.stock();
    let command = match model {
        Some(model) => Harness::Pi.with_model(stock, model),
        None => stock.to_string(),
    };
    Harness::Pi.in_worktree(&command, "", "")
}

#[test]
fn a_model_label_of_the_selected_harness_is_followed_and_casting_is_gone() {
    let world = cast(&["model:haiku"]);
    world.issue(8, &["ready-for-agent"]);
    let planned = world.afk(&PLAN).ok();
    assert!(planned["host_kit"].get("cast").is_none());
    assert!(planned.get("caster").is_none());
    assert_eq!(planned["frontier"][0]["cast"], followed("claude-code", "haiku"));
    assert_eq!(planned["casting"][0]["stages"][0]["followed"], true);
    assert_eq!(planned["casting"][0]["needed"], false);
    assert_eq!(planned["casting"][1]["why"], "the user's default");
    assert!(world.afk(&["cast"]).error().contains("automatic casting is gone"));
    assert!(world.afk(&["kit", "--cast", "on"]).error().contains("automatic casting is gone"));
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [claude(Some("haiku")), claude(None)]);
    let doctor = world.afk(&["doctor", "--now", "1000"]).json;
    assert!(doctor["host"]["host_kit"].get("cast").is_none());
    assert_eq!(doctor["issues"]["casting"][0]["running"]["model"], "haiku");
    let panel = world.render_panel("followed", &["--now", "1000"], (1440, 1000)).html;
    assert!(panel.contains("running claude-code on haiku"), "{panel}");
    assert!(!panel.contains("A caster") && !panel.contains("Automatic model casting"), "{panel}");
}

#[test]
fn a_switch_reorders_the_kit_and_the_label_still_decides_the_model() {
    let world = cast(&["model:haiku"]);
    world.kit_models("pi", &[]);
    world.afk(&["switch", "--harness", "pi"]).ok();
    let planned = world.afk(&PLAN).ok();
    assert!(planned.get("caster").is_none());
    assert_eq!(planned["frontier"][0]["cast"]["harness"], "pi");
    assert_eq!(planned["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(planned["frontier"][0]["cast"]["why"], "user_default");
    assert_eq!(planned["frontier"][0]["cast"]["unmatched"], "`model:haiku`");
    world.afk(&TICK).ok();
    let commands = started(&world);
    assert_eq!(commands.len(), 1);
    assert!(commands[0].contains(&pi(None)) && commands[0].contains("AFKCONVOY_GUARD="));
    assert!(!commands[0].contains("--model"));
    assert_eq!(world.labels(7), ["ready-for-agent", "model:haiku"]);
    assert!(world.comments(7).iter().any(|c| c.contains("The label stays.")));

    world.afk(&["switch", "--harness", "claude-code"]).ok();
    world.issue(8, &["ready-for-agent", "model:haiku"]);
    let planned = world.afk(&PLAN).ok();
    assert!(planned["casting"].as_array().unwrap().iter().all(|s| s["needed"] == false));
    let eight = planned["frontier"].as_array().unwrap().iter().find(|i| i["number"] == 8).unwrap();
    assert_eq!(eight["cast"], followed("claude-code", "haiku"));
}

#[test]
fn a_paused_harness_falls_through_and_the_label_stays() {
    let world = cast(&["model:haiku"]);
    world.pause("claude-code", 900, "refused");
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"][0]["cast"]["harness"], "pi");
    assert_eq!(planned["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(planned["frontier"][0]["cast"]["why"], "user_default");
    world.afk(&TICK).ok();
    let commands = started(&world);
    assert_eq!(commands.len(), 1);
    assert!(commands[0].contains(&pi(None)) && commands[0].contains("AFKCONVOY_GUARD="));
    assert_eq!(world.labels(7), ["ready-for-agent", "model:haiku"]);
    std::fs::remove_file(world.paused_path()).unwrap();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["casting"][0]["stages"][0]["model"], "haiku");
    assert!(planned["casting"][0]["running"]["model"].is_null());
}

#[test]
fn no_label_runs_the_user_default_even_when_the_kit_holds_one_model() {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    world.with_kit(|kit| kit["harnesses"][0]["models"] = json!([{"model": "opus"}]));
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"][0]["cast"], user_default("claude-code"));
    assert!(planned.get("caster").is_none());
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [claude(None)]);
    assert!(!started(&world)[0].contains("--model"));
}

#[test]
fn a_model_label_is_followed_only_on_the_selected_harness() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"][0]["cast"]["harness"], "claude-code");
    assert_eq!(planned["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(planned["frontier"][0]["cast"]["why"], "user_default");
    assert_eq!(planned["frontier"][0]["cast"]["unmatched"], "`model:a/slow`");
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [claude(None)]);
    let told: Vec<_> = world
        .comments(7)
        .into_iter()
        .filter(|c| c.starts_with("<!--convoy:unmatched-->"))
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("`model:a/slow`"));
    assert_eq!(world.labels(7), ["ready-for-agent", "harness:pi", "model:a/slow"]);

    let world = cast(&["harness:claude-code", "model:haiku"]);
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [claude(Some("haiku"))]);
    assert!(
        !world.comments(7).iter().any(|c| c.starts_with("<!--convoy:unmatched-->")),
        "{:?}",
        world.comments(7)
    );
}

#[test]
fn an_unfollowed_label_is_said_once_and_a_harness_label_is_not() {
    let world = cast(&["model:not-held"]);
    world.afk(&TICK).ok();
    tick_at(&world, 1100);
    let told: Vec<_> = world
        .comments(7)
        .into_iter()
        .filter(|c| c.starts_with("<!--convoy:unmatched-->"))
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("`model:not-held`") && told[0].contains("The label stays."));
    assert_eq!(world.labels(7), ["ready-for-agent", "model:not-held"]);
    assert_eq!(started(&world), [claude(None)]);

    let world = cast(&["model:opus", "model:haiku"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(planned["frontier"][0]["cast"]["unmatched"], "`model:opus` and `model:haiku`");
    world.afk(&TICK).ok();
    let told: Vec<_> = world
        .comments(7)
        .into_iter()
        .filter(|c| c.starts_with("<!--convoy:unmatched-->"))
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("two model labels"), "{told:?}");
    assert_eq!(started(&world), [claude(None)]);

    for labels in [&["harness:nope"][..], &["harness:pi"], &["harness:codex"]] {
        let world = cast(labels);
        world.afk(&TICK).ok();
        assert_eq!(started(&world), [claude(None)], "{labels:?}");
        assert!(
            !world.comments(7).iter().any(|c| c.starts_with("<!--convoy:unmatched-->")),
            "{labels:?}: {}",
            world.comments(7).join("\n")
        );
        assert!(world.labels(7).iter().any(|label| label == labels[0]));
    }
}

#[test]
fn a_switch_to_codex_runs_codex_s_user_default() {
    let world = cast(&["harness:pi", "model:a/none"]);
    world.kit_models("codex", &["gpt-x"]);
    let switch = ["switch", "--instance", "alpha", "--host", "h1", "--now", "900"];
    world.afk(&[&switch[..], &["--all", "--harness", "codex"]].concat()).ok();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"][0]["cast"]["harness"], "codex");
    assert_eq!(planned["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(planned["frontier"][0]["cast"]["why"], "user_default");
    world.afk(&TICK).ok();
    let commands = started(&world);
    assert_eq!(commands.len(), 1);
    assert!(commands[0].contains(Harness::Codex.stock()), "{}", commands[0]);
    assert!(!commands[0].contains("--model"), "{}", commands[0]);
}

#[test]
fn a_tick_starts_no_caster_and_dispatches_at_once() {
    let world = cast(&[]);
    world.issue(8, &["ready-for-agent"]);
    world.issue(9, &["ready-for-agent", "harness:codex"]);
    let ticked = tick_at(&world, 1000);
    let did = ticked["did"].to_string();
    assert!(!did.contains("\"cast\""), "{ticked}");
    assert_eq!(started(&world).len(), 3, "{ticked}");
    assert!(started(&world).iter().all(|command| !command.contains(" cast; exit")));
    let planned = world.afk(&["plan", "--now", "1100"]).ok();
    assert!(planned.get("caster").is_none(), "{planned}");
    for excluded in planned["excluded"].as_array().unwrap() {
        let reason = excluded["reason"].as_str().unwrap();
        assert!(!reason.contains("caster"), "{excluded}");
    }
    let panel = world.render_panel("no-caster", &["--now", "1100"], (1440, 1000)).html;
    assert!(!panel.contains("caster"), "{panel}");
}

#[test]
fn switching_again_follows_the_label_of_the_harness_now_first() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.kit_models("codex", &["gpt-x"]);
    world.afk(&["switch", "--harness", "codex"]).ok();
    world.afk(&TICK).ok();
    let commands = started(&world);
    assert_eq!(commands.len(), 1);
    assert!(!commands[0].contains("--model"), "{}", commands[0]);
    let moved =
        world.afk(&["switch", "7", "--instance", "alpha", "--host", "h1", "--harness", "pi"]).ok();
    assert_eq!(moved["switched"][0]["to"], "pi");
    let commands = started(&world);
    let command = commands.last().unwrap();
    assert!(command.contains(&pi(Some("a/slow"))) && command.contains("AFKCONVOY_GUARD="));
    assert!(world.afk(&["switch", "--clear"]).error().contains("--clear"));
    world.issue(8, &["ready-for-agent", "harness:claude-code", "model:haiku"]);
    let plan = world.afk(&PLAN).ok();
    let eight = plan["frontier"].as_array().unwrap().iter().find(|i| i["number"] == 8).unwrap();
    assert_eq!(eight["cast"]["harness"], "pi");
    assert_eq!(eight["cast"]["model"], Value::Null);
    assert_eq!(eight["cast"]["why"], "user_default");
    assert_eq!(eight["cast"]["unmatched"], "`model:haiku`");
}

#[test]
fn a_paused_candidate_is_skipped_without_reordering_the_kit_or_rewriting_labels() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    let kit = std::fs::read(world.kit_path()).unwrap();
    world.pause("pi", 400, "organization disabled subscription access");
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"][0]["cast"]["harness"], "claude-code");
    assert_eq!(planned["frontier"][0]["cast"]["model"], Value::Null);
    assert_eq!(planned["host_kit"]["paused"][0]["harness"], "pi");
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [claude(None)]);
    assert_eq!(world.labels(7), ["ready-for-agent", "harness:pi", "model:a/slow"]);
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), kit);
}

#[test]
fn a_host_kit_every_harness_of_which_is_paused_starts_no_worker_and_plan_says_so() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.pause("claude-code", 400, "refused");
    let planned = world.afk(&PLAN).ok();
    let reason = planned["excluded"][0]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("held while every harness of the host kit is paused: `claude-code`"),
        "{reason}"
    );
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [] as [String; 0]);
}

#[test]
fn a_pause_does_not_rewrite_labels() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.issue(8, &["ready-for-agent", "harness:pi"]);
    world.pause("pi", 400, "refused");
    tick_at(&world, 1000);
    assert_eq!(world.labels(7), ["ready-for-agent", "harness:pi", "model:a/slow"]);
    assert_eq!(world.labels(8), ["ready-for-agent", "harness:pi"]);
    assert_eq!(started(&world), vec![claude(None); 2]);
    assert!(world.comments(7).iter().any(|c| c.starts_with("<!--convoy:unmatched-->")));
    assert!(!world.comments(8).iter().any(|c| c.starts_with("<!--convoy:unmatched-->")));
}

#[test]
fn each_paused_harness_is_probed_every_hour_and_one_that_answers_is_back() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.pause("pi", 0, "refused");
    tick_at(&world, 3599);
    assert_eq!(world.paused()[0]["probed"], Value::Null, "not probed within the hour");
    tick_at(&world, 3600);
    assert_eq!(world.paused()[0]["probed"], 3600, "probed, and silent");
    let probes: Vec<_> =
        world.shell_asked().into_iter().filter(|asked| asked.contains("single word OK")).collect();
    assert_eq!(probes.len(), 1, "{probes:?}");
    assert!(world.questions().is_empty(), "a recovery probe opens no terminal");
    assert_eq!(started(&world), [claude(None)]);

    world.probes_answer();
    tick_at(&world, 7199);
    assert_eq!(world.paused().as_array().unwrap().len(), 1, "an hour after its last probe");
    tick_at(&world, 7200);
    assert_eq!(world.paused(), json!([]), "it answered");
    assert_eq!(started(&world), [claude(None)]);
}

#[test]
fn a_human_label_that_matches_nothing_stays_and_the_worker_runs_the_user_default() {
    let world = cast(&["harness:claude-code", "model:sonnet"]);
    world.issue(8, &["ready-for-agent", "harness:claude-code", "model:opuss"]);
    let planned = world.afk(&PLAN).ok();
    assert!(planned.get("caster").is_none());
    let eight = planned["frontier"].as_array().unwrap().iter().find(|e| e["number"] == 8).unwrap();
    assert_eq!(eight["cast"]["why"], "user_default");
    assert_eq!(eight["cast"]["model"], Value::Null);
    assert_eq!(eight["cast"]["unmatched"], "`model:opuss`");
    assert!(world.afk(&["cast", "--now", "1100"]).error().contains("automatic casting is gone"));
    assert_eq!(world.labels(7), ["ready-for-agent", "harness:claude-code", "model:sonnet"]);
    assert_eq!(world.labels(8), ["ready-for-agent", "harness:claude-code", "model:opuss"]);
}

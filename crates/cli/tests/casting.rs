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

#[test]
fn casting_defaults_off_and_manual_model_labels_still_select_within_the_harness() {
    let world = cast(&["model:haiku"]);
    world.issue(8, &["ready-for-agent"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["host_kit"]["cast"], false);
    assert_eq!(planned["frontier"][0]["cast"]["model"], "haiku");
    assert_eq!(planned["casting"][0]["stages"][0]["followed"], true);
    assert!(planned["casting"][1]["why"].as_str().unwrap().contains("off on this host"));
    let answer = world.afk(&["cast"]).ok();
    assert!(answer["said"].as_str().unwrap().contains("off on this host"));
    assert!(world.caster_asked().is_empty());
    world.afk(&TICK).ok();
    assert_eq!(casters(&world), 0);
    assert_eq!(started(&world), ["agent --model 'haiku'", "agent --model 'opus'"]);
    let doctor = world.afk(&["doctor", "--now", "1000"]).json;
    assert_eq!(doctor["host"]["host_kit"]["cast"], false);
    assert_eq!(doctor["issues"]["casting"][0]["running"]["model"], "haiku");
    let panel = world.render_panel("cast-off", &["--now", "1000"], (1440, 1000)).html;
    assert!(
        panel.contains("Automatic model casting is off")
            && panel.contains("running claude-code on haiku"),
        "{panel}"
    );
}

#[test]
fn enabled_casting_has_no_choice_on_a_model_less_harness_and_reuses_labels_after_switching_back() {
    let world = cast(&["model:haiku"]);
    world.comment(7, "Cast `model:haiku`: Small.");
    world.afk(&["kit", "--cast", "on"]).ok();
    world.afk(&["switch", "--harness", "codex"]).ok();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["caster"], Value::Null);
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness":"codex","model":null,"why":"first"})
    );
    assert!(planned["casting"][0]["why"].as_str().unwrap().contains("no model choice"));
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);
    world.afk(&["switch", "--harness", "claude-code"]).ok();
    world.issue(8, &["ready-for-agent", "model:haiku"]);
    world.comment(8, "Cast `model:haiku`: Small.");
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["caster"], Value::Null);
    assert!(planned["casting"].as_array().unwrap().iter().all(|s| s["needed"] == false));
    assert_eq!(world.labels(7), ["ready-for-agent", "model:haiku"]);
}

#[test]
fn fallback_adds_a_model_for_the_new_harness_without_removing_the_previous_casting() {
    let world = cast(&["model:haiku"]);
    world.comment(7, "Cast `model:haiku`: Small.");
    world.afk(&["kit", "--cast", "on"]).ok();
    world.pause("claude-code", 900, "refused");
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["caster"]["to_cast"], json!([7]));
    world.caster_answers(
        "{\"issue\":7,\"harness\":\"pi\",\"model\":\"a/slow\",\"why\":\"Wide.\"}\n",
    );
    world.afk(&["cast", "--now", "1000"]).ok();
    assert_eq!(world.labels(7), ["ready-for-agent", "model:haiku", "model:a/slow"]);
    let asked = world.caster_asked();
    assert!(!asked.contains("- claude-code ") && asked.contains("- pi a/fast\n"), "{asked}");
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Pi.with_model(Harness::Pi.stock(), "a/slow")]);
    std::fs::remove_file(world.paused_path()).unwrap();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["caster"], Value::Null);
    assert_eq!(planned["casting"][0]["stages"][0]["model"], "haiku");
    assert_eq!(planned["casting"][0]["running"]["model"], "a/slow");
}

#[test]
fn casting_selects_a_model_only_within_the_first_eligible_kit_harness() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness":"claude-code", "model":"opus", "why":"first"})
    );
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::ClaudeCode.with_model("agent", "opus")]);
    let world = cast(&["harness:claude-code", "model:haiku"]);
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::ClaudeCode.with_model("agent", "haiku")]);
}

#[test]
fn an_issue_with_no_casting_and_no_choice_to_make_runs_the_host_kit_s_first() {
    // The host kit holds one harness on one model: there is nothing to cast.
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    world.with_kit(|kit| kit["harnesses"][0]["models"] = json!([{"model": "opus"}]));
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "claude-code", "model": "opus", "why": "first"})
    );
    assert_eq!(planned["caster"], Value::Null);
    world.afk(&TICK).ok();
    assert_eq!(started(&world), ["agent --model 'opus'"]);
}

#[test]
fn unheld_casting_preserves_labels_and_runs_the_kit_first_without_carrying_models() {
    for labels in [
        &["harness:pi", "model:a/none"][..],
        &["harness:codex", "model:gpt-x"],
        &["harness:nope"],
        &["model:not-held"],
    ] {
        let world = cast(labels);
        let planned = world.afk(&PLAN).ok();
        assert_eq!(
            planned["frontier"][0]["cast"],
            json!({"harness":"claude-code", "model":"opus", "why":"first"})
        );
        world.afk(&TICK).ok();
        assert_eq!(started(&world), [Harness::ClaudeCode.with_model("agent", "opus")]);
        assert!(world.labels(7).iter().any(|label| label == labels[0]));
        assert!(!world.comments(7).iter().any(|c| c.starts_with("<!--convoy:unmatched-->")));
    }
}

#[test]
fn a_switch_wins_over_the_casting() {
    let world = cast(&["harness:pi", "model:a/none"]);
    let switch = ["switch", "--instance", "alpha", "--host", "h1", "--now", "900"];
    world.afk(&[&switch[..], &["--all", "--harness", "codex"]].concat()).ok();
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "codex", "model": null, "why": "first"})
    );
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);
}

/// Ticks as `alpha` at `now`.
fn tick_at(world: &World, now: u64) -> Value {
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok()
}

/// The terminals started on a caster: each runs `afkconvoy cast`.
fn casters(world: &World) -> usize {
    started(world).iter().filter(|command| command.contains(" cast; exit")).count()
}

#[test]
fn a_tick_starts_one_caster_and_holds_the_issues_it_casts_back_for_ten_minutes_at_most() {
    let world = cast(&[]);
    world.afk(&["kit", "--cast", "on"]).ok();
    world.issue(8, &["ready-for-agent"]);
    // A human's casting is never the caster's: 9 is dispatched as it says.
    world.issue(9, &["ready-for-agent", "harness:codex"]);
    let ticked = tick_at(&world, 1000);
    assert_eq!(casters(&world), 1, "{ticked}");
    assert_eq!(started(&world).len(), 2, "the caster and the worker on 9: {ticked}");
    assert!(started(&world).contains(&Harness::ClaudeCode.with_model("agent", "opus")));
    assert_eq!(ticked["did"][0], json!({"do": "cast", "issue": null, "outcome": "done"}));

    let planned = world.afk(&["plan", "--now", "1100"]).ok();
    assert_eq!(
        planned["caster"],
        json!({"running_since": 1000, "due": false, "held": [7, 8], "to_cast": [7, 8]})
    );
    for excluded in &planned["excluded"].as_array().unwrap()[..2] {
        let reason = excluded["reason"].as_str().unwrap();
        assert!(reason.starts_with("held for the caster"), "{excluded}");
    }
    let panel = world.render_panel("caster-runs", &["--now", "1100"], (1440, 1000)).html;
    assert!(panel.contains("<b>A caster runs</b>"), "{panel}");
    assert!(panel.contains("#7, #8 held for it"), "{panel}");

    // No second caster while it runs, and nothing it is to cast is dispatched.
    tick_at(&world, 1300);
    assert_eq!((casters(&world), started(&world).len()), (1, 2));

    // Past ten minutes they are dispatched uncast, on the host kit's first.
    let planned = world.afk(&["plan", "--now", "1600"]).ok();
    assert_eq!(planned["caster"], Value::Null);
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "claude-code", "model": "opus", "why": "uncast"})
    );
    let panel = world.render_panel("caster-late", &["--now", "1600"], (1440, 1000)).html;
    assert!(panel.contains("the caster did not cast it in ten minutes"), "{panel}");
    tick_at(&world, 1600);
    let opus = started(&world).iter().filter(|c| *c == "agent --model 'opus'").count();
    assert_eq!((casters(&world), opus), (1, 3));
}

#[test]
fn an_issue_with_no_choice_to_make_or_a_casting_starts_no_caster() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick_at(&world, 1000);
    assert_eq!((casters(&world), started(&world)), (0, vec!["agent".to_string()]));

    let world = cast(&["harness:pi"]);
    tick_at(&world, 1000);
    assert_eq!(casters(&world), 0);
}

#[test]
fn cast_labels_each_issue_only_with_a_model_of_its_selected_harness() {
    let world = cast(&[]);
    world.afk(&["kit", "--cast", "on"]).ok();
    world.issue(8, &["ready-for-agent"]);
    world.issue(9, &["ready-for-agent", "harness:pi"]);
    world.issue(10, &["ready-for-agent", "epic"]);
    world.issue(11, &["ready-for-agent"]);
    world.with_kit(|kit| kit["harnesses"][0]["models"][1]["good_at"] = json!("quick, small fixes"));
    world.caster_answers(concat!(
        "Here you are:\n",
        "{\"issue\":7,\"harness\":\"claude-code\",\"model\":\"haiku\",\"why\":\"A one-line fix.\"}\n",
        "{\"issue\":8,\"harness\":\"pi\",\"model\":\"a/fast\",\"why\":\"Wrong harness.\"}\n",
        "{\"issue\":9,\"harness\":\"claude-code\",\"model\":\"opus\",\"why\":\"Not asked.\"}\n",
        "{\"issue\":11,\"harness\":\"claude-code\",\"model\":\"none\",\"why\":\"Out of range.\"}\n",
    ));
    let answered = world.afk(&["cast", "--now", "1000"]).ok();
    assert_eq!(
        answered["cast"],
        json!([{"issue":7,"harness":"claude-code","model":"haiku","why":"A one-line fix."}])
    );
    assert_eq!(answered["uncast"].as_array().unwrap().len(), 2, "{answered}");
    let asked = world.caster_asked();
    for given in [
        "## #7 issue 7",
        "## #8 issue 8",
        "## #11 issue 11",
        "- claude-code haiku: quick, small fixes\n",
    ] {
        assert!(asked.contains(given), "{given}\n{asked}");
    }
    assert!(!asked.contains("## #9") && !asked.contains("## #10"));
    assert!(!asked.contains("- pi ") && !asked.contains("- codex"), "{asked}");
    assert_eq!(world.labels(7), ["ready-for-agent", "model:haiku"]);
    assert_eq!(world.labels(8), ["ready-for-agent"]);
    assert_eq!(world.labels(9), ["ready-for-agent", "harness:pi"]);
    assert_eq!(world.comments(7)[0], "Cast `model:haiku`: A one-line fix.");
    world.afk(&TICK).ok();
    assert!(started(&world).contains(&"agent --model 'haiku'".to_string()));
}

#[test]
fn switching_again_changes_the_kit_without_a_separate_clearable_choice() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.afk(&["switch", "--harness", "codex"]).ok();
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);
    let moved =
        world.afk(&["switch", "7", "--instance", "alpha", "--host", "h1", "--harness", "pi"]).ok();
    assert_eq!(moved["switched"][0]["to"], "pi");
    assert_eq!(
        started(&world).last(),
        Some(&Harness::Pi.with_model(Harness::Pi.stock(), "a/slow"))
    );
    assert!(world.afk(&["switch", "--clear"]).error().contains("--clear"));
    world.issue(8, &["ready-for-agent", "harness:claude-code", "model:haiku"]);
    let plan = world.afk(&PLAN).ok();
    let eight = plan["frontier"].as_array().unwrap().iter().find(|i| i["number"] == 8).unwrap();
    assert_eq!(eight["cast"], json!({"harness":"pi", "model":"a/fast", "why":"first"}));
}

#[test]
fn a_paused_candidate_is_skipped_without_holding_its_ticket_or_reordering_the_kit() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    let kit = std::fs::read(world.kit_path()).unwrap();
    world.pause("pi", 400, "organization disabled subscription access");
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["frontier"][0]["cast"]["harness"], "claude-code");
    assert_eq!(planned["host_kit"]["paused"][0]["harness"], "pi");
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::ClaudeCode.with_model("agent", "opus")]);
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
fn a_temporary_pause_never_rewrites_a_casters_or_a_humans_ticket_labels() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.comment(7, "Cast `harness:pi` and `model:a/slow`: A small fix.");
    world.issue(8, &["ready-for-agent", "harness:pi"]);
    world.pause("pi", 400, "refused");
    tick_at(&world, 1000);
    assert_eq!(casters(&world), 0);
    assert_eq!(world.labels(7), ["ready-for-agent", "harness:pi", "model:a/slow"]);
    assert_eq!(world.labels(8), ["ready-for-agent", "harness:pi"]);
    assert_eq!(started(&world), vec![Harness::ClaudeCode.with_model("agent", "opus"); 2]);
}

#[test]
fn each_paused_harness_is_probed_every_hour_and_one_that_answers_is_back() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    world.pause("pi", 0, "refused");
    tick_at(&world, 3599);
    assert_eq!(world.paused()[0]["probed"], Value::Null, "not probed within the hour");
    tick_at(&world, 3600);
    assert_eq!(world.paused()[0]["probed"], 3600, "probed, and silent");
    // In an orca terminal of its own, closed once it was silent.
    let questions = world.questions();
    let probes: Vec<_> =
        questions.iter().filter(|(asked, _)| asked.contains("single word OK")).collect();
    assert_eq!(probes.len(), 1, "{questions:?}");
    assert!(!probes[0].1, "its terminal is closed");
    assert_eq!(started(&world), [Harness::ClaudeCode.with_model("agent", "opus")]);

    world.probes_answer();
    tick_at(&world, 7199);
    assert_eq!(world.paused().as_array().unwrap().len(), 1, "an hour after its last probe");
    tick_at(&world, 7200);
    assert_eq!(world.paused(), json!([]), "it answered");
    assert_eq!(started(&world), [Harness::ClaudeCode.with_model("agent", "opus")]);
}

#[test]
fn a_caster_s_casting_naming_no_held_model_is_cast_again_with_ids_and_a_human_s_stays() {
    let world = cast(&["harness:claude-code", "model:sonnet"]);
    world.comment(7, "Cast `harness:claude-code` and `model:sonnet`: A small fix.");
    world.afk(&["kit", "--cast", "on"]).ok();
    // A human's label naming nothing stays and runs the default.
    world.issue(8, &["ready-for-agent", "harness:claude-code", "model:opuss"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(planned["caster"]["to_cast"], json!([7]), "{planned}");
    let eight = planned["frontier"].as_array().unwrap().iter().find(|e| e["number"] == 8).unwrap();
    assert_eq!(eight["cast"]["why"], "first");
    assert_eq!(eight["cast"]["unmatched"], Value::Null);

    // The caster answers with a name; the label names the model's id.
    world.with_kit(|kit| {
        kit["harnesses"][0]["models"][0] = json!({"model": "claude-opus-5-5", "names": ["opus"]});
    });
    world.caster_answers(
        "{\"issue\": 7, \"harness\": \"claude-code\", \"model\": \"OPUS\", \"why\": \"Hard.\"}\n",
    );
    let answered = world.afk(&["cast", "--now", "1100"]).ok();
    assert_eq!(answered["cast"][0]["model"], "claude-opus-5-5", "{answered}");
    assert_eq!(
        world.labels(7),
        ["ready-for-agent", "harness:claude-code", "model:sonnet", "model:claude-opus-5-5"]
    );
    let said = world.comments(7).last().unwrap().clone();
    assert!(said.starts_with("Cast `model:claude-opus-5-5`: Hard."));
    assert_eq!(world.labels(8), ["ready-for-agent", "harness:claude-code", "model:opuss"]);
}

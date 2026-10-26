//! Checked research through the real command: placements, atomic refusals,
//! partial evidence and the resulting worker selection.
use serde_json::{Value, json};

use crate::common::World;

fn pending() -> World {
    let world = World::new();
    world.kit_models("claude-code", &["fast", "deep", "unknown"]);
    world.with_kit(|kit| {
        for model in kit["harnesses"][0]["models"].as_array_mut().unwrap() {
            model["good_at"] = json!("research pending");
        }
    });
    world
}

fn finding(model: &str) -> Value {
    json!({"harness":"claude-code", "model":model, "good_at":"wide refactors; costly on small fixes",
        "names":[format!("{model} display")],
        "sources":[{"url":"https://vendor.example/card", "date":"2026-10-24"}]})
}

fn apply(world: &World, research: &Value) -> crate::common::Answer {
    let file = world.root.join("research.json");
    std::fs::write(&file, research.to_string()).unwrap();
    world.afk_in(&world.root, &["kit", "--research", file.to_str().unwrap(), "--now", "1000"])
}

#[test]
fn sourced_research_places_models_in_file_order_without_calling_a_model() {
    let world = pending();
    let before = world.orca_called_with();
    let answer =
        apply(&world, &json!({"models":[finding("deep"), finding("fast"), finding("unknown")]}))
            .ok();
    assert_eq!(answer["written"], true);
    assert_eq!(answer["placed"].as_array().unwrap().len(), 3);
    let models = &answer["kit"]["harnesses"][0]["models"];
    assert_eq!(models[0]["model"], "deep");
    assert_eq!(models[0]["researched_at"], 1000);
    assert_eq!(models[0]["names"], json!(["deep display"]));
    assert_eq!(models[0]["sources"], finding("deep")["sources"]);
    assert_eq!(world.orca_called_with(), before, "applying a file needs no orca or model");
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["host_kit"]["harnesses"][0]["default_model"], false);
    assert!(
        !doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["what"].as_str().unwrap().contains("unplaced models"))
    );
    world.issue(7, &["ready-for-agent", "model:deep display"]);
    let planned = world.afk(&["plan"]);
    assert_eq!(planned.code, 0, "{}; orca: {}", planned.json, world.orca());
    let plan = planned.ok();
    assert_eq!(plan["frontier"][0]["cast"]["model"], "deep");
}

#[test]
fn legacy_not_researched_models_named_by_doctor_accept_dated_research() {
    let world = World::new();
    world.kit_models("codex", &["gpt-6-sol"]);
    world.with_kit(|kit| {
        let codex = kit["harnesses"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|h| h["harness"] == "codex")
            .unwrap();
        codex["models"][0]["good_at"] = json!("not researched");
        codex["models"][0]["probed"] = json!(1);
    });
    let doctor = world.afk(&["doctor", "--all"]).ok();
    assert!(
        doctor["findings"].as_array().unwrap().iter().any(|f| f["what"]
            .as_str()
            .unwrap()
            .contains("unplaced models")
            && f["evidence"].to_string().contains("gpt-6-sol")),
        "{doctor}"
    );
    let mut research = finding("gpt-6-sol");
    research["harness"] = json!("codex");
    let applied = apply(&world, &json!({"models":[research]})).ok();
    assert_eq!(applied["placed"], json!([{"harness":"codex", "model":"gpt-6-sol"}]));
    let doctor = world.afk(&["doctor", "--all"]).ok();
    assert!(
        !doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["what"].as_str().unwrap().contains("unplaced models")),
        "{doctor}"
    );
}

#[test]
fn malformed_or_inapplicable_research_changes_neither_the_kit_nor_its_time() {
    let world = pending();
    let before = std::fs::read(world.kit_path()).unwrap();
    let modified = std::fs::metadata(world.kit_path()).unwrap().modified().unwrap();
    let mut cases = vec![
        (json!({"models":[]}), "places no model"),
        (json!({"models":[], "extra":true}), "unknown field"),
    ];
    for (key, value, fault) in [
        ("harness", json!("absent"), "harness not in"),
        ("model", json!("absent"), "model not listed"),
        ("good_at", json!(" "), "good_at"),
        ("good_at", json!("research pending"), "good_at"),
        ("names", json!([""]), "names"),
        ("sources", json!([]), "dated source"),
        ("sources", json!([{"url":"file:///card", "date":"2026-10-24"}]), "source URL"),
        ("sources", json!([{"url":"https:///card", "date":"2026-10-24"}]), "source URL"),
        (
            "sources",
            json!([{"url":"https://vendor.example/card", "date":"2026-02-30"}]),
            "calendar date",
        ),
    ] {
        let mut bad = finding("fast");
        bad[key] = value;
        // A valid placement preceding the fault must not be written either.
        cases.push((json!({"models":[finding("deep"), bad]}), fault));
    }
    cases.push((json!({"models":[finding("fast"), finding("fast")]}), "identity twice"));
    for (file, fault) in cases {
        let error = apply(&world, &file).error();
        assert!(error.contains(fault) && error.contains("research.json"), "{error}");
        assert_eq!(std::fs::read(world.kit_path()).unwrap(), before, "{fault}");
        assert_eq!(std::fs::metadata(world.kit_path()).unwrap().modified().unwrap(), modified);
    }
    let file = world.root.join("bad.json");
    std::fs::write(&file, "```json\n{}\n```").unwrap();
    assert!(
        world
            .afk(&["kit", "--research", file.to_str().unwrap()])
            .error()
            .contains("one JSON object")
    );
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), before);
}

#[test]
fn partial_research_preserves_defaults_pending_models_established_priority_and_shared_identities() {
    let world = pending();
    world.kit_command("pi", "pi");
    world.kit_models("pi", &["provider/deep"]);
    world.with_kit(|kit| {
        kit["cast"] = json!(true);
        kit["harnesses"][1]["models"][0]["good_at"] = json!("research pending");
    });
    let doctor = world.afk(&["doctor", "--all"]).ok();
    assert_eq!(
        doctor["findings"].as_array().unwrap().len(),
        2,
        "host findings need no listed checkout"
    );
    let refreshed = world.afk(&["doctor"]).json["host"]["host_kit"]["refreshed"].clone();
    let first = apply(&world, &json!({"models":[finding("fast")]})).ok();
    assert_eq!(first["placed"], json!([{"harness":"claude-code", "model":"fast"}]));
    let old = first["kit"]["harnesses"][0]["models"][0].clone();
    let doctor = world.afk(&["doctor"]).json;
    assert_eq!(doctor["host"]["host_kit"]["harnesses"][1]["default_model"], true);
    assert_eq!(doctor["host"]["host_kit"]["refreshed"], refreshed);
    let placed = apply(&world, &json!({"models":[finding("deep")]})).ok();
    assert_eq!(
        placed["placed"],
        json!([{"harness":"claude-code", "model":"deep"}, {"harness":"pi", "model":"provider/deep"}])
    );
    assert_eq!(placed["kit"]["cast"], true);
    assert_eq!(
        placed["kit"]["harnesses"][0]["models"][0], old,
        "established priority and research survive"
    );
    assert_eq!(placed["kit"]["harnesses"][0]["models"][2]["good_at"], "research pending");
    let before = std::fs::read(world.kit_path()).unwrap();
    assert!(
        apply(&world, &json!({"models":[finding("deep")]})).error().contains("places no model")
    );
    assert_eq!(std::fs::read(world.kit_path()).unwrap(), before);
}

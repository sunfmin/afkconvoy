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
fn an_issue_s_casting_names_the_harness_and_model_its_worker_runs() {
    let world = cast(&["harness:pi", "model:a/slow"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "pi", "model": "a/slow", "why": "casting"})
    );
    let ticked = world.afk(&TICK).ok();
    assert_eq!(
        started(&world),
        [Harness::Pi.with_model(Harness::Pi.stock(), "a/slow")],
        "{ticked}"
    );
    // A harness named alone is started on its command as written.
    let world = cast(&["harness:codex"]);
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);
}

#[test]
fn an_issue_with_no_casting_runs_the_host_kit_s_first() {
    let world = cast(&["bug"]);
    let planned = world.afk(&PLAN).ok();
    assert_eq!(
        planned["frontier"][0]["cast"],
        json!({"harness": "claude-code", "model": "opus", "why": "first"})
    );
    world.afk(&TICK).ok();
    assert_eq!(started(&world), ["agent --model 'opus'"]);
}

#[test]
fn a_casting_the_host_kit_does_not_hold_is_not_dispatched_and_plan_says_why() {
    for (labels, said) in [
        (
            &["harness:pi", "model:a/none"][..],
            "`model:a/none` names a model the host kit holds for no `pi`",
        ),
        (
            &["harness:codex", "model:gpt-x"][..],
            "`model:gpt-x` names a model the host kit holds for no `codex`",
        ),
        (&["model:opus"][..], "`model:opus` names a model of no harness"),
    ] {
        let world = cast(labels);
        let planned = world.afk(&PLAN).ok();
        assert_eq!(planned["frontier"], json!([]), "{labels:?}");
        let excluded = &planned["excluded"][0];
        assert_eq!(excluded["number"], 7);
        let reason = excluded["reason"].as_str().unwrap();
        assert!(reason.starts_with("not dispatched: ") && reason.contains(said), "{reason}");
        // A casting it does not hold is told with what it holds.
        let holds = "it holds: claude-code (opus, haiku); pi (a/fast, a/slow); codex (no model)";
        assert_eq!(reason.contains(holds), labels[0].starts_with("harness:"), "{reason}");
        world.afk(&TICK).ok();
        assert_eq!(started(&world), [] as [String; 0], "{labels:?}");
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
        json!({"harness": "codex", "model": null, "why": "switch"})
    );
    world.afk(&TICK).ok();
    assert_eq!(started(&world), [Harness::Codex.stock()]);
}

//! A claim whose worker declared a verdict takes no slot: it is held until the
//! verdict is acted on, and the frontier does not wait for that.

use crate::common;

use common::World;
use serde_json::{Value, json};

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

/// A world whose two slots are taken at 1000, by workers on #7 and #8, with
/// `more` ready on the frontier.
fn full(more: &[u64]) -> World {
    let world = World::new();
    world.say_concurrency(&world.clone, 2);
    for issue in [7, 8].iter().chain(more) {
        world.issue(*issue, &["ready-for-agent"]);
    }
    let first = did(&tick(&world, 1000));
    assert_eq!(first[..2], ["dispatch 7 done", "dispatch 8 done"]);
    world
}

/// The worker of #8 declares `marker` at `at`, epoch seconds, and every
/// worker stops then: the grace of each runs from there.
fn declare(world: &World, marker: &str, at: u64) {
    world.comment(8, &format!("<!--convoy:verdict {marker}-->\nwhat the worker says"));
    world.with_gh(|gh| {
        let rows = gh["comments"]["8"].as_array_mut().unwrap();
        rows.last_mut().unwrap()["at"] = json!(at);
    });
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": at * 1000});
            terminal["lastOutputAt"] = json!(at * 1000);
        }
    });
}

fn run_line(world: &World, now: &str) -> Vec<String> {
    let args = ["run", "--foreground", "--host", "h1", "--second-ms", "0"];
    let more = ["--instance", "alpha", "--ticks", "1", "--now", now];
    world.afk_lines(&[&args[..], &more[..]].concat()).0
}

#[test]
fn a_claim_whose_worker_declared_frees_its_slot_for_the_frontier() {
    let world = full(&[9, 10]);
    declare(&world, "phase=escalate", 1050);

    // Within the grace of its worker the verdict waits, and its slot does not.
    let ticked = tick(&world, 1100);
    assert_eq!(
        did(&ticked),
        ["judge 7 nothing", "judge 8 nothing", "dispatch 9 done", "heartbeat done"]
    );
    assert_eq!(ticked["held"], 3);

    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1110"]).ok();
    let declared: Vec<(u64, bool)> = plan["mine"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mine| (mine["number"].as_u64().unwrap(), mine["declared"].as_bool().unwrap()))
        .collect();
    assert_eq!(declared, [(7, false), (8, true), (9, false)]);
    assert_eq!(plan["free_slots"], 0, "the two slots are taken by #7 and #9");

    let panel = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["panel", "--instance", "alpha", "--host", "h1", "--now", "1110"])
        .output()
        .unwrap();
    let page = String::from_utf8(panel.stdout).unwrap();
    let gauges = page.split("<dl class=\"gauges\">").nth(1).unwrap().split("</dl>").next();
    let gauges = gauges.unwrap();
    assert!(gauges.contains("<dt>Claims held</dt><dd>2 <small>of 2</small></dd>"), "{gauges}");
    assert!(gauges.contains("<dt>Declared</dt><dd>1</dd>"), "{gauges}");

    // The line says the three claims apart: two of two slots, and one declared.
    assert_eq!(
        run_line(&world, "1120"),
        ["tick 1: nothing to do · holding 2/2 · 1 declared · frontier 1 · next in 90s"]
    );

    // Once the grace runs out it is acted on as ever, and frees no slot it never took.
    let ticked = tick(&world, 1400);
    assert_eq!(did(&ticked)[..2], ["judge 7 nothing", "judge 8 done"]);
    assert!(!did(&ticked).contains(&"dispatch 10 done".to_string()), "{ticked}");
    assert_eq!(world.labels(8), ["ready-for-human"]);
    assert_eq!(ticked["held"], 2);
}

#[test]
fn a_verdict_from_an_earlier_claim_frees_no_slot() {
    let world = full(&[9]);
    // Left on the issue before the claim held now was made at 1000.
    declare(&world, "phase=escalate", 900);
    let ticked = tick(&world, 1100);
    assert_eq!(did(&ticked), ["judge 7 nothing", "judge 8 nothing", "heartbeat done"]);
    assert_eq!(ticked["held"], 2);
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1110"]).ok();
    assert_eq!(plan["mine"][1]["declared"], false);
}

#[test]
fn a_worker_started_again_under_a_declared_claim_waits_for_a_free_slot() {
    // Every blocker it named has closed by the time its grace runs out.
    let blocked = |world: &World| {
        world.issue(5, &[]);
        world.close(5, "completed");
        declare(world, "phase=blocked blocked_by=5", 1050);
    };

    // The two slots are taken by #7 and #9: the start waits, and the claim is held.
    let world = full(&[9]);
    blocked(&world);
    assert_eq!(did(&tick(&world, 1100))[2], "dispatch 9 done");
    let ticked = tick(&world, 1400);
    assert_eq!(did(&ticked)[1], "judge 8 nothing");
    assert_eq!(world.claims().len(), 3);
    assert_eq!(ticked["held"], 3);

    // With a slot free, it is started again into it.
    let world = full(&[]);
    blocked(&world);
    let ticked = tick(&world, 1400);
    assert_eq!(did(&ticked)[1], "judge 8 continued");
    assert_eq!(ticked["held"], 2);
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1410"]).ok();
    assert_eq!(plan["mine"][1]["declared"], false, "the verdict it answered is not its own");
}

//! Admission: a busy host starts no new worker, and `tick`, `run`, `plan` and
//! `doctor` say why. What holds is the core's to decide, on its boundaries;
//! these are the reads of the stand-in host (`World::busy`) and what each
//! command says of them.

use crate::common;

use common::World;
use serde_json::Value;

/// A load no host of any number of cores is under.
const PAST_ANY: &str = "100000.00";

fn backlog(numbers: &[u64]) -> World {
    let world = World::new();
    for &n in numbers {
        world.issue(n, &["ready-for-agent"]);
    }
    world
}

fn tick(world: &World, now: &str) -> Value {
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&[&args[..], &[]].concat()).ok()
}

fn dispatched(ticked: &Value) -> Vec<u64> {
    let did = ticked["did"].as_array().unwrap();
    let started = did.iter().filter(|step| step["do"] == "dispatch" && step["outcome"] == "done");
    started.map(|step| step["issue"].as_u64().unwrap()).collect()
}

#[test]
fn a_busy_host_dispatches_nothing_new_and_the_frontier_waits_for_the_next_tick() {
    let world = backlog(&[3, 5]);
    world.busy(PAST_ANY);
    let held = tick(&world, "1000");
    assert_eq!(dispatched(&held), Vec::<u64>::new(), "{held}");
    assert_eq!(held["frontier_remaining"], 2);
    let by = held["held_back_by"][0].as_str().unwrap();
    assert!(by.starts_with("the load 100000.00 is past "), "{by}");
    assert!(by.contains("200% of its "), "{by}");
    assert!(world.claims().is_empty());

    // The host at ease again: the next tick starts them.
    world.busy("0.00");
    let admitted = tick(&world, "1090");
    assert_eq!(dispatched(&admitted), [3, 5]);
    assert!(admitted["held_back_by"].is_null(), "{admitted}");
}

#[test]
fn the_tick_log_line_says_which_threshold_held_it_back() {
    let world = backlog(&[3]);
    world.busy(PAST_ANY);
    let args = [
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--ticks",
        "1",
        "--now",
        "1000",
    ];
    let more = ["--second-ms", "0"];
    let (lines, _) = world.afk_lines(&[&args[..], &more[..]].concat());
    let line = lines.iter().find(|line| line.contains("admission holds back new workers"));
    let line = line.unwrap_or_else(|| panic!("{lines:#?}"));
    assert!(line.contains(": the load 100000.00 is past "), "{line}");
    assert!(line.contains("frontier 1"), "{line}");
    assert!(!line.contains("nothing to do"), "{line}");
}

#[test]
fn plan_says_what_would_be_dispatched_and_that_admission_holds_it_back() {
    let world = backlog(&[3, 5]);
    world.busy(PAST_ANY);
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    let frontier: Vec<u64> = plan["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["number"].as_u64().unwrap())
        .collect();
    assert_eq!(frontier, [3, 5], "what would be dispatched");
    assert_eq!(plan["held_back"][0]["by"], "load", "{plan}");
    let admission = &plan["admission"];
    assert_eq!(admission["admits"], false);
    let by = admission["held_back_by"][0].as_str().unwrap();
    assert!(by.starts_with("the load 100000.00 is past "), "{by}");
    assert_eq!(admission["reading"]["load"], 100000.0);
    assert_eq!(admission["reading"]["memory_free_percent"], 80);

    world.busy("0.00");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert!(plan["held_back"].is_null(), "{plan}");
    assert_eq!(plan["admission"]["admits"], true);
    assert_eq!(plan["admission"]["held_back_by"], serde_json::json!([]));
}

#[test]
fn doctor_shows_the_thresholds_in_force_and_what_the_host_reads() {
    let world = backlog(&[]);
    world.busy("1.50");
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["doctor", "--host", "h1", "--now", "1000"])
        .env("AFKCONVOY_ADMIT_LOAD", "150")
        .env("AFKCONVOY_ADMIT_MEMORY", "90")
        .output()
        .unwrap();
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let admission = &answer["host"]["admission"];
    let cores = admission["reading"]["cores"].as_f64().unwrap();
    assert_eq!(admission["shares"]["load_percent_of_cores"], 150, "{answer}");
    assert_eq!(admission["thresholds"]["load"].as_f64().unwrap(), cores * 1.5);
    assert_eq!(admission["reading"]["load"], 1.5);
    assert_eq!(admission["thresholds"]["memory_free_percent"], 90);
    let said: Vec<&str> = admission["shares"]["said_by_host"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(said, ["AFKCONVOY_ADMIT_LOAD", "AFKCONVOY_ADMIT_MEMORY"]);
    assert_eq!(admission["reading"]["disk_free_bytes"], 500u64 << 30, "the stand-in `df`");
    assert_eq!(
        admission["held_back_by"],
        serde_json::json!(["80% of its memory is free, under 90%"])
    );
}

#[test]
fn a_share_the_host_says_wrong_is_an_error_not_a_default() {
    let world = backlog(&[3]);
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["plan", "--now", "1000"])
        .env("AFKCONVOY_ADMIT_LOAD", "lots")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let error = answer["error"].as_str().unwrap();
    assert!(error.contains("AFKCONVOY_ADMIT_LOAD=\"lots\" is no share"), "{error}");
}

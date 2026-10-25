//! A public tracker is refused: anyone can comment on its issues, and so
//! write what the fleet reads as its state and what its workers read as
//! their task (docs/adr/0027). A start's probe refuses one, so does `run
//! --tracker`, and a running fleet instance whose tracker is made public
//! ends as its next check finds it, within the hour one that found it
//! private stands.

use crate::common;

use std::time::{Duration, Instant};

use common::World;
use serde_json::{Value, json};

const TRACKER: &str = "acme/tracker";
/// The repo label of the checkout's repo, `acme/dev`.
const MINE: &str = "repo:acme/dev";
const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "100"];

/// An unbound repo, `acme/dev`, whose gate passes, and which GitHub says is
/// `visibility`.
fn world(visibility: &str) -> World {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, "acme/dev");
    world.with_gh(|gh| gh["visibility"] = json!(visibility));
    world
}

/// The line that says a private tracker of `world`'s checkout.
fn fix(world: &World) -> String {
    format!("afkconvoy run {} --tracker <owner>/<private repo>", world.clone.display())
}

/// Whether `refused` refuses `world`'s checkout for its tracker `named`,
/// with the line that says a private one.
fn refuses(world: &World, refused: &str, named: &str) -> bool {
    let said = format!("the tracker of {}, {named}, is public", world.clone.display());
    refused.contains(&said)
        && refused.contains("anyone can comment on its issues")
        && refused.contains(&format!("`{}`", fix(world)))
}

/// Waits until `done`, for 60 seconds at the most.
fn until(what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(60), "never happened: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The findings of `doctor` of the kind whose `what` holds `symptom`.
fn found<'a>(doctor: &'a Value, symptom: &str) -> Vec<&'a Value> {
    let findings = doctor["findings"].as_array().unwrap().iter();
    findings.filter(|found| found["what"].as_str().unwrap().contains(symptom)).collect()
}

#[test]
fn a_checkout_of_a_public_repo_that_says_no_tracker_is_refused_by_every_start() {
    let world = world("public");
    world.issue(7, &["ready-for-agent"]);
    world.watch_pushes(None);

    let refused = world.afk(&RUN).error();
    assert!(refuses(&world, &refused, "acme/dev"), "{refused}");
    assert_eq!(world.checkout_list()["checkouts"], json!([]), "nothing is on the list");
    for start in [&["plan"][..], &["tick", "--instance", "alpha", "--host", "h1"]] {
        let refused = world.afk(start).error();
        assert!(refuses(&world, &refused, "acme/dev"), "{start:?}: {refused}");
    }
    // Refused before the probe pushes anything, or the fleet claims anything.
    assert_eq!(world.probes(), [] as [String; 0]);
    assert_eq!(world.pushes(), [] as [String; 0]);
    assert!(world.claims().is_empty());
    assert_eq!(world.labels(7), ["ready-for-agent"]);

    // Saying its own repo again is refused as well.
    let refused = world.afk(&[&RUN[..], &["--tracker", "default"]].concat()).error();
    assert!(refuses(&world, &refused, "acme/dev"), "{refused}");
}

#[test]
fn run_tracker_naming_a_public_repo_is_refused_the_same_way_and_makes_nothing_in_it() {
    let world = world("private");
    world.put_tracker(TRACKER);
    world.with_tracker_of(TRACKER, |gh| gh["visibility"] = json!("public"));

    let refused = world.afk(&[&RUN[..], &["--tracker", TRACKER]].concat()).error();
    assert!(refuses(&world, &refused, TRACKER), "{refused}");
    assert_eq!(world.tracker_of(TRACKER)["labels"], json!(["ready-for-agent"]), "none made");
    assert_eq!(world.checkout_list()["checkouts"], json!([]), "nothing is on the list");
}

#[test]
fn run_tracker_naming_a_private_repo_of_a_checkout_of_a_public_one_is_taken_and_its_fleet_works() {
    let world = world("public");
    world.put_tracker(TRACKER);
    world.with_tracker_of(TRACKER, |gh| {
        let issue = json!({"number": 7, "title": "tracked 7", "state": "open",
            "labels": ["ready-for-agent", MINE]});
        gh["issues"].as_array_mut().unwrap().push(issue);
    });

    let ran = world.afk(&[&RUN[..], &["--tracker", TRACKER]].concat()).ok();
    assert_eq!(ran["tracker"], json!({"from": null, "to": TRACKER}), "{ran}");
    until("the tracker's #7 is dispatched", || !world.claims().is_empty());
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(found(&doctor, "is public"), [] as [&Value; 0], "{doctor}");
}

#[test]
fn a_probe_that_found_the_tracker_private_stands_for_the_hour_and_no_longer() {
    let world = world("private");
    world.issue(7, &["ready-for-agent"]);
    let tick = |now: &str| world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]);
    tick("1000").ok();
    world.with_gh(|gh| gh["visibility"] = json!("public"));

    // Within the hour its probe stands, and nothing asks again.
    tick("4599").ok();
    // Past it, the probe is made again, and refuses it.
    let refused = tick("4600").error();
    assert!(refuses(&world, &refused, "acme/dev"), "{refused}");
    // Private again, the next start passes; of a tracker said anew, a start
    // asks at once, whatever stands.
    world.with_gh(|gh| gh["visibility"] = json!("private"));
    tick("4600").ok();
    world.put_tracker(TRACKER);
    world.with_tracker_of(TRACKER, |gh| gh["visibility"] = json!("public"));
    world.say_tracker(&world.clone, TRACKER);
    let refused = world.afk(&["plan", "--now", "4601"]).error();
    assert!(refuses(&world, &refused, TRACKER), "{refused}");
}

#[test]
fn a_fleet_instance_whose_tracker_is_made_public_works_nothing_from_its_next_check_until_it_is_private()
{
    let world = world("private");
    let first = world.afk(&RUN).ok();
    assert!(first["first_tick"].as_str().unwrap().contains("frontier 0"), "{first}");

    // The repo is made public, and the hour its check stands is over.
    world.with_gh(|gh| gh["visibility"] = json!("public"));
    world.issue(7, &["ready-for-agent"]);
    let note = world.clone.join(".git/convoy-tracker");
    let mut noted: Value = serde_json::from_str(&std::fs::read_to_string(&note).unwrap()).unwrap();
    noted["at"] = json!(noted["at"].as_u64().unwrap() - 3600);
    std::fs::write(&note, noted.to_string()).unwrap();
    world.afk(&["wake"]).ok();

    // Its next check ends it: it is broken, and works nothing.
    let broken = || {
        let checkouts = world.afk(&["checkouts"]).ok();
        checkouts["checkouts"][0]["state"] == "broken"
    };
    until("the fleet instance is broken", broken);
    assert!(world.claims().is_empty(), "#7 is not dispatched");
    assert_eq!(world.labels(7), ["ready-for-agent"]);

    // `doctor` says why, with the line that fixes it.
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let public = found(&doctor, "is public: its fleet instance works nothing");
    assert_eq!(public.len(), 1, "{doctor}");
    assert_eq!((&public[0]["severity"], &public[0]["fix"]), (&json!("critical"), &json!(fix(&world))));
    assert!(refuses(&world, public[0]["evidence"].as_str().unwrap(), "acme/dev"), "{doctor}");
    let broken = found(&doctor, "is broken");
    assert_eq!(broken.len(), 1, "{doctor}");
    assert!(refuses(&world, broken[0]["what"].as_str().unwrap(), "acme/dev"), "{doctor}");
    assert_eq!(broken[0]["fix"], json!(fix(&world)), "{doctor}");

    // Private again, the host runner's next start of it works.
    world.with_gh(|gh| gh["visibility"] = json!("private"));
    until("#7 is dispatched", || !world.claims().is_empty());
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(found(&doctor, "is public"), [] as [&Value; 0], "{doctor}");
}

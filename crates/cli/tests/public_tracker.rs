//! A public tracker is refused: anyone can comment on its issues, and so
//! write what the fleet reads as its state and what its workers read as
//! their task (docs/adr/0091). A start's probe refuses one, so does `run
//! --tracker`, and a running fleet instance whose tracker is made public
//! ends as its next check finds it, within the hour one that found it
//! private stands.

use crate::common;

use std::process::Child;

use common::World;
use serde_json::{Value, json};

const TRACKER: &str = "acme/tracker";
/// The repo label of the checkout's repo, `acme/dev`.
const MINE: &str = "repo:acme/dev";
const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "100"];

/// An unbound repo, `acme/dev`, whose gate passes, which GitHub says is
/// `visibility`, and whose checkout names its own repo as its tracker.
fn world(visibility: &str) -> World {
    let world = World::new();
    world.set_gate("true");
    world.name_origin(&world.clone, "acme/dev");
    world.with_gh(|gh| gh["visibility"] = json!(visibility));
    world.say_said_tracker(&world.clone, "acme/dev");
    world
}

/// The checkout list of `world` as it starts: its checkout, naming its own
/// repo.
fn own_entry(world: &World) -> Value {
    json!([{"checkout": world.clone, "tracker": "acme/dev"}])
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

/// Waits until `done`.
fn until(what: &str, done: impl Fn() -> bool) {
    afkconvoy_patience::until_true(what, done)
}

/// The findings of `doctor` of the kind whose `what` holds `symptom`.
fn found<'a>(doctor: &'a Value, symptom: &str) -> Vec<&'a Value> {
    let findings = doctor["findings"].as_array().unwrap().iter();
    findings.filter(|found| found["what"].as_str().unwrap().contains(symptom)).collect()
}

#[test]
fn a_checkout_of_a_public_repo_that_names_its_own_repo_is_refused_by_every_start() {
    let world = world("public");
    world.issue(7, &["ready-for-agent"]);
    world.watch_pushes(None);

    let refused = world.afk(&RUN).error();
    assert!(refuses(&world, &refused, "acme/dev"), "{refused}");
    assert_eq!(world.checkout_list()["checkouts"], own_entry(&world), "nothing is changed");
    for start in [&["plan"][..], &["tick", "--instance", "alpha", "--host", "h1"]] {
        let refused = world.afk(start).error();
        assert!(refuses(&world, &refused, "acme/dev"), "{start:?}: {refused}");
    }
    // Refused before the probe pushes anything, or the fleet claims anything.
    assert_eq!(world.probes(), [] as [String; 0]);
    assert_eq!(world.pushes(), [] as [String; 0]);
    assert!(world.claims().is_empty());
    assert_eq!(world.labels(7), ["ready-for-agent"]);

    // Saying its own repo again is refused as well, in whatever case.
    let refused = world.afk(&[&RUN[..], &["--tracker", "Acme/Dev"]].concat()).error();
    assert!(refuses(&world, &refused, "acme/dev"), "{refused}");
}

#[test]
fn a_checkout_of_a_public_repo_that_says_no_tracker_works_the_local_one_and_is_not_refused() {
    let world = world("public");
    world.unlist_checkout(&world.clone);
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    assert_eq!(plan["tracker"], "local/h1", "{plan}");
    // `default` says it again, of a checkout that named its own repo.
    let world = world_named_then_default();
    let plan = world.afk(&["plan", "--instance", "alpha"]).ok();
    assert_eq!(plan["tracker"], "local/h1", "{plan}");
}

/// A public repo's checkout that named its own repo, then said `default`.
fn world_named_then_default() -> World {
    let world = world("public");
    world.say_said_tracker(&world.clone, Value::Null);
    world
}

#[test]
fn run_tracker_naming_a_public_repo_is_refused_the_same_way_and_makes_nothing_in_it() {
    let world = world("private");
    world.put_tracker(TRACKER);
    world.with_tracker_of(TRACKER, |gh| gh["visibility"] = json!("public"));

    let refused = world.afk(&[&RUN[..], &["--tracker", TRACKER]].concat()).error();
    assert!(refuses(&world, &refused, TRACKER), "{refused}");
    assert_eq!(world.tracker_of(TRACKER)["labels"], json!(["ready-for-agent"]), "none made");
    assert_eq!(world.checkout_list()["checkouts"], own_entry(&world), "nothing is changed");
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
    assert_eq!(ran["tracker"], json!({"from": "acme/dev", "to": TRACKER}), "{ran}");
    until("the tracker's #7 is dispatched", || !world.claims().is_empty());
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(found(&doctor, "is public"), [] as [&Value; 0], "{doctor}");
}

/// Has `world`'s checkout on the host's checkout list as an older afkconvoy
/// left it, saying no tracker, under the world's host settings: its file in
/// the cache directory, which the next read takes into the host database
/// naming the checkout's own repo, `acme/dev`, as that release meant it.
fn listed_untracked(world: &World) {
    let file = world.cache().join("afkconvoy/checkouts.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut list = world.checkout_list();
    list["checkouts"] =
        json!([{"checkout": world.clone, "named": {"host": "h1"}, "tracker": "acme/dev"}]);
    std::fs::write(file, list.to_string()).unwrap();
}

/// Has `world`'s checkout on the host's checkout list as an older afkconvoy
/// left it (`listed_untracked`), and that afkconvoy's host runner hold the
/// host → it, which the next `run` moves onto this afkconvoy.
fn listed_by_an_older_host_runner(world: &World) -> Child {
    listed_untracked(world);
    world.host_runner_of("0.0.1")
}

/// The lines of the host runner's log of `world`'s checkout that say a start
/// of its fleet instance did not come to its first tick.
fn refused_starts(world: &World) -> Vec<String> {
    let at = world.clone.display().to_string();
    let lines = world.runner_log().into_iter().filter(|line| {
        ["fleet instance broken: ", "fleet instance not started: ", "checkout "]
            .iter()
            .any(|said| line.starts_with(&format!("{said}{at}")))
    });
    lines.filter(|line| !line.starts_with("checkout added: ")).collect()
}

#[test]
fn run_tracker_that_moves_an_older_host_runner_starts_its_checkout_under_that_tracker_at_once() {
    let world = world("public");
    world.put_tracker(TRACKER);
    world.with_tracker_of(TRACKER, |gh| {
        let issue = json!({"number": 7, "title": "tracked 7", "state": "open",
            "labels": ["ready-for-agent", MINE]});
        gh["issues"].as_array_mut().unwrap().push(issue);
    });
    let mut older = listed_by_an_older_host_runner(&world);
    let old = older.id();

    // Taken at the first `run`: the host runner it starts starts the checkout
    // from the list, and the list says the tracker by then.
    let ran = world.afk(&[&RUN[..], &["--tracker", TRACKER]].concat()).ok();
    assert_eq!(ran["host_runner"]["replaced"], json!({"pid": old, "version": "0.0.1"}), "{ran}");
    assert!(older.wait().unwrap().success());
    assert_eq!(ran["added"], false, "{ran}");
    assert_eq!(ran["tracker"], json!({"from": "acme/dev", "to": TRACKER}), "{ran}");
    assert_eq!(world.checkout_list()["checkouts"][0]["tracker"], TRACKER);

    // Never started under the entry it had: no start of it was refused.
    assert_eq!(refused_starts(&world), [] as [String; 0]);
    until("the tracker's #7 is dispatched", || !world.claims().is_empty());
}

#[test]
fn run_tracker_that_starts_the_host_runner_starts_its_listed_checkout_under_that_tracker_at_once() {
    let world = world("public");
    world.put_tracker(TRACKER);
    listed_untracked(&world);

    let ran = world.afk(&[&RUN[..], &["--tracker", TRACKER]].concat()).ok();
    assert_eq!(ran["host_runner"]["started"], true, "{ran}");
    assert_eq!(ran["host_runner"]["replaced"], Value::Null, "{ran}");
    assert_eq!(ran["tracker"], json!({"from": "acme/dev", "to": TRACKER}), "{ran}");
    assert_eq!(refused_starts(&world), [] as [String; 0]);
}

#[test]
fn a_listed_checkout_of_a_public_repo_a_move_starts_is_refused_and_stays_on_the_list_broken() {
    let world = world("public");
    let mut older = listed_by_an_older_host_runner(&world);

    // Taken into the host database, its entry names its own repo, the
    // tracker the older release meant (ADR-0117): a `run` that says no
    // other refuses that public tracker before it moves the older runner,
    // with the line that fixes it, and the checkout stays on the list.
    let refused = world.afk(&RUN);
    assert_eq!(refused.code, 1, "{}", refused.json);
    let error = refused.json["error"].as_str().unwrap();
    assert!(refuses(&world, error, "acme/dev"), "{error}");
    let listed = world.checkout_list()["checkouts"].clone();
    let pinned = json!([{"checkout": world.clone, "named": {"host": "h1"}, "tracker": "acme/dev"}]);
    assert_eq!(listed, pinned, "its own repo named, as the older release meant it");
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(found(&doctor, "is public: its fleet instance works nothing").len(), 1, "{doctor}");

    // And that line moves the older runner and starts it at once.
    world.put_tracker(TRACKER);
    let ran = world.afk(&[&RUN[..], &["--tracker", TRACKER]].concat()).ok();
    assert_eq!(ran["tracker"], json!({"from": "acme/dev", "to": TRACKER}), "{ran}");
    assert_eq!(ran["added"], false, "{ran}");
    assert!(older.wait().unwrap().success());
}

#[test]
fn a_probe_that_found_the_tracker_private_stands_for_the_hour_and_no_longer() {
    let world = world("private");
    world.issue(7, &["ready-for-agent"]);
    let tick =
        |now: &str| world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]);
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
    // A second of its sleeps takes 10ms: the host runner starts it again 0.6s
    // after it broke, not 6s.
    const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "10"];
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
    assert_eq!(
        (&public[0]["severity"], &public[0]["fix"]),
        (&json!("critical"), &json!(fix(&world)))
    );
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

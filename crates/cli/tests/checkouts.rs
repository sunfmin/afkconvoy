//! `checkouts` and `doctor --all`: every repo of the host, as JSON, read
//! from the checkout list and, where one runs, the host runner — what a
//! session that looks after every repo of the host asks instead of keeping
//! a list of its own (docs/adr/0023).

use crate::common;

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use common::World;
use serde_json::{Value, json};

/// A `run` on the host `h1`, its sleeps short.
const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "100"];

/// The keys of an object.
fn keys(object: &Value) -> BTreeSet<String> {
    object.as_object().unwrap().keys().cloned().collect()
}

/// The answer's row of the checkout `checkout`.
fn row<'a>(answer: &'a Value, checkout: &Path) -> &'a Value {
    let rows = answer["checkouts"].as_array().unwrap();
    rows.iter().find(|row| row["checkout"] == json!(checkout)).unwrap_or_else(|| {
        panic!("no row of {} in {answer}", checkout.display());
    })
}

/// Puts `checkouts` on this world's checkout list, as `run` would have,
/// with no host runner started.
fn list(world: &World, checkouts: &[&Path]) {
    let file = world.cache().join("afkconvoy/checkouts.json");
    let mut listed = world.checkout_list();
    let rows = checkouts.iter().map(|checkout| json!({"checkout": checkout}));
    listed["checkouts"] = Value::Array(rows.collect());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, listed.to_string()).unwrap();
}

#[test]
fn checkouts_answers_each_checkout_on_the_list_as_the_host_runner_holds_it() {
    let world = World::new();
    // none on the list, and no host runner: an empty list answers
    let none = world.afk(&["checkouts"]).ok();
    assert_eq!(none, json!({"host_runner": null, "checkouts": []}));

    world.issue(7, &["ready-for-agent"]);
    let first = world.afk(&RUN).ok();
    assert!(first["first_tick"].as_str().unwrap().contains("dispatched #7"), "{first}");
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();

    // from anywhere on the host: it is about no one checkout
    let answer = world.afk_in(&world.root, &["checkouts"]).ok();
    // what the help shows is what it answers, key for key
    let help = &common::shown("`afkconvoy checkouts`", "json")[0];
    let help: Value = serde_json::from_str(help).unwrap();
    assert_eq!(keys(&answer), keys(&help), "{answer}");
    assert_eq!(keys(&answer["host_runner"]), keys(&help["host_runner"]), "{answer}");
    let about = &first["host_runner"];
    for key in ["pid", "version", "log"] {
        assert_eq!(answer["host_runner"][key], about[key], "{answer}");
    }
    let rows = answer["checkouts"].as_array().unwrap();
    let checkouts: Vec<&Value> = rows.iter().map(|row| &row["checkout"]).collect();
    assert_eq!(checkouts, [&json!(world.clone), &json!(clone2)], "in the list's order");
    for row in rows {
        assert_eq!(keys(row), keys(&help["checkouts"][0]), "{row}");
    }

    // each as its `run` answered it, and as its last tick ended
    let (one, two) = (row(&answer, &world.clone), row(&answer, &clone2));
    for (row, ran) in [(one, &first), (two, &second)] {
        assert_eq!(row["state"], "running", "{row}");
        for key in ["repo", "instance", "tick_log"] {
            assert_eq!(row[key], ran[key], "{key}: {row}");
        }
        assert!(row["ticked"].as_u64().is_some(), "{row}");
    }
    // #7's worker holds its claim: not idle; the second repo has nothing to do
    assert_eq!(
        (&one["idle"], &one["holding"], &one["frontier"]),
        (&json!(false), &json!(1), &json!(0))
    );
    assert_eq!(
        (&two["idle"], &two["holding"], &two["frontier"]),
        (&json!(true), &json!(0), &json!(0))
    );

    // with the host runner stopped, from the list alone: the same checkouts,
    // stopped, each with the fleet instance its tick log names
    world.afk(&["stop", "--all"]).ok();
    let stopped = world.afk(&["checkouts"]).ok();
    assert_eq!(stopped["host_runner"], Value::Null, "{stopped}");
    for (checkout, ran) in [(&world.clone, &first), (&clone2, &second)] {
        let row = row(&stopped, checkout);
        assert_eq!(row["state"], "stopped", "{row}");
        assert_eq!(row["instance"], ran["instance"], "{row}");
        assert!(row["holding"].is_u64() && row["ticked"].is_u64(), "as it last ticked: {row}");
    }
    // and it changed nothing
    assert_eq!(world.host_runner(), None);
    assert_eq!(world.afk(&["checkouts"]).ok(), stopped);
}

#[test]
fn a_listed_checkout_the_host_runner_holds_no_fleet_instance_of_is_broken() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    world.kill_host_runner("KILL");
    // the next host runner cannot start the checkout on its list by itself:
    // its login shell sets a flag of `run` to what `run` refuses
    let profile = std::fs::OpenOptions::new().append(true).open(world.home().join(".profile"));
    writeln!(profile.unwrap(), "export AFKCONVOY_HOT_CORES=bogus").unwrap();
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();
    let log = world.runner_log();
    let not_started = format!("fleet instance not started: {}: ", world.clone.display());
    assert!(log.iter().any(|line| line.starts_with(&not_started)), "{log:#?}");

    let answer = world.afk(&["checkouts"]).ok();
    assert_eq!(answer["host_runner"]["pid"], second["host_runner"]["pid"], "{answer}");
    let broken = row(&answer, &world.clone);
    assert_eq!(broken["state"], "broken", "{answer}");
    // the fleet instance that last ran it, as it last ticked
    assert_eq!(broken["instance"], first["instance"], "{answer}");
    assert_eq!(broken["tick_log"], first["tick_log"], "{answer}");
    assert_eq!((&broken["idle"], &broken["holding"]), (&json!(true), &json!(0)), "{answer}");
    assert_eq!(row(&answer, &clone2)["state"], "running", "{answer}");
}

#[test]
fn checkouts_says_a_host_runner_of_an_older_afkconvoy_cannot_say_what_it_holds() {
    let world = World::new();
    list(&world, &[&world.clone]);
    let mut older = world.host_runner_of("0.0.1");
    let error = world.afk(&["checkouts"]).error();
    older.kill().unwrap();
    let _ = older.wait();
    assert!(error.contains("of afkconvoy 0.0.1, cannot say which checkouts it holds"), "{error}");
    assert!(error.contains("`afkconvoy run`, in any checkout, moves it onto this"), "{error}");
}

#[test]
fn doctor_all_answers_each_checkouts_doctor_and_the_hosts_findings_once() {
    let world = World::new();
    let clone2 = world.second_repo();
    let gone = world.root.join("gone");
    list(&world, &[&world.clone, &clone2, &gone]);
    // a harness paused is the host's, whichever checkout finds it
    world.pause("pi", 900, "it refused every prompt");
    // #9 of the first repo waits for a human: it was escalated, with no
    // consult to ask first
    world.set_gate_config("gate_command: ./gate.sh\ngate_seconds: 600\nescalate_to: human");
    world.issue(9, &["ready-for-agent"]);
    let on = ["--issue", "9", "--instance", "alpha", "--now", "900"];
    world.afk(&[&["claim"][..], &on].concat()).ok();
    world.afk(&[&["escalate"][..], &on, &["--reason", "it needs a decision"]].concat()).ok();

    // from anywhere on the host, with no host runner; a checkout that is no
    // git checkout any more could not be asked: non-zero, the same object
    let all = world.afk_in(&world.root, &["doctor", "--all", "--now", "1000"]);
    assert_eq!(all.code, 1, "{}", all.json);
    let answer = all.json;
    assert_eq!(keys(&answer), BTreeSet::from(["checkouts", "findings", "host"].map(String::from)));
    assert_eq!(
        keys(&answer["host"]),
        BTreeSet::from(["exclusive", "host_kit", "inspection"].map(String::from))
    );

    // each checkout's `doctor` under it, as `doctor` in it answers, but for
    // what is the host's
    let rows = answer["checkouts"].as_array().unwrap();
    let checkouts: Vec<&Value> = rows.iter().map(|row| &row["checkout"]).collect();
    assert_eq!(checkouts, [&json!(world.clone), &json!(clone2), &json!(gone)]);
    let alone = world.afk(&["doctor", "--now", "1000"]).json;
    for (checkout, repo) in
        [(&world.clone, &world.bare), (&clone2, &world.root.join("remote2.git"))]
    {
        let row = row(&answer, checkout);
        assert_eq!(row["repo"], json!(repo), "{row}");
        let doctor = &row["doctor"];
        assert_eq!(keys(doctor), keys(&alone), "{doctor}");
        assert!(doctor["waiting_on_human"].is_array(), "{doctor}");
        for key in ["host_kit", "exclusive", "inspection"] {
            assert!(doctor["host"].get(key).is_none(), "{key} is said once: {doctor}");
        }
        for key in ["tools", "settings", "admission"] {
            assert!(doctor["host"].get(key).is_some(), "{key} is the checkout's: {doctor}");
        }
        let paused = doctor["findings"].as_array().unwrap().iter();
        assert!(!paused.clone().any(|found| found["what"].as_str().unwrap().contains("paused")));
    }
    let waiting = &row(&answer, &world.clone)["doctor"]["waiting_on_human"];
    assert_eq!(waiting[0]["issue"], 9, "{waiting}");
    assert_eq!(waiting, &alone["waiting_on_human"]);
    assert_eq!(row(&answer, &clone2)["doctor"]["waiting_on_human"], json!([]));

    // the host's finding once, as `doctor` in either checkout says it
    let paused: Vec<&Value> = answer["findings"].as_array().unwrap().iter().collect();
    assert_eq!(paused.len(), 1, "{answer}");
    assert!(paused[0]["what"].as_str().unwrap().contains("`pi` is paused on this host"));
    let said = alone["findings"].as_array().unwrap();
    assert!(said.contains(paused[0]), "{alone}");

    let unasked = row(&answer, &gone);
    assert_eq!(unasked["repo"], Value::Null, "{unasked}");
    let error = unasked["doctor"]["error"].as_str().unwrap();
    assert!(error.contains("is not a directory"), "{error}");

    // with every checkout asked and nothing critical: 0
    list(&world, &[&world.clone, &clone2]);
    world.afk(&["doctor", "--all", "--now", "1000"]).ok();
    // and none on the list is no checkout to ask
    list(&world, &[]);
    let none = world.afk(&["doctor", "--all"]).ok();
    assert_eq!((&none["checkouts"], &none["findings"]), (&json!([]), &json!([])));
}

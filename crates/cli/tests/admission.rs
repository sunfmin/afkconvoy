//! Admission: a busy host starts no new worker, and `tick`, `run`, `plan` and
//! `doctor` say why. What holds is the core's to decide, on its boundaries;
//! these are the reads of the stand-in host (`World::busy`) and what each
//! command says of them.

use crate::common;

use common::World;
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// A load no host of any number of cores is under.
const PAST_ANY: &str = "100000.00";

/// A command of the exclusive queue, a `sh` that sleeps until it is
/// dropped, running once `exclusive` was granted its turn. Its whole tree
/// ends with the test, the `sh` and its `sleep` too, and not `exclusive`
/// alone (sunfmin/control#508).
fn queued(world: &World) -> common::FixtureProcess {
    let marker = world.root.join("running");
    let command = common::FixtureProcess::spawn(
        world
            .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
            .args(["exclusive", "--", "sh", "-c"])
            .arg(format!("touch '{}'; sleep 30", marker.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    );
    let start = Instant::now();
    while !marker.exists() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    command
}

/// A load past admission's share of this host's cores, as `sysctl` says it:
/// three times them, where twice holds.
fn past_the_share() -> String {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    format!("{}.00", 3 * cores)
}

#[test]
fn a_queue_busy_over_the_last_minute_and_idle_now_holds_its_lagging_load_and_admits() {
    let world = backlog(&[3, 5]);
    let processes = world.root.join("admission-processes");
    // Until the queue's work is said busy, no process takes any CPU.
    std::fs::write(&processes, "999999 1 0.0\n").unwrap();
    let _runner = world.host_held();
    world.busy(PAST_ANY);
    let command = queued(&world);
    // Its tests start programs, which the host's scanning daemons check.
    let busy = format!(
        "{} 1 0.0\n999999 {} 60.0\n999997 1 40.0 /usr/libexec/syspolicyd\n",
        command.id(),
        command.id()
    );
    std::fs::write(&processes, busy).unwrap();
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(doctor["host"]["admission"]["reading"]["queue_load"], 100000.0, "{doctor}");
    assert_eq!(doctor["host"]["admission"]["reading"]["other_load"], 0.0, "{doctor}");
    // Between two of its phases the gate's tree takes nothing for a moment,
    // while the load it made lags on, and something else takes a little.
    world.busy(&past_the_share());
    let idle = format!("{} 1 0.0\n999998 1 5.0\n", command.id());
    std::fs::write(&processes, idle).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let admitted = tick(&world, "1000");
    assert_eq!(dispatched(&admitted), [3, 5], "{admitted}");
    drop(command);
}

#[test]
fn load_outside_the_queue_throughout_holds_while_the_queue_runs() {
    let world = backlog(&[3]);
    let processes = world.root.join("admission-processes");
    std::fs::write(&processes, "999998 1 100.0\n").unwrap();
    let _runner = world.host_held();
    world.busy(&past_the_share());
    let command = queued(&world);
    std::fs::write(&processes, format!("{} 1 0.0\n999998 1 100.0\n", command.id())).unwrap();
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(doctor["host"]["admission"]["reading"]["queue_load"], 0.0, "{doctor}");
    std::thread::sleep(Duration::from_secs(1));
    let held = tick(&world, "1000");
    assert!(dispatched(&held).is_empty(), "{held}");
    assert!(held["held_back_by"].to_string().contains("load"), "{held}");
    drop(command);
}

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
    assert!(by.ends_with("and how busy its cores are could not be read"), "{by}");
    assert!(world.claims().is_empty());

    // The host at ease again: the next tick starts them.
    world.busy("0.00");
    let admitted = tick(&world, "1090");
    assert_eq!(dispatched(&admitted), [3, 5]);
    assert!(admitted["held_back_by"].is_null(), "{admitted}");
}

/// A load past the share with most cores idle is not the host's being
/// busy: macOS counts another account's idle simulators in its load
/// (docs/adr/0084, sunfmin/control#1168).
#[test]
fn a_load_past_its_share_holds_only_while_the_cores_are_busy() {
    let world = backlog(&[3]);
    world.busy(&past_the_share());
    world.cores_busy(95);
    let held = tick(&world, "1000");
    assert!(dispatched(&held).is_empty(), "{held}");
    let by = held["held_back_by"][0].as_str().unwrap();
    assert!(by.ends_with(", and its cores are 95% busy, past 90%"), "{by}");
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(plan["admission"]["reading"]["cpu_busy_percent"], 95, "{plan}");
    assert_eq!(plan["admission"]["thresholds"]["cpu_busy_percent"], 90, "{plan}");

    world.cores_busy(40);
    let admitted = tick(&world, "1090");
    assert_eq!(dispatched(&admitted), [3], "{admitted}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1090"]).json;
    let reading = &doctor["host"]["admission"]["reading"];
    assert_eq!(reading["cpu_busy_percent"], 40, "{doctor}");
    assert!(reading["load"].as_f64().unwrap() > reading["cores"].as_f64().unwrap() * 2.0);
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
    world.say_host_settings(json!({"admit_load": 150, "admit_memory": 90}));
    // the variables that stand for the flags of `run` are `run`'s alone
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["doctor", "--host", "h1", "--now", "1000"])
        .env("AFKCONVOY_ADMIT_LOAD", "400")
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
    assert_eq!(said, ["admit_load", "admit_memory"]);
    assert_eq!(admission["reading"]["disk_free_bytes"], 500u64 << 30, "the stand-in `df`");
    assert_eq!(
        admission["held_back_by"],
        serde_json::json!(["80% of its memory is free, under 90%"])
    );
}

#[test]
fn a_share_said_wrong_is_refused_and_one_kept_wrong_is_an_error_not_a_default() {
    let world = backlog(&[3]);
    let before = world.checkout_list();
    let error = world.afk(&["run", "--admit-load", "lots"]).error();
    assert!(error.contains("`lots` is not a whole number, nor `default`"), "{error}");
    // said by the variable that stands for the flag, the same
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .arg("run")
        .env("AFKCONVOY_ADMIT_DISK", "5%")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let error = answer["error"].as_str().unwrap();
    assert!(error.contains("`5%` is not a whole number"), "{error}");
    assert_eq!(world.checkout_list(), before, "nothing was written");

    // a file mended wrong by hand holds nothing back by a default
    world.say_host_settings(json!({"hot_cores": -1}));
    let error = world.afk(&["plan", "--now", "1000"]).error();
    assert!(error.contains("checkouts.json cannot be read"), "{error}");
}

/// A candidate chain that had more conflicts than landings in the last hour holds new
/// starts of its repo, and says why, while another repo on the host still
/// starts (docs/adr/0076, sunfmin/control#914).
#[test]
fn a_chain_that_cannot_keep_up_holds_its_repo_while_another_repo_starts() {
    let world = backlog(&[]);
    // A tick writes this repo's tick log; the chain's answers go beside it.
    tick(&world, "10000");
    let clone2 = world.second_repo();
    let runs = world.cache().join("afkconvoy").join("runs");
    let log = std::fs::read_dir(&runs)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
        .expect("the first repo's tick log");
    let land = |at: u64, outcome: &str| {
        json!({"at": at, "instance": "alpha", "host": "h1", "command": {"command": "land",
            "on": "#1", "by": "worker", "outcome": outcome}})
        .to_string()
    };
    let lines = [
        land(5000, "conflict"),
        land(8000, "speculative_conflict"),
        land(9000, "conflict"),
        land(9500, "merged"),
        land(9900, "speculative_conflict"),
    ];
    let mut text = std::fs::read_to_string(&log).unwrap();
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }
    std::fs::write(&log, text).unwrap();

    world.issue(3, &["ready-for-agent"]);
    let held = tick(&world, "10090");
    assert!(dispatched(&held).is_empty(), "{held}");
    let by = held["held_back_by"][0].as_str().unwrap_or_default();
    assert!(by.starts_with("landing contention, 3 conflicts in the last hour"), "{held}");
    assert!(world.claims().is_empty());
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "10090"]).ok();
    assert_eq!(plan["held_back"][0]["by"], "landing", "{plan}");
    assert_eq!(plan["admission"]["landing"]["conflicts"], 3, "{plan}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "10090"]).json;
    let said = doctor["findings"].to_string();
    assert!(said.contains("its candidate chain cannot keep up: 3 conflicts against 1"), "{said}");

    // The other repo's candidate chain is clear: its work starts.
    let gh2 = world.root.join("gh2.json");
    let mut state: Value = serde_json::from_str(&std::fs::read_to_string(&gh2).unwrap()).unwrap();
    state["issues"].as_array_mut().unwrap().push(
        json!({"number": 4, "title": "issue 4", "labels": ["ready-for-agent"], "state": "open"}),
    );
    std::fs::write(&gh2, state.to_string()).unwrap();
    let args = ["tick", "--instance", "beta", "--host", "h1", "--now", "10090"];
    let other = world.afk_in(&clone2, &args).ok();
    assert_eq!(dispatched(&other), [4], "{other}");

    // An hour on, the conflicts have aged out: the chain drained.
    let drained = tick(&world, "15000");
    assert_eq!(dispatched(&drained), [3], "{drained}");
}

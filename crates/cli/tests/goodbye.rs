mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use common::World;
use serde_json::{Value, json};

/// The lease, and a day: what a heartbeat that holds nothing outlives on its
/// own host, and on another.
const LEASE: u64 = 4500;
const DAY: u64 = 86_400;

/// A `run` with nothing fixed: who it runs as, and when, is each test's.
const RUN: [&str; 5] = ["run", "--host", "h1", "--worker-command", "agent"];

/// Starts `run`, waits for its first tick to end — it sleeps, as a fleet that
/// is running does — and sends it `signal` → the lines it printed, and its
/// answer. It must then exit by itself, and cleanly.
fn stopped_by(world: &World, signal: &str, args: &[&str]) -> (Vec<String>, Value) {
    let mut run = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    let mut run = run.args(RUN).args(args).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines: Vec<String> = vec![];
    let mut stdout = BufReader::new(run.stdout.take().unwrap()).lines();
    for line in stdout.by_ref() {
        lines.push(line.unwrap());
        if lines.last().unwrap().starts_with("tick 1:") {
            break;
        }
    }
    let told = Command::new("kill").args([signal, &run.id().to_string()]).status().unwrap();
    assert!(told.success(), "the run was gone before it was told to stop: {lines:?}");
    lines.extend(stdout.map(|line| line.unwrap()));
    assert!(run.wait().unwrap().success(), "{lines:?}");
    let answer = serde_json::from_str(&lines.pop().unwrap()).unwrap();
    (lines, answer)
}

fn heartbeats(world: &World) -> Value {
    world.afk(&["scan"]).ok()["heartbeats"].clone()
}

fn beat(world: &World, instance: &str, host: &str, now: u64) {
    let now = now.to_string();
    let args = ["heartbeat", "--instance", instance, "--host", host, "--now", &now];
    assert_eq!(world.afk(&args).ok()["refreshed"], true);
}

fn start_at(world: &World, instance: &str, now: u64) -> Vec<String> {
    let now = now.to_string();
    let pacing = ["--instance", instance, "--second-ms", "0", "--ticks", "1", "--now", &now];
    world.afk_lines(&[&RUN[..], &pacing].concat()).0
}

#[test]
fn a_run_told_to_stop_while_it_holds_nothing_takes_its_heartbeat_with_it() {
    for signal in ["-TERM", "-INT"] {
        let world = World::new();
        // it held a claim once, and beat for it
        beat(&world, "alpha", "h1", 1000);
        let (lines, answer) = stopped_by(&world, signal, &["--instance", "alpha", "--now", "1060"]);
        assert_eq!(
            lines.last().unwrap(),
            "stop: holding no claim, it took its heartbeat with it",
            "{signal}: {lines:?}"
        );
        assert_eq!(answer, json!({"stopped": true, "ticks": 1, "instance": "alpha"}));
        assert_eq!(heartbeats(&world), json!({}), "{signal}");
    }
}

#[test]
fn a_run_told_to_stop_while_it_holds_a_claim_leaves_both_to_the_next_start() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let (lines, _) = stopped_by(&world, "-TERM", &["--instance", "alpha", "--now", "1000"]);
    assert!(lines[0].starts_with("tick 1: dispatched #7"), "{lines:?}");
    assert_eq!(
        lines.last().unwrap(),
        "stop: it left its heartbeat with the claims it holds: the next start on this host \
         takes them"
    );
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    assert_eq!(heartbeats(&world), json!({"alpha": 1000}));

    // a minute later, well within the lease: the start is the proof it is dead
    let lines = start_at(&world, "beta", 1060);
    assert_eq!(
        lines[0],
        "start: took #7 stamped with this host · swept 1 dead heartbeat(s) that held nothing"
    );
    assert_eq!(world.claims(), [(7, "beta".to_string())]);
    assert_eq!(heartbeats(&world), json!({"beta": 1060}));
}

#[test]
fn a_start_sweeps_its_hosts_heartbeats_past_the_lease_and_anyones_past_a_day() {
    let world = World::new();
    beat(&world, "gone", "h1", 1000);
    beat(&world, "peer", "h2", 1000);
    // exactly the lease old, a heartbeat is live to everyone
    let lines = start_at(&world, "alpha", 1000 + LEASE);
    assert!(!lines.iter().any(|line| line.starts_with("start:")), "{lines:?}");

    let later = 1001 + LEASE;
    beat(&world, "idle", "h1", later - 60);
    world.watch_pushes(None);
    let lines = start_at(&world, "beta", later);
    assert_eq!(lines[0], "start: swept 1 dead heartbeat(s) that held nothing");
    assert_eq!(world.pushes(), ["refs/convoy/heartbeat/gone"], "and nobody else's");
    assert_eq!(heartbeats(&world), json!({"idle": later - 60, "peer": 1000}));

    // a day on, whichever host stamped it
    let lines = start_at(&world, "gamma", 1001 + DAY);
    assert_eq!(lines[0], "start: swept 2 dead heartbeat(s) that held nothing");
    assert_eq!(heartbeats(&world), json!({}));
}

#[test]
fn ten_starts_and_stops_on_one_host_leave_at_most_one_heartbeat() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    for cycle in 1..=10 {
        // each a new fleet instance, named for the host by `run` itself
        let (lines, _) = stopped_by(&world, "-TERM", &[]);
        let beats = heartbeats(&world);
        assert!(beats.as_object().unwrap().len() <= 1, "cycle {cycle}: {beats} after {lines:?}");
        if cycle == 5 {
            // the work is done: the next run settles its claim, and holds nothing
            world.close(7, "completed");
        }
    }
    assert_eq!(heartbeats(&world), json!({}), "the last ones stopped holding nothing");
    assert_eq!(world.claims(), []);
}

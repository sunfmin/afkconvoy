//! Every command that changes fleet state logs a line in the tick log of its
//! host and repo as it acts, and wakes that repo's `run`, which logs who woke it.

mod common;

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::{Duration, Instant};

use afkconvoy::log::Entry;
use common::World;

/// Every line of this world's tick log, as `run` would print it.
fn logged(world: &World) -> Vec<String> {
    let runs = world.cache().join("afkconvoy/runs");
    let Ok(files) = std::fs::read_dir(&runs) else { return vec![] };
    let files = files.map(|entry| entry.unwrap().path());
    let mut files: Vec<_> = files.filter(|file| file.extension().unwrap() == "jsonl").collect();
    assert!(files.len() <= 1, "one log for the one repo: {files:?}");
    let Some(file) = files.pop() else { return vec![] };
    let text = std::fs::read_to_string(file).unwrap();
    let entry = |line: &str| serde_json::from_str::<Entry>(line).unwrap().said();
    text.lines().map(entry).collect()
}

fn afk(world: &World, args: &[&str]) -> common::Answer {
    world.afk(&[args, &["--now", "1100"]].concat())
}

#[test]
fn park_escalate_and_takeover_by_hand_each_log_who_did_what_on_which_issue() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    world.issue(9, &["ready-for-agent"]);
    world.issue(41, &["bug"]);
    world.pr(410, 41, "issue-41");
    let alpha = ["--instance", "alpha", "--host", "h1"];
    afk(&world, &[&["claim", "7"][..], &alpha].concat()).ok();
    world.comment(7, "<!--convoy:verdict phase=blocked blocked_by=41-->\nwhy");
    afk(&world, &[&["claim", "8"][..], &alpha].concat()).ok();
    afk(&world, &["claim", "9", "--instance", "beta", "--host", "h2"]).ok();
    let before = logged(&world).len();

    // by hand, with no `run` to wake: each acts all the same
    let parked = afk(&world, &[&["park", "7"][..], &alpha].concat()).ok();
    assert_eq!(parked["woke"], false, "{parked}");
    let escalated = afk(&world, &[&["escalate", "8", "--reason", "x"][..], &alpha].concat()).ok();
    assert_eq!(escalated["woke"], false, "{escalated}");
    let took = afk(&world, &[&["takeover", "--from", "beta", "--yes"][..], &alpha].concat()).ok();
    assert_eq!(took["woke"], false, "{took}");
    let refused = afk(&world, &[&["park", "9"][..], &alpha].concat()).error();
    assert_eq!(
        logged(&world)[before..],
        [
            "park #7 (operator): blocked by #41".to_string(),
            "escalate #8 (operator): escalated".to_string(),
            "takeover from beta (operator): taken: took [#9], lost []".to_string(),
            format!("park #9 (operator): FAILED: {refused}"),
        ]
    );

    // what only reads writes no line
    let lines = logged(&world).len();
    afk(&world, &["plan"]).ok();
    afk(&world, &["scan"]).ok();
    afk(&world, &["recovery", "7"]).ok();
    afk(&world, &["takeover", "--list"]).ok();
    assert_eq!(logged(&world).len(), lines);
    // the claims before: each a line of its own
    let claims = &logged(&world)[..before];
    assert_eq!(
        claims,
        ["claim #7 (operator): won", "claim #8 (operator): won", "claim #9 (operator): won"]
    );
}

#[test]
fn a_worker_s_command_says_it_was_a_worker_s() {
    let world = World::new();
    world.issue(12, &[]);
    let answer = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["wake", "--now", "1100"])
        .env("AFKCONVOY_ISSUE", "12")
        .output()
        .unwrap();
    assert!(answer.status.success());
    assert_eq!(logged(&world), ["wake #12 (worker): done"]);
}

#[test]
fn the_run_logs_the_command_that_woke_it() {
    let world = World::new();
    let mut child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["run", "--instance", "alpha", "--host", "h1", "--ticks", "2"])
        .args(["--worker-command", "agent"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok);
    let first = lines.find(|line| line.starts_with("tick 1: ") && line.contains("next in"));
    assert!(first.is_some_and(|first| first.ends_with("next in 90s")));
    world.issue(9, &["bug"]);
    let started = Instant::now();
    afk(&world, &["claim", "9", "--instance", "beta", "--host", "h2"]).ok();
    let woken = lines.find(|line| line.starts_with("tick 2: started"));
    assert_eq!(woken.as_deref(), Some("tick 2: started (woken by claim #9)"));
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    let _ = child.kill();
    let _ = child.wait();
    // the command's line and the run's, in the one log
    let logged = logged(&world);
    let claim = logged.iter().position(|line| line == "claim #9 (operator): won");
    let tick = logged.iter().position(|line| line == "tick 2: started (woken by claim #9)");
    assert!(claim.is_some() && claim < tick, "{logged:#?}");
}

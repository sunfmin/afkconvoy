mod common;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use common::{World, git};
use serde_json::json;

/// How long anything a wake should bring about is waited for. The sleep it
/// ends is far longer (`LONG`), so "within this" means "woken".
const SOON: Duration = Duration::from_secs(20);

/// A `run` of two ticks whose sleep between them is real: 90 seconds at the
/// least. Its stdout comes line by line.
struct Running {
    child: Child,
    lines: Receiver<String>,
}

fn run(world: &World) -> Running {
    let mut child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["run", "--instance", "alpha", "--host", "h1", "--ticks", "2"])
        .args(["--worker-command", "agent"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let out = child.stdout.take().unwrap();
    let (send, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    Running { child, lines }
}

impl Running {
    /// The next line `run` prints, if one comes within `within`.
    fn line(&self, within: Duration) -> Option<String> {
        self.lines.recv_timeout(within).ok()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wake(world: &World, dir: &Path) -> serde_json::Value {
    world.afk_in(dir, &["wake"]).ok()
}

fn until(what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < SOON, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_wake_from_a_worktree_opens_the_next_tick_now() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let running = run(&world);
    let first = running.line(SOON).expect("the first tick");
    assert!(first.starts_with("tick 1: dispatched #7"), "{first}");
    assert!(first.ends_with("next in 90s"), "{first}");
    assert_eq!(running.line(Duration::from_millis(300)), None, "it sleeps");

    // the worker's worktree, not the fleet's checkout: the repo is what is woken
    let worktree = world.orca()["worktrees"][0]["path"].as_str().unwrap().to_string();
    let started = Instant::now();
    assert_eq!(wake(&world, Path::new(&worktree)), json!({"woke": true}));

    let second = running.line(SOON).expect("the wake opens the second tick");
    assert!(second.starts_with("tick 2: "), "{second}");
    assert!(started.elapsed() < SOON);
    let stopped = running.line(SOON).expect("the answer");
    assert!(stopped.contains("\"ticks\":2"), "{stopped}");
}

#[test]
fn the_tick_a_wake_opens_reads_github_like_any_other() {
    let world = World::new();
    let running = run(&world);
    let first = running.line(SOON).expect("the first tick");
    assert!(first.starts_with("tick 1: nothing to do"), "{first}");

    // what changed meanwhile is not in the wake: the tick finds it on GitHub
    world.issue(9, &["ready-for-agent"]);
    assert_eq!(wake(&world, &world.clone), json!({"woke": true}));
    let second = running.line(SOON).expect("the second tick");
    assert!(second.starts_with("tick 2: dispatched #9"), "{second}");
}

#[test]
fn a_wake_that_arrives_during_a_tick_makes_the_next_start_at_once() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    // every GitHub call takes a while, so the first tick does
    world.with_gh(|gh| gh["slow_ms"] = json!(150));
    let running = run(&world);

    // the tick is reading: its list of the open issues was asked for
    until("the first tick's rebuild", || {
        let calls = world.gh()["calls"].to_string();
        calls.contains("graphql")
    });
    assert_eq!(wake(&world, &world.clone), json!({"woke": true}));
    assert_eq!(wake(&world, &world.clone), json!({"woke": true}), "and a second, as well");

    let first = running.line(SOON).expect("the first tick");
    assert!(first.starts_with("tick 1: dispatched #7"), "it ended after the wake came: {first}");
    assert!(first.ends_with("next in 90s"), "{first}");
    let second = running.line(SOON).expect("no sleep: the wake was waiting");
    assert!(second.starts_with("tick 2: "), "{second}");
}

#[test]
fn with_no_run_to_wake_a_wake_succeeds_and_does_nothing() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let refs = world.remote(&["for-each-ref"]);
    let status = git(&world.clone, &["status", "--porcelain", "--ignored"]);
    world.gh_calls();
    world.watch_pushes(None);

    let answer = wake(&world, &world.clone);
    assert_eq!(answer["woke"], false);
    assert!(answer["detail"].as_str().unwrap().contains("no afkconvoy run is listening"));

    assert!(!world.clone.join(".git/convoy-wake.sock").exists(), "it left nothing");
    assert_eq!(world.remote(&["for-each-ref"]), refs);
    assert_eq!(git(&world.clone, &["status", "--porcelain", "--ignored"]), status);
    assert_eq!(world.gh_calls(), [] as [Vec<String>; 0]);
    assert_eq!(world.orca_calls(), [] as [Vec<String>; 0]);
    assert_eq!(world.pushes(), [] as [String; 0]);
}

#[test]
fn a_wake_after_the_run_ended_is_lost_and_costs_nothing() {
    let world = World::new();
    // one tick, and the run is over
    let ran = world.afk_lines(&[
        "run",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--ticks",
        "1",
        "--worker-command",
        "agent",
    ]);
    assert_eq!(ran.1["ticks"], 1);
    assert_eq!(wake(&world, &world.clone)["woke"], false);

    // …and one sent before a run starts is not kept for it: the run sleeps
    let running = run(&world);
    let first = running.line(SOON).expect("the first tick");
    assert!(first.starts_with("tick 1: "), "{first}");
    assert_eq!(running.line(Duration::from_millis(500)), None, "no early second tick");
}

#[test]
fn the_worker_brief_and_the_join_brief_carry_the_wake_line() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let who = ["--instance", "alpha", "--host", "h1", "--now", "1000", "--worker-command", "agent"];
    let dispatch = |issue: &str| {
        let mut args = vec!["dispatch", "--issue", issue];
        args.extend(who);
        world.afk(&args).ok()
    };
    let brief = |started: &serde_json::Value| {
        let dir = started["worktree"].as_str().unwrap().to_string();
        let git_dir = git(Path::new(&dir), &["rev-parse", "--absolute-git-dir"]);
        std::fs::read_to_string(Path::new(&git_dir).join("convoy-worker-prompt.md")).unwrap()
    };
    let line = format!("\n{} wake\n", env!("CARGO_BIN_EXE_afkconvoy"));

    let whole = brief(&dispatch("7"));
    assert!(whole.contains("2. **Implement**"), "the whole brief");
    assert!(whole.contains(&line), "{whole}");
    assert!(whole.contains("or your comment is posted"));

    // its PR opened, the issue is dispatched again: the join brief
    let dir = world.orca()["worktrees"][0]["path"].as_str().unwrap().to_string();
    common::commit(Path::new(&dir), "seven.txt", "7\n");
    git(Path::new(&dir), &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(Path::new(&dir), &["symbolic-ref", "--short", "HEAD"]));
    let joining = brief(&dispatch("7"));
    assert!(joining.contains("Your ONLY job is to get that PR on the train"), "{joining}");
    assert!(joining.contains(&line), "{joining}");
    assert!(!joining.contains("your comment is posted"), "a joiner posts no verdict");
}

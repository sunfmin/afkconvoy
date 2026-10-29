use crate::common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{Running, World, git};
use serde_json::json;

/// How long anything a wake should bring about is waited for. The sleep it
/// ends is far longer (`LONG`), so "within this" means "woken".
const SOON: Duration = Duration::from_secs(20);

/// A `run` of two ticks whose sleep between them is real: 90 seconds at the
/// least.
fn run(world: &World) -> Running {
    world.running(&["--instance", "alpha", "--host", "h1", "--ticks", "2"])
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
    assert_eq!(wake(&world, Path::new(&worktree)), json!({"woke": true, "tick": 2}));

    // said before the tick does anything, within a second of the wake
    let opened = running.every_line(SOON).expect("the wake opens the second tick");
    assert_eq!(opened, "tick 2: started (woken by wake)");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    let second = running.line(SOON).expect("the second tick ends");
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
    assert_eq!(wake(&world, &world.clone), json!({"woke": true, "tick": 2}));
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
    let woken = Instant::now();
    assert_eq!(wake(&world, &world.clone), json!({"woke": true, "after_tick": 1}));
    let again = json!({"woke": true, "after_tick": 1});
    assert_eq!(wake(&world, &world.clone), again, "and a second, as well");

    // said as it is heard, while the tick goes on — once
    let heard = running.line(SOON).expect("the wake heard");
    assert_eq!(heard, "woken by wake during tick 1: tick 2 follows");
    assert!(
        woken.elapsed() < Duration::from_secs(1),
        "said within a second: {:?}",
        woken.elapsed()
    );
    let first = running.line(SOON).expect("the first tick");
    assert!(first.starts_with("tick 1: dispatched #7"), "it ended after the wake came: {first}");
    assert!(first.ends_with("next in 90s"), "{first}");
    let started = running.every_line(SOON).expect("no sleep: the wake was waiting");
    assert_eq!(started, "tick 2: started (woken by wake)", "it starts as the first ends");
    let second = running.line(SOON).expect("the second tick");
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
    let detail = answer["detail"].as_str().unwrap();
    assert!(detail.starts_with("no host runner runs on this host"), "{detail}");
    let fix = format!("`afkconvoy run {}`", world.clone.display());
    assert!(detail.contains(&fix), "with the fix: {detail}");

    assert!(!world.cache().join("afkconvoy/host-runner.sock").exists(), "it left nothing");
    assert!(!world.clone.join(".git/convoy-wake.sock").exists(), "no socket of the repo's");
    assert_eq!(world.remote(&["for-each-ref"]), refs);
    assert_eq!(git(&world.clone, &["status", "--porcelain", "--ignored"]), status);
    assert_eq!(world.gh_calls(), [] as [Vec<String>; 0]);
    assert_eq!(world.orca_calls(), [] as [Vec<String>; 0]);
    assert_eq!(world.pushes(), [] as [String; 0]);
}

#[test]
fn a_wake_outside_a_git_checkout_is_the_error_doctor_answers_there() {
    let world = World::new();
    let elsewhere = world.home();
    let doctor = world.afk_in(&elsewhere, &["doctor"]).error();
    assert!(doctor.ends_with("is not in a git checkout"), "{doctor}");

    let answer = world.afk_in(&elsewhere, &["wake"]);
    assert!(answer.stderr.is_empty(), "nothing on stderr: {}", answer.stderr);
    assert_eq!(answer.error(), doctor);
    // no checkout, so no tick log to say it in
    assert!(!world.cache().join("afkconvoy/runs").exists(), "it logged nothing");
}

#[test]
fn a_run_that_does_not_answer_costs_the_wake_its_wait_and_no_more() {
    let world = World::new();
    // something listens where the host runner would, and answers nothing:
    // named from inside the cache directory, as the host runner names it,
    // whose path may be too long for a socket's. Each test is a process of
    // its own under nextest.
    let cache = world.cache().join("afkconvoy");
    std::fs::create_dir_all(&cache).unwrap();
    std::env::set_current_dir(&cache).unwrap();
    let mute = std::os::unix::net::UnixListener::bind("host-runner.sock").unwrap();

    let started = Instant::now();
    let answer = wake(&world, &world.clone);
    let took = started.elapsed();
    assert_eq!(answer["woke"], false, "{answer}");
    let detail = answer["detail"].as_str().unwrap();
    assert!(detail.contains("did not answer within 3s"), "{detail}");
    assert!(detail.contains("`afkconvoy doctor`"), "with the fix: {detail}");
    assert!(took >= Duration::from_secs(3) && took < Duration::from_secs(10), "{took:?}");
    // it was asked all the same, naming the checkout
    let (asked, _) = mute.accept().unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(asked), &mut line).unwrap();
    let asked: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(asked, json!({"op": "wake", "checkout": world.clone, "by": "wake"}), "{line}");
}

#[test]
fn a_wake_after_the_run_ended_is_lost_and_costs_nothing() {
    let world = World::new();
    // one tick, and the run is over
    let ran = world.afk_lines(&[
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--ticks",
        "1",
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
    let who = ["--instance", "alpha", "--host", "h1", "--now", "1000"];
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
    assert!(joining.contains("Your ONLY job is to land that PR in a turn"), "{joining}");
    assert!(joining.contains(&line), "{joining}");
    assert!(!joining.contains("your comment is posted"), "a joiner posts no verdict");
}

/// What the tick log `answer` names said, line by line.
fn tick_log(answer: &serde_json::Value) -> Vec<String> {
    let text = std::fs::read_to_string(answer["tick_log"].as_str().unwrap()).unwrap();
    let entry = |line: &str| serde_json::from_str::<afkconvoy::log::Entry>(line).unwrap().said();
    text.lines().map(entry).collect()
}

#[test]
fn a_wake_ends_the_sleep_of_its_checkouts_fleet_instance_alone() {
    let world = World::new();
    // one host runner holds both, each asleep for 90 seconds at the least
    let first = world.afk(&["run", "--host", "h1"]).ok();
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &["run", "--host", "h1"]).ok();
    assert_eq!(second["host_runner"]["pid"], first["host_runner"]["pid"], "{second}");

    let started = Instant::now();
    assert_eq!(wake(&world, &clone2), json!({"woke": true, "tick": 2}));
    let woken = "tick 2: started (woken by wake)".to_string();
    until("the second tick of the woken", || tick_log(&second).contains(&woken));
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    std::thread::sleep(Duration::from_millis(500));
    let other = tick_log(&first);
    assert!(!other.iter().any(|line| line.starts_with("tick 2:")), "{other:#?}");
}

/// Two repos' fleets on one host and tracker. The second has reached its
/// 1500s idle sleep when the first closes #7 or reads it closed.
fn closed_blocker(closes: bool, through_edge: bool) {
    let world = World::new();
    world.name_origin(&world.clone, "acme/dev");
    world.put_tracker("acme/tracker");
    let second = world.second_repo();
    let url = "https://github.com/acme/other.git";
    git(&second, &["config", "remote.origin.url", url]);
    let rewrite = format!("url.{}.insteadOf", world.root.join("remote2.git").display());
    git(&second, &["config", &rewrite, url]);
    let mut gh2 = world.gh2();
    gh2["trackers"] = world.gh()["trackers"].clone();
    std::fs::write(world.root.join("gh2.json"), gh2.to_string()).unwrap();
    world.with_tracker(|gh| {
        gh["issues"] = json!([
            {"number": 7, "title": "blocker", "state": "open",
             "labels": if through_edge { vec!["repo:acme/dev"] }
                       else { vec!["repo:acme/dev", "ready-for-agent"] }},
            {"number": 9, "title": "unblocked next", "state": "open",
             "labels": ["repo:acme/other", "ready-for-agent"]},
            {"number": 10, "title": "still blocked", "state": "open",
             "labels": ["repo:acme/other", "ready-for-agent"]},
            {"number": 11, "title": "another blocker", "state": "open", "labels": []},
            {"number": 13, "title": "older closure", "state": "closed", "labels": []}
        ]);
        gh["edges"] = json!({"9": [7, 13], "10": [7, 11]});
        if through_edge {
            gh["issues"].as_array_mut().unwrap().push(json!({
                "number": 8, "title": "observer", "state": "open",
                "labels": ["repo:acme/dev"]
            }));
            gh["edges"]["8"] = json!([7]);
        }
    });
    // A declared verdict is acted on only past every sign of the worker's
    // life, including its files' mtimes. Keep that grace out of this test.
    let now =
        (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
            + 600)
            .to_string();
    let args = ["run", "--host", "h1", "--tracker", "acme/tracker", "--now", &now];
    let source = world.afk(&args).ok();
    let target = world.afk_in(&second, &args).ok();
    for tick in [2, 3] {
        wake(&world, &second);
        until("an empty tick of the blocked fleet", || {
            tick_log(&target).iter().any(|line| line.starts_with(&format!("tick {tick}: nothing")))
        });
    }
    assert!(tick_log(&target).iter().any(|line| line.ends_with("next in 1500s")));
    if closes {
        world.with_tracker(|gh| {
            gh["comments"]["7"].as_array_mut().unwrap().push(json!({
                "id": 90001, "body": "<!--convoy:verdict phase=already-satisfied-->",
                "html_url": "https://gh/i/7#90001"
            }));
        });
        world.with_orca(|orca| {
            let terminal = &mut orca["terminals"][0];
            terminal["agent"] = json!({"state": "idle", "stateStartedAt": 1});
            terminal["lastOutputAt"] = json!(1);
        });
    } else {
        world.with_tracker(|gh| gh["issues"][0]["state"] = json!("closed"));
    }
    let started = Instant::now();
    wake(&world, &world.clone);
    while !tick_log(&target).iter().any(|line| line.starts_with("tick 4: dispatched #9")) {
        assert!(
            started.elapsed() < SOON,
            "the blocked fleet did not dispatch: source {:#?}, target {:#?}, blocker {}",
            tick_log(&source),
            tick_log(&target),
            world.tracker()["issues"][0]
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(started.elapsed() < SOON, "it did not wait for the idle timer");
    let by = format!(
        "tick 4: started (woken by acme/tracker#7 closed, seen by fleet instance {})",
        source["instance"].as_str().unwrap()
    );
    let lines = tick_log(&target);
    assert!(lines.contains(&by), "{lines:#?}");
    assert!(!lines.iter().any(|line| line.contains("dispatched #10")), "{lines:#?}");
    assert_eq!(world.tracker()["issues"][0]["state"], "closed");
}

#[test]
fn closing_an_issue_wakes_the_other_repos_fleet_and_it_dispatches_without_its_idle_wait() {
    closed_blocker(true, false);
}

#[test]
fn reading_a_claimed_issue_closed_wakes_the_other_repos_blocked_fleet() {
    closed_blocker(false, false);
}

#[test]
fn reading_a_closed_dependency_edge_wakes_the_other_repos_blocked_fleet() {
    closed_blocker(false, true);
}

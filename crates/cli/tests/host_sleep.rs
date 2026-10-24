use crate::common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::{World, bookkeeping};
use serde_json::{Value, json};

/// A world whose fleet worked: alpha of h1 held a claim on #7, which is now
/// closed. Its next start takes the claim and its first tick settles it.
fn worked() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let claim = ["claim", "--issue", "7", "--instance", "alpha", "--host", "h1"];
    assert_eq!(world.afk(&claim).ok()["won"], true);
    world.close_issue(7);
    world
}

/// Runs alpha of h1 for `ticks` ticks, putting the host to sleep once every
/// fleet on it has been idle no time at all → the lines it printed, but for
/// those it prints whatever it does (`bookkeeping`).
fn run(world: &World, ticks: &str) -> Vec<String> {
    run_with(world, ticks, |_| {})
}

/// The same, with what `more` adds to its command.
fn run_with(world: &World, ticks: &str, more: impl Fn(&mut Command)) -> Vec<String> {
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    world.host_sleeps(&mut command);
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1"];
    let pacing = ["--second-ms", "0", "--ticks", ticks, "--host-sleep-after", "0"];
    command.args(args).args(pacing);
    more(&mut command);
    let out = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    stdout.lines().filter(|line| !bookkeeping(line)).map(str::to_string).collect()
}

/// What `doctor` says of the last check for host sleep.
fn last_check(world: &World) -> Value {
    let answer = world.afk(&["doctor", "--host", "h1"]);
    answer.json["fleet"]["host_sleep"].clone()
}

/// Has the world's one host runner hold the checkout `dir` as `run` with
/// `args` asks, putting the host to sleep unless told otherwise → what that
/// `run` answered, once the first tick of its fleet instance ended.
fn hold(world: &World, dir: &Path, args: &[&str]) -> Value {
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), dir);
    world.host_sleeps(&mut command);
    let out = command.args(["run", "--host", "h1"]).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(stdout.lines().last().unwrap()).unwrap()
}

/// Another fleet on the host, beta, of the world's second repo, which the
/// host runner holds beside alpha: its `run` told `--no-host-sleep`, which
/// the host then says, when `told`; with these issues ready on its repo →
/// what its `run` answered.
fn beta(world: &World, told: bool, issues: &[u64]) -> Value {
    let clone2 = world.second_repo();
    with_gh2(world, |gh| {
        for number in issues {
            let issue = json!({"number": number, "title": format!("issue {number}"),
                "labels": ["ready-for-agent"], "state": "open"});
            gh["issues"].as_array_mut().unwrap().push(issue);
        }
    });
    let mut args = vec!["--instance", "beta"];
    if told {
        args.push("--no-host-sleep");
    }
    hold(world, &clone2, &args)
}

/// Changes the second repo's stand-in GitHub in place.
fn with_gh2(world: &World, change: impl FnOnce(&mut Value)) {
    let mut state = world.gh2();
    change(&mut state);
    std::fs::write(world.root.join("gh2.json"), state.to_string()).unwrap();
}

/// Alpha of h1, held by the world's host runner beside beta, checking for
/// host sleep after each tick, `pacing` its ticks → what its `run` answered.
fn alpha(world: &World, pacing: &[&str]) -> Value {
    let args = [&["--instance", "alpha", "--second-ms", "0", "--host-sleep-after", "0"], pacing];
    hold(world, &world.clone, &args.concat())
}

/// Waits up to 30 seconds for alpha's fleet instance to end.
fn ended(world: &World) {
    let ended = format!("fleet instance ended: {}: ", world.clone.display());
    let until = Instant::now() + Duration::from_secs(30);
    while !world.runner_log().iter().any(|line| line.starts_with(&ended)) {
        assert!(Instant::now() < until, "alpha never ended: {:#?}", world.runner_log());
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the tick log `answer` names said, line by line.
fn tick_log(answer: &Value) -> Vec<String> {
    let text = std::fs::read_to_string(answer["tick_log"].as_str().unwrap()).unwrap();
    let entry = |line: &str| serde_json::from_str::<afkconvoy::log::Entry>(line).unwrap().said();
    text.lines().map(entry).collect()
}

#[test]
fn a_fleet_that_worked_puts_its_host_to_sleep_once_every_fleet_on_it_is_idle() {
    let world = worked();
    let lines = run(&world, "2");
    assert_eq!(
        lines[0],
        "start: puts this host to sleep within its sleep hours, 00:00-24:00, once every fleet on \
         it has been idle 0s (`--host-sleep-hours default` changes them back to 00:00-06:00, \
         `--no-host-sleep` keeps it awake)"
    );
    let slept: Vec<&String> = lines.iter().filter(|line| line.starts_with("host sleep:")).collect();
    assert_eq!(slept.len(), 1, "{lines:?}");
    assert!(slept[0].ends_with("put the host to sleep"), "{lines:?}");
    assert!(!lines.iter().any(|line| line.starts_with("host woke:")), "no sleep to notice");
    // it ticked once more first: the line comes after the second tick
    let at = |prefix: &str| lines.iter().position(|line| line.starts_with(prefix)).unwrap();
    assert!(at("tick 2:") < at("host sleep:"), "{lines:?}");
    assert_eq!(world.pmset_calls(), ["sleepnow"]);
    let checked = last_check(&world);
    assert_eq!(checked["slept"], true, "{checked}");
    // it made no socket of its own, and the host runner's goes as it exits
    let cache = world.cache().join("afkconvoy");
    assert!(!cache.join("instances").exists(), "no socket of a fleet instance's");
    assert!(!cache.join("host-runner.sock").exists(), "it removes its socket as it exits");
}
#[test]
fn a_host_woken_since_the_fleet_last_worked_is_not_put_back_to_sleep() {
    let world = World::new();
    // it never held a claim: nothing it did keeps a host woken by hand awake
    let lines = run(&world, "2");
    // a check that kept the host awake is a line, as in the log
    let kept = "host sleep: kept awake: this instance has not worked since the host last woke";
    assert!(lines.iter().any(|line| line == kept), "{lines:?}");
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let checked = last_check(&world);
    assert_eq!(checked["slept"], false, "{checked}");
    assert_eq!(
        checked["kept_awake_by"][0],
        "this instance has not worked since the host last woke"
    );

    // the host woke after it last worked
    let world = worked();
    std::fs::write(world.root.join("host-woke"), "99999999999").unwrap();
    run(&world, "2");
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let checked = last_check(&world);
    assert_eq!(
        checked["kept_awake_by"][0],
        "this instance has not worked since the host last woke"
    );
}

#[test]
fn the_keyboard_or_mouse_used_lately_keeps_the_host_awake() {
    let world = worked();
    std::fs::write(world.root.join("host-untouched"), "30").unwrap();
    run(&world, "2");
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let checked = last_check(&world);
    assert_eq!(checked["kept_awake_by"][0], "the keyboard or mouse was used 30s ago");
}

#[test]
fn an_instance_idle_less_than_ten_minutes_makes_no_check() {
    let world = worked();
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    world.host_sleeps(&mut command);
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1"];
    let pacing = ["--second-ms", "0", "--ticks", "3", "--host-sleep-hours", "default"];
    let out = command.args(args).args(pacing).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let host_sleep = "start: puts this host to sleep within its sleep hours, 00:00-06:00, once \
                      every fleet on it has been idle ten minutes (`--host-sleep-hours` says \
                      others, `--no-host-sleep` keeps it awake)";
    let lines: Vec<&str> = stdout.lines().filter(|line| !bookkeeping(line)).collect();
    assert_eq!(lines[0], host_sleep, "{stdout}");
    assert!(!stdout.contains("host sleep:"), "{stdout}");
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    assert_eq!(last_check(&world), Value::Null);
}

/// The sleep hours that start `from` minutes after now and last `long`, in
/// the time zone `ahead` hours ahead of UTC, as `--host-sleep-hours` says them.
fn hours(ahead: u64, from: u64, long: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let local = (now / 60 + ahead * 60) % 1440;
    let said = |minute: u64| format!("{:02}:{:02}", minute % 1440 / 60, minute % 60);
    format!("{}-{}", said(local + from), said(local + from + long))
}

#[test]
fn a_check_outside_the_sleep_hours_keeps_the_host_awake_and_says_them() {
    let world = worked();
    // in two hours, for an hour, in the host's local time
    let outside = hours(9, 120, 60);
    let lines = run_with(&world, "2", |command| {
        command.env("TZ", "JST-9").args(["--host-sleep-hours", &outside]);
    });
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let said = format!("the host's local time is outside its sleep hours, {outside}");
    let kept = format!("host sleep: kept awake: {said}");
    assert!(lines.contains(&kept), "{lines:?}");
    let start = format!("start: puts this host to sleep within its sleep hours, {outside}, ");
    assert!(lines[0].starts_with(&start), "{lines:?}");
    let checked = last_check(&world);
    assert_eq!(checked["slept"], false, "{checked}");
    assert_eq!(checked["kept_awake_by"], json!([said]), "{checked}");
    // the next check is as they start: none more in this run
    let checks = lines.iter().filter(|line| line.starts_with("host sleep:")).count();
    assert_eq!(checks, 1, "{lines:?}");
}

#[test]
fn the_sleep_hours_are_of_the_host_s_local_time() {
    // from an hour ago to an hour on in UTC+9, which UTC is nine hours behind
    let within = hours(9, 1440 - 60, 120);
    let world = worked();
    run_with(&world, "2", |command| {
        command.env("TZ", "JST-9").args(["--host-sleep-hours", &within]);
    });
    assert_eq!(world.pmset_calls(), ["sleepnow"]);
    assert_eq!(last_check(&world)["slept"], true);

    let world = worked();
    run_with(&world, "2", |command| {
        command.env("TZ", "UTC0").args(["--host-sleep-hours", &within]);
    });
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let said = format!("the host's local time is outside its sleep hours, {within}");
    assert_eq!(last_check(&world)["kept_awake_by"][0], said);
}

#[test]
fn a_run_told_no_host_sleep_keeps_the_host_awake_for_every_fleet_on_it() {
    let world = worked();
    // beta's `run` says it of the host: alpha, which says nothing, checks never
    beta(&world, true, &[]);
    let alpha = alpha(&world, &["--ticks", "2"]);
    ended(&world);
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    assert_eq!(last_check(&world), Value::Null);
    let lines = tick_log(&alpha);
    assert!(!lines.iter().any(|line| line.starts_with("host sleep:")), "{lines:#?}");
}

#[test]
fn a_busy_peer_keeps_the_host_awake() {
    // beta has an issue to work: its first tick claims it
    let world = worked();
    let beta = beta(&world, false, &[9]);
    assert!(beta["first_tick"].as_str().unwrap().contains("dispatched #9"), "{beta}");
    alpha(&world, &["--ticks", "2"]);
    ended(&world);
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let checked = last_check(&world);
    assert_eq!(checked["kept_awake_by"][0], "beta is not idle", "{checked}");
}

#[test]
fn an_idle_peer_is_asked_to_tick_and_the_host_sleeps_once_it_did() {
    let world = worked();
    // idle, and asleep for 90 seconds at the least: its first tick ended
    let beta = beta(&world, false, &[]);
    alpha(&world, &["--ticks", "2"]);
    ended(&world);
    // beta has not worked since the host woke, which keeps nothing awake: only
    // the instance that puts the host to sleep must have
    let checked = last_check(&world);
    assert_eq!(checked["slept"], true, "{checked}");
    assert_eq!(world.pmset_calls(), ["sleepnow"]);
    // asked to tick first, it was woken to, at once
    let lines = tick_log(&beta);
    assert!(lines.contains(&"tick 2: started (woken)".to_string()), "{lines:#?}");
}

#[test]
fn a_fleet_instance_told_to_stop_while_a_check_waits_for_the_ticks_stops_at_once() {
    let world = worked();
    let beta = beta(&world, false, &[]);
    // asked to tick, beta takes far longer than this test: every read of its
    // GitHub takes half a minute
    with_gh2(&world, |gh| gh["slow_ms"] = json!(30_000));
    let alpha = alpha(&world, &[]);
    // the check asked beta to tick, and alpha's own second tick has ended
    let until = Instant::now() + Duration::from_secs(30);
    let ticked = |line: &String| line.starts_with("tick 2: ") && !bookkeeping(line);
    while !tick_log(&alpha).iter().any(ticked) {
        assert!(Instant::now() < until, "{:#?}", tick_log(&alpha));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(tick_log(&beta).contains(&"tick 2: started (woken)".to_string()));

    let told = Instant::now();
    let stopped = world.afk(&["stop"]).ok();
    assert_eq!(stopped["removed"], true, "{stopped}");
    assert!(told.elapsed() < Duration::from_secs(5), "it waited {:?}", told.elapsed());
    let lines = tick_log(&alpha);
    assert!(lines.last().unwrap().starts_with("stop:"), "{lines:#?}");
    assert_eq!(
        world.pmset_calls(),
        Vec::<String>::new(),
        "a fleet instance told to stop puts nothing to sleep"
    );
}

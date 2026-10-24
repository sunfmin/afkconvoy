use crate::common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::{World, bookkeeping};
use serde_json::Value;

/// The directory every `run` of a world's host listens in.
fn instances(world: &World) -> PathBuf {
    world.cache().join("afkconvoy/instances")
}

/// Listens as `name` where every `run` of `world`'s host does, as a `run`
/// would: named from inside the directory, whose path is too long for a
/// socket's. Each test is a process of its own under nextest.
fn bind(world: &World, name: &str) -> UnixListener {
    std::fs::create_dir_all(instances(world)).unwrap();
    std::env::set_current_dir(instances(world)).unwrap();
    UnixListener::bind(name).unwrap()
}

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

/// Another fleet on the same host, of a repo of its own, running until it is
/// dropped: told `--no-host-sleep` when `told`.
struct Peer {
    _world: World,
    child: Child,
}

impl Peer {
    fn start(world: &World, told: bool) -> Peer {
        Peer::start_with(world, told, &[])
    }

    /// The same, with these issues ready on its repo.
    fn start_with(world: &World, told: bool, issues: &[u64]) -> Peer {
        let peer = World::new();
        for issue in issues {
            peer.issue(*issue, &["ready-for-agent"]);
        }
        let mut command = peer.command(env!("CARGO_BIN_EXE_afkconvoy"), &peer.clone);
        peer.host_sleeps(&mut command);
        command.env("XDG_CACHE_HOME", world.cache());
        let args = ["run", "--foreground", "--instance", "beta", "--host", "h1"];
        command.args(args);
        if told {
            command.arg("--no-host-sleep");
        }
        let child = command.stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let socket = instances(world).join("beta.sock");
        let started = std::time::Instant::now();
        while !socket.exists() {
            assert!(started.elapsed().as_secs() < 20, "beta never listened");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Peer { _world: peer, child }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
    assert!(!instances(&world).join("alpha.sock").exists(), "it removes its socket as it exits");
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
    assert!(lines.iter().any(|line| *line == kept), "{lines:?}");
    let start = format!("start: puts this host to sleep within its sleep hours, {outside}, ");
    assert!(lines[0].starts_with(&start), "{lines:?}");
    let checked = last_check(&world);
    assert_eq!(checked["slept"], false, "{checked}");
    assert_eq!(checked["kept_awake_by"], serde_json::json!([said]), "{checked}");
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
fn a_fleet_told_no_host_sleep_keeps_the_host_awake_for_every_fleet_on_it() {
    let world = worked();
    let peer = Peer::start(&world, true);
    run(&world, "2");
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let checked = last_check(&world);
    assert_eq!(checked["kept_awake_by"][0], "beta was told --no-host-sleep", "{checked}");
    drop(peer);
}

#[test]
fn a_busy_peer_keeps_the_host_awake_and_one_that_never_answers_too() {
    // beta has an issue to work: its first tick claims it
    let world = worked();
    let peer = Peer::start_with(&world, false, &[9]);
    std::thread::sleep(std::time::Duration::from_secs(3));
    run(&world, "2");
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let checked = last_check(&world);
    assert_eq!(checked["kept_awake_by"][0], "beta is not idle", "{checked}");
    drop(peer);

    // something listens as gamma, and never answers
    let world = worked();
    let _mute = bind(&world, "gamma.sock");
    run(&world, "2");
    assert_eq!(world.pmset_calls(), Vec::<String>::new());
    let checked = last_check(&world);
    assert_eq!(checked["kept_awake_by"][0], "gamma did not answer in time", "{checked}");
}

#[test]
fn an_idle_peer_is_asked_to_tick_and_a_socket_nobody_listens_on_is_removed() {
    let world = worked();
    // what a run killed with no goodbye left
    drop(bind(&world, "ghost.sock"));
    let peer = Peer::start(&world, false);
    // beta must have been idle the wait: none here, but its first tick ended
    std::thread::sleep(std::time::Duration::from_secs(3));
    let lines = run(&world, "2");
    assert!(!instances(&world).join("ghost.sock").exists(), "{lines:?}");
    // beta has not worked since the host woke, which keeps nothing awake: only
    // the instance that puts the host to sleep must have
    let checked = last_check(&world);
    assert_eq!(checked["slept"], true, "{checked}");
    assert_eq!(world.pmset_calls(), ["sleepnow"]);
    drop(peer);
}

/// Listens as `delta` where every `run` of `world`'s host does: idle since the
/// epoch, it answers how it stands, and never answers once asked to tick.
fn never_ticks(world: &World) {
    let listener = bind(world, "delta.sock");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut asked = String::new();
                BufReader::new(&stream).read_line(&mut asked).unwrap();
                if asked.trim() == "standing" {
                    let standing = r#"{"instance":"delta","repo":"o/r","version":"0.4.3","no_host_sleep":false,"idle":true,"since":0,"worked":null}"#;
                    writeln!(&stream, "{standing}").unwrap();
                }
                // asked to tick: it holds on, answering nothing
                std::thread::sleep(Duration::from_secs(300));
            });
        }
    });
}

#[test]
fn a_run_told_to_stop_while_a_check_waits_for_the_ticks_stops_at_once() {
    let world = worked();
    never_ticks(&world);
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    world.host_sleeps(&mut command);
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1"];
    let pacing = ["--second-ms", "0", "--host-sleep-after", "0"];
    let mut run = command.args(args).args(pacing).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(run.stdout.take().unwrap()).lines();
    // the check asked delta to tick, and its own second tick has ended
    for line in lines.by_ref() {
        let line = line.unwrap();
        if line.starts_with("tick 2: ") && !bookkeeping(&line) {
            break;
        }
    }
    let told = Instant::now();
    let pid = run.id().to_string();
    assert!(Command::new("kill").args(["-TERM", &pid]).status().unwrap().success());
    let rest: Vec<String> = lines.map(|line| line.unwrap()).collect();
    assert!(run.wait().unwrap().success(), "{rest:?}");
    assert!(told.elapsed() < Duration::from_secs(5), "it waited {:?}", told.elapsed());
    assert!(rest.iter().any(|line| line.starts_with("stop:")), "{rest:?}");
    assert_eq!(
        world.pmset_calls(),
        Vec::<String>::new(),
        "a run told to stop puts nothing to sleep"
    );
}

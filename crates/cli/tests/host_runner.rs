//! `run` hands its checkout to the host's one host runner, starting it
//! detached where none runs, and returns once that checkout's fleet instance
//! has made its first tick (docs/adr/0023). Every test here runs `run`
//! detached, as an operator does, so the host runner is a process of its own:
//! the world kills it as it ends.

use crate::common;

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use afkconvoy::log::{Entry, What};
use common::{World, asked, commit, git};
use serde_json::{Value, json};

/// A `run` of the first repo on the host `h1`, its sleeps short.
const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "100"];

/// Every line of the tick log `answer` names, each as it is kept.
fn tick_log(answer: &Value) -> Vec<Value> {
    let text = std::fs::read_to_string(answer["tick_log"].as_str().unwrap()).unwrap();
    text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

/// What a line of a tick log is: its one key beside when, which instance and
/// which host — `start`, `tick_started`, `tick`, `stop`.
fn kind(line: &Value) -> String {
    let keys = line.as_object().unwrap().keys();
    let mut kinds = keys.filter(|key| !["at", "instance", "host"].contains(&key.as_str()));
    kinds.next().unwrap().clone()
}

/// The keys of an object.
fn keys(object: &Value) -> BTreeSet<String> {
    object.as_object().unwrap().keys().cloned().collect()
}

/// Whether the process `pid` is alive.
fn alive(pid: &Value) -> bool {
    let pid = pid.as_u64().unwrap().to_string();
    Command::new("kill").args(["-0", &pid]).status().unwrap().success()
}

/// The checkouts the host's list holds, in order.
fn listed(world: &World) -> Vec<String> {
    let list = world.checkout_list();
    let rows = list["checkouts"].as_array().unwrap().iter();
    rows.map(|row| row["checkout"].as_str().unwrap().to_string()).collect()
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

/// Asserts that each line of the host runner's log is one event, stamped in
/// UTC.
fn each_line_stamped(world: &World) {
    let text = std::fs::read_to_string(world.cache().join("afkconvoy/host-runner.log")).unwrap();
    for line in text.lines() {
        let stamped = line.starts_with("20") && line.len() > 20 && line.as_bytes()[19] == b' ';
        assert!(stamped, "{line:?} in:\n{text}");
    }
}

/// Waits up to 30 seconds for the host runner's log to hold a line that
/// starts with `start` → that line.
fn logged(world: &World, start: &str) -> String {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(line) = world.runner_log().into_iter().find(|line| line.starts_with(start)) {
            return line;
        }
        assert!(Instant::now() < until, "no {start:?} in {:#?}", world.runner_log());
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn run_adds_its_checkout_and_answers_once_its_first_tick_ended() {
    let world = World::new();
    // from a directory in the checkout: the checkout is its root
    let answer = world.afk_in(&world.clone.join("docs"), &RUN).ok();
    // what the help shows is what it answers, key for key
    let help = &common::shown("`afkconvoy run`", "json")[0];
    let help: Value = serde_json::from_str(help).unwrap();
    assert_eq!(keys(&answer), keys(&help), "{answer}");
    assert_eq!(keys(&answer["host_runner"]), keys(&help["host_runner"]), "{answer}");
    assert_eq!(answer["checkout"], json!(world.clone), "{answer}");
    assert_eq!(answer["repo"], json!(world.bare), "a remote that is a path is its directory");
    assert_eq!((&answer["added"], &answer["host_runner"]["started"]), (&json!(true), &json!(true)));
    assert_eq!(answer["host_runner"]["version"], afkconvoy::VERSION);
    assert_eq!(answer["host_runner"]["replaced"], Value::Null, "none ran: {answer}");
    assert!(answer["instance"].as_str().unwrap().starts_with("h1-"), "{answer}");
    assert_eq!(answer["warnings"], json!([]));

    // the tick log is as a `run` in the foreground keeps it, and the first
    // tick answered is the first tick it holds
    let first_tick = answer["first_tick"].as_str().unwrap();
    assert!(first_tick.starts_with("tick 1: "), "{first_tick}");
    let lines = tick_log(&answer);
    let kinds: Vec<String> = lines.iter().take(3).map(kind).collect();
    assert_eq!(kinds, ["start", "tick_started", "tick"]);
    let entry: Entry = serde_json::from_value(lines[2].clone()).unwrap();
    assert!(matches!(entry.what, What::Tick(_)));
    assert_eq!(entry.said(), first_tick);
    assert_eq!(entry.instance, answer["instance"].as_str().unwrap());

    // on the list, with what its `run` named for it alone; beside it, the
    // host setting its environment named, which its answer says it changed
    let list = world.checkout_list();
    assert_eq!(list["checkouts"], json!([{"checkout": world.clone, "named": {"host": "h1"}}]));
    assert_eq!(list["settings"]["no_host_sleep"], true, "AFKCONVOY_NO_HOST_SLEEP=true");
    assert_eq!(answer["settings"], json!({"no_host_sleep": {"from": false, "to": true}}));
    let log = world.runner_log();
    let pid = &answer["host_runner"]["pid"];
    let started = format!(
        "host runner started: afkconvoy {}, pid {pid}, 0 checkouts on the list",
        afkconvoy::VERSION
    );
    assert!(log.contains(&started), "{log:#?}");
    let clone = shown(&world.clone);
    let instance =
        format!("fleet instance started: {clone}: {} on h1", answer["instance"].as_str().unwrap());
    assert!(log.contains(&instance), "{log:#?}");
    let added = format!("checkout added: {clone} ({})", shown(&world.bare));
    assert!(log.contains(&added), "{log:#?}");
    let at = |line: &str| log.iter().position(|said| said == line).unwrap();
    assert!(at(&started) < at(&instance) && at(&instance) < at(&added), "{log:#?}");

    // `run` returned, and the host runner works on
    assert!(alive(pid), "{answer}");
    assert_eq!(world.host_runner(), pid.as_u64().map(|pid| pid as u32));
}

#[test]
fn the_host_runner_is_detached_and_writes_its_own_log() {
    let world = World::new();
    let program = env!("CARGO_BIN_EXE_afkconvoy");
    let out = world.command(program, &world.clone).args(RUN).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert_eq!(
        stdout.lines().count(),
        1,
        "one JSON object, and nothing of the host runner's: {stdout}"
    );
    let answer: Value = serde_json::from_str(&stdout).unwrap();

    // a process group of its own, which a terminal's Ctrl-C and hangup miss
    let pid = answer["host_runner"]["pid"].as_u64().unwrap();
    let group_of = |pid: u64| {
        let ps = Command::new("ps").args(["-o", "pgid=", "-p", &pid.to_string()]).output();
        String::from_utf8_lossy(&ps.unwrap().stdout).trim().parse::<u64>().unwrap()
    };
    assert_eq!(group_of(pid), pid);
    assert_ne!(group_of(u64::from(std::process::id())), pid);

    // its lines are in its own log, where its answers say
    let log = world.cache().join("afkconvoy/host-runner.log");
    assert_eq!(answer["host_runner"]["log"], json!(log));
    let said = world.runner_log();
    assert!(said.iter().any(|line| line.starts_with("host runner started: ")), "{said:#?}");
    assert!(said.iter().any(|line| line.starts_with("checkout added: ")), "{said:#?}");
    each_line_stamped(&world);
}

#[test]
fn what_a_fleet_instance_warns_of_reaches_the_run_and_the_log_after_its_checkout() {
    let world = World::new();
    world.shell_knows("kim", Some("kim is an alias for ANTHROPIC_BASE_URL=x claude --model k"));
    world.kit_command("claude-code", "kim");
    let ran = world.afk(&RUN);
    let warning = "the worker launch command \"kim\" shows no unattended flag";
    assert!(ran.stderr.contains(&format!("warning: {warning}")), "{}", ran.stderr);
    let answer = ran.ok();
    let warnings = answer["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|said| said.as_str().unwrap().starts_with(warning)), "{answer}");
    logged(&world, &format!("{}: warning: {warning}", shown(&world.clone)));
    each_line_stamped(&world);
}

#[test]
fn a_run_beside_a_host_runner_hands_its_checkout_to_it() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();
    assert_eq!(second["host_runner"]["pid"], first["host_runner"]["pid"], "{second}");
    assert_eq!(second["host_runner"]["started"], false, "{second}");
    // of the same version: no upgrade
    assert_eq!(second["host_runner"]["replaced"], Value::Null, "{second}");
    assert_eq!(second["added"], true, "{second}");
    assert_eq!(second["checkout"], json!(clone2));
    assert_eq!(second["repo"], json!(world.root.join("remote2.git")));
    assert_ne!(second["instance"], first["instance"], "a fleet instance each");
    assert_ne!(second["tick_log"], first["tick_log"], "and a tick log each");
    assert_eq!(listed(&world), [shown(&world.clone), shown(&clone2)]);
    // each repo's fleet reads its own GitHub
    assert!(!world.gh2()["calls"].as_array().unwrap().is_empty());

    // run again: the fleet instance that runs is the answer
    let again = world.afk(&RUN).ok();
    assert_eq!(again["host_runner"]["pid"], first["host_runner"]["pid"]);
    assert_eq!(again["added"], false, "{again}");
    assert_eq!(
        (&again["instance"], &again["first_tick"]),
        (&first["instance"], &first["first_tick"])
    );
    assert_eq!(listed(&world), [shown(&world.clone), shown(&clone2)]);
}

/// What a session the tests run under, and a `run` started from it, set for
/// itself: none reaches a host runner.
const SESSION: [(&str, &str); 3] =
    [("CLAUDE_CODE_ENTRYPOINT", "cli"), ("CLAUDE_CODE_SSE_PORT", "1"), ("ORCA_TERMINAL_ID", "t-9")];

/// Of `envs`, the names a session set for itself.
fn of_a_session(envs: &[String]) -> Vec<&String> {
    let set = |name: &&String| {
        ["CLAUDE_CODE_", "CLAUDECODE", "ORCA_"].iter().any(|s| name.starts_with(s))
    };
    envs.iter().filter(set).collect()
}

#[test]
fn the_host_runner_starts_with_a_fresh_login_shells_environment() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let program = env!("CARGO_BIN_EXE_afkconvoy");
    let run = world.command(program, &world.clone).args(RUN).envs(SESSION).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stdout));
    let answer: Value = serde_json::from_slice(&run.stdout).unwrap();
    assert!(answer["first_tick"].as_str().unwrap().contains("dispatched #7"), "{answer}");

    // the host runner's calls are those its login profile alone gives a name
    let runners = |calls: Vec<(Vec<String>, Vec<String>)>| -> Vec<(Vec<String>, Vec<String>)> {
        calls
            .into_iter()
            .filter(|(_, envs)| envs.contains(&"AFKCONVOY_TEST_LOGIN".into()))
            .collect()
    };
    let gh = runners(world.gh_called_with());
    let orca = runners(world.orca_called_with());
    assert!(!gh.is_empty(), "the host runner's fleet reads GitHub");
    assert!(orca.iter().any(|(call, _)| asked(call) == "terminal create"), "{orca:?}");
    for (call, envs) in gh.iter().chain(&orca) {
        assert_eq!(of_a_session(envs), [] as [&String; 0], "{call:?} ran with {envs:?}");
    }

    // the control: a fleet held in the session's own process has them all,
    // so the check above can fail
    let control = World::new();
    control.issue(7, &["ready-for-agent"]);
    let held = ["run", "--foreground", "--ticks", "1", "--second-ms", "0"];
    let ran = control.command(program, &control.clone).args(held).envs(SESSION).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
    let calls = control.gh_called_with().into_iter().chain(control.orca_called_with());
    let calls: Vec<_> = calls.collect();
    let started = calls.iter().find(|(call, _)| asked(call) == "terminal create");
    let (_, envs) = started.expect("the control started a worker");
    for (name, _) in SESSION {
        assert!(of_a_session(envs).contains(&&name.to_string()), "{name} in {envs:?}");
    }
    assert!(calls.iter().all(|(_, envs)| !envs.contains(&"AFKCONVOY_TEST_LOGIN".into())));
}

/// A config whose base branch the remote does not have, committed and
/// pushed: every start's probe refuses it.
fn base_branch_gone(world: &World) {
    let config = "```yaml\nbase_branch: release\ngate_command: ./gate.sh\n```\n";
    commit(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["push", "-q", "origin", "main"]);
}

#[test]
fn a_checkout_whose_startup_probe_fails_is_refused_and_not_left_on_the_list() {
    let world = World::new();
    base_branch_gone(&world);
    let refused = world.afk(&RUN);
    assert_eq!(refused.code, 1, "{}", refused.json);
    let error = refused.json["error"].as_str().unwrap();
    assert!(error.contains("the remote has no branch \"release\""), "{error}");
    assert_eq!(refused.json["see"], "afkconvoy help troubleshooting", "{}", refused.json);
    assert_eq!(refused.json["host_runner"]["started"], true, "{}", refused.json);
    assert!(alive(&refused.json["host_runner"]["pid"]), "the host runner works on");
    assert_eq!(listed(&world), [] as [String; 0]);
    let log = world.runner_log();
    let clone = shown(&world.clone);
    let said = format!("checkout refused: {clone}: ");
    assert!(log.iter().any(|line| line.starts_with(&said) && line.contains("release")), "{log:#?}");
    assert!(!log.iter().any(|line| line.starts_with("checkout added: ")), "{log:#?}");
}

#[test]
fn a_listed_checkout_whose_startup_probe_fails_is_refused_and_taken_off_the_list() {
    let world = World::new();
    world.afk(&RUN).ok();
    world.kill_host_runner("KILL");
    base_branch_gone(&world);

    let refused = world.afk(&RUN);
    assert_eq!(refused.code, 1, "{}", refused.json);
    let error = refused.json["error"].as_str().unwrap();
    assert!(error.contains("the remote has no branch \"release\""), "{error}");
    assert_eq!(listed(&world), [] as [String; 0]);
    let log = world.runner_log();
    let said = format!("checkout removed: {}: refused: ", shown(&world.clone));
    assert!(log.iter().any(|line| line.starts_with(&said)), "{log:#?}");
}

#[test]
fn a_second_checkout_of_a_repo_on_the_list_is_refused_naming_the_first() {
    let world = World::new();
    world.afk(&RUN).ok();
    let before = world.checkout_list();
    let clone = shown(&world.clone);

    // another clone of the same remote, and a worktree of the clone that is
    // no worker's
    let again = world.clone_again("again");
    let tree = world.root.join("tree");
    git(&world.clone, &["worktree", "add", "-q", "--detach", tree.to_str().unwrap(), "HEAD"]);
    for second in [&again, &tree] {
        let refused = world.afk_in(second, &RUN).error();
        assert!(
            refused.contains(&format!("is on this host's checkout list already, as {clone}")),
            "{refused}"
        );
        assert!(refused.contains(&format!("afkconvoy run {clone}")), "{refused}");
        let log = world.runner_log();
        let said = format!("checkout refused: {}: ", shown(second));
        assert!(log.iter().any(|line| line.starts_with(&said)), "{log:#?}");
    }
    assert_eq!(world.checkout_list(), before);
}

#[test]
fn a_host_runner_started_again_starts_every_checkout_on_the_list() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let first = world.afk(&RUN).ok();
    assert!(first["first_tick"].as_str().unwrap().contains("dispatched #7"), "{first}");
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &["run", "--second-ms", "100"]).ok();
    world.kill_host_runner("KILL");

    // a `run` that names nothing finds its checkout started from the list,
    // as its first `run` named it
    let again = world.afk(&["run"]).ok();
    assert_ne!(again["host_runner"]["pid"], first["host_runner"]["pid"]);
    assert_eq!(again["host_runner"]["started"], true, "{again}");
    assert_eq!(again["added"], false, "{again}");
    let instance = again["instance"].as_str().unwrap();
    assert!(instance.starts_with("h1-") && again["instance"] != first["instance"], "{again}");
    let log = world.runner_log();
    let took = format!("{}: start: took #7 stamped with this host", shown(&world.clone));
    assert!(log.iter().any(|line| line.starts_with(&took)), "{log:#?}");
    assert!(log.iter().any(|line| line.ends_with(", 2 checkouts on the list")), "{log:#?}");
    // one of the same version before it is no upgrade
    assert!(!log.iter().any(|line| line.starts_with("host runner upgraded")), "{log:#?}");
    assert_eq!(again["host_runner"]["replaced"], Value::Null, "{again}");
    // it took its own host's claims (docs/adr/0003)
    assert_eq!(world.claims(), [(7, instance.to_string())]);
    // and the other checkout on the list was started too
    let starts = tick_log(&second).iter().filter(|line| kind(line) == "start").count();
    assert_eq!(starts, 2, "{:#?}", tick_log(&second));
    assert_eq!(listed(&world), [shown(&world.clone), shown(&clone2)]);
}

#[test]
fn a_run_of_a_listed_checkout_the_host_runner_could_not_start_starts_it_as_its_run_named_it() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.afk(&RUN).ok();
    world.kill_host_runner("KILL");
    // the next host runner cannot start the checkout by itself: its login
    // shell sets a flag of `run` to what `run` refuses
    let profile = std::fs::OpenOptions::new().append(true).open(world.home().join(".profile"));
    writeln!(profile.unwrap(), "export AFKCONVOY_HOT_CORES=bogus").unwrap();

    let again = world.afk(&["run", "--second-ms", "100"]).ok();
    let log = world.runner_log();
    let clone = shown(&world.clone);
    let not_started = format!("fleet instance not started: {clone}: ");
    let line = log.iter().find(|line| line.starts_with(&not_started)).expect("its start failed");
    assert!(line.contains("'bogus'") && line.ends_with("stays on the checkout list"), "{log:#?}");
    each_line_stamped(&world);
    // started on the host its first `run` named, taking that host's claims
    let instance = again["instance"].as_str().unwrap();
    assert!(instance.starts_with("h1-"), "{again}");
    assert_eq!(world.claims(), [(7, instance.to_string())]);
    let list = world.checkout_list();
    assert_eq!(list["checkouts"], json!([{"checkout": world.clone, "named": {"host": "h1"}}]));
}

#[test]
fn a_fleet_instance_that_ended_stops_watching_its_workers() {
    let world = World::new();
    // its loop ends after its first tick, which `run` may answer or not
    let _ = world.afk(&["run", "--host", "h1", "--second-ms", "10", "--ticks", "1"]);
    logged(&world, &format!("fleet instance ended: {}: ", shown(&world.clone)));
    // a sample it took as it ended is over
    std::thread::sleep(Duration::from_secs(1));
    let calls = || world.orca()["calls"].as_array().unwrap().clone();
    let ended = calls().len();
    // a watcher left watching asks orca for the worktrees every sixth
    // sample, each a tenth of a second here and what it takes
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(calls()[ended..], [] as [Value; 0], "asked after its fleet instance ended");
}

#[test]
fn sigterm_stops_every_fleet_instance_and_keeps_the_list() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();
    let before = world.checkout_list();

    world.kill_host_runner("TERM");
    for answer in [&first, &second] {
        let lines = tick_log(answer);
        assert_eq!(kind(lines.last().unwrap()), "stop", "{lines:#?}");
    }
    let log = world.runner_log();
    assert!(
        log.contains(&"host runner stopped: 2 checkouts kept on the list".to_string()),
        "{log:#?}"
    );
    assert!(!world.cache().join("afkconvoy/host-runner.sock").exists(), "its socket goes");
    assert_eq!(world.host_runner(), None);
    assert_eq!(world.checkout_list(), before);
}

/// A load no host of any number of cores is under, as the stand-in `sysctl`
/// says it.
const PAST_ANY: &str = "100000.00";

/// Waits up to 30 seconds for a tick of the fleet instance `answer` names
/// whose line holds `said` → that line.
fn ticked(answer: &Value, said: &str) -> String {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let lines = tick_log(answer).into_iter().filter(|line| kind(line) == "tick");
        let entries = lines.map(|line| serde_json::from_value::<Entry>(line).unwrap().said());
        if let Some(line) = entries.into_iter().find(|line| line.contains(said)) {
            return line;
        }
        assert!(Instant::now() < until, "no tick says {said:?}: {:#?}", tick_log(answer));
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_host_setting_a_run_names_holds_for_every_fleet_instance_at_once() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.busy(PAST_ANY);
    let first = world.afk(&RUN).ok();
    let held = first["first_tick"].as_str().unwrap();
    assert!(held.contains("admission holds back new workers"), "{first}");

    // a `run` of another repo names a share: the first repo's fleet
    // instance, never started again, admits at a tick of its own
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &[&RUN[..], &["--admit-load", "100000000"]].concat()).ok();
    assert_eq!(second["settings"], json!({"admit_load": {"from": null, "to": 100000000}}));
    assert_eq!(second["host_runner"]["pid"], first["host_runner"]["pid"]);
    assert!(ticked(&first, "dispatched #7").contains("holding 1/"));
    assert_eq!(world.claims(), [(7, first["instance"].as_str().unwrap().to_string())]);

    // naming it as it is changes nothing, and `default` changes it back
    let again = world.afk(&[&RUN[..], &["--admit-load", "100000000"]].concat()).ok();
    assert_eq!(again["settings"], json!({}));
    let back = world.afk(&[&RUN[..], &["--admit-load", "default"]].concat()).ok();
    assert_eq!(back["settings"], json!({"admit_load": {"from": 100000000, "to": null}}));
    assert_eq!(world.checkout_list()["settings"]["admit_load"], Value::Null);
}

#[test]
fn the_host_settings_outlive_the_host_runner_and_doctor_says_those_in_force() {
    let world = World::new();
    let named = ["--hot-cores", "2.5", "--hot-memory", "8", "--admit-disk", "0"];
    let first = world.afk(&[&RUN[..], &named[..]].concat()).ok();
    let changed = &first["settings"];
    assert_eq!(changed["hot_cores"], json!({"from": null, "to": 2.5}), "{first}");
    assert_eq!(changed["hot_memory"], json!({"from": null, "to": 8.0}), "{first}");
    assert_eq!(changed["admit_disk"], json!({"from": null, "to": 0}), "{first}");
    world.kill_host_runner("KILL");

    // a host runner started again keeps them, and a `run` that names none
    // changes none
    let again = world.afk(&["run"]).ok();
    assert_eq!(again["host_runner"]["started"], true, "{again}");
    assert_eq!(again["settings"], json!({}), "{again}");
    let settings = &world.afk(&["doctor", "--host", "h1"]).json["host"]["settings"];
    assert_eq!(settings["file"], json!(world.cache().join("afkconvoy/checkouts.json")));
    let (said, in_force) = (&settings["said"], &settings["in_force"]);
    assert_eq!((&said["hot_cores"], &in_force["hot_cores"]), (&json!(2.5), &json!(2.5)));
    assert_eq!((&said["admit_disk"], &in_force["admit_disk"]), (&json!(0), &json!(0)));
    assert_eq!(in_force["hot_memory"], 8.0);
    // what it says none of, its default worked out for this host
    assert_eq!((&said["admit_load"], &in_force["admit_load"]), (&Value::Null, &json!(200)));
    assert_eq!(said["hot_written"], Value::Null);
    assert!(in_force["hot_written"].as_f64().unwrap() >= 1.0, "{settings}");
    assert_eq!(in_force["no_host_sleep"], true, "{settings}");

    // the flag holds over the variable that stands for the other
    if cfg!(target_os = "macos") {
        let woken = world.afk(&["run", "--host-sleep"]).ok();
        assert_eq!(woken["settings"], json!({"no_host_sleep": {"from": true, "to": false}}));
    }
}

#[test]
fn stop_takes_one_checkout_off_the_list_and_stops_its_fleet_instance_alone() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let first = world.afk(&RUN).ok();
    assert!(first["first_tick"].as_str().unwrap().contains("dispatched #7"), "{first}");
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();

    // from a directory in the checkout: the checkout is its root
    let stopped = world.afk_in(&world.clone.join("docs"), &["stop"]).ok();
    // what the help shows is what it answers, key for key
    let help = &common::shown("`afkconvoy stop`", "json")[0];
    let help: Value = serde_json::from_str(help).unwrap();
    assert_eq!(keys(&stopped), keys(&help), "{stopped}");
    assert_eq!(keys(&stopped["host_runner"]), keys(&help["host_runner"]), "{stopped}");
    assert_eq!(stopped["checkout"], json!(world.clone), "{stopped}");
    assert_eq!(stopped["repo"], json!(world.bare), "{stopped}");
    assert_eq!(stopped["removed"], true, "{stopped}");
    assert_eq!(stopped["instance"], first["instance"], "{stopped}");
    assert_eq!(stopped["host_runner"]["pid"], first["host_runner"]["pid"], "{stopped}");
    // it answers once its fleet instance stopped, the claim it holds kept
    // with its heartbeat (docs/adr/0003)
    let left = "stop: it left its heartbeat with the claims it holds: the next start on this host \
                takes them";
    assert_eq!(stopped["stop"], left, "{stopped}");
    let lines = tick_log(&first);
    assert_eq!(kind(lines.last().unwrap()), "stop", "{lines:#?}");
    let instance = first["instance"].as_str().unwrap();
    assert_eq!(world.claims(), [(7, instance.to_string())]);

    // the other checkout's fleet instance works on, in the same host runner
    assert_eq!(listed(&world), [shown(&clone2)]);
    assert!(alive(&first["host_runner"]["pid"]));
    assert!(!tick_log(&second).iter().any(|line| kind(line) == "stop"), "{second}");
    let log = world.runner_log();
    let clone = shown(&world.clone);
    let removed =
        format!("checkout removed: {clone}: by `afkconvoy stop`, its fleet instance stops");
    let ended = format!("fleet instance ended: {clone}: ");
    let at = |start: &str| log.iter().position(|line| line.starts_with(start));
    assert!(at(&removed) < at(&ended) && at(&ended).is_some(), "{log:#?}");
    each_line_stamped(&world);

    // not on the list: it says so, exit 0, and changes nothing
    let before = world.checkout_list();
    let again = world.afk(&["stop"]).ok();
    assert_eq!(
        (&again["removed"], &again["instance"], &again["stop"]),
        (&json!(false), &Value::Null, &Value::Null),
        "{again}"
    );
    assert_eq!(world.checkout_list(), before);
    // a second checkout of a repo on the list is refused, naming the first
    let other = world.clone_again("again");
    git(&other, &["remote", "set-url", "origin", world.root.join("remote2.git").to_str().unwrap()]);
    let refused = world.afk_in(&other, &["stop"]).error();
    let named =
        format!("is on this host's checkout list as {}, not as {}", shown(&clone2), shown(&other));
    assert!(refused.contains(&named), "{refused}");
    assert!(refused.contains(&format!("afkconvoy stop {}", shown(&clone2))), "{refused}");
    assert_eq!(world.checkout_list(), before);
}

#[test]
fn stop_all_stops_every_fleet_instance_then_the_host_runner_and_keeps_the_list() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();
    let before = world.checkout_list();

    // run anywhere on the host
    let stopped = world.afk_in(&world.root, &["stop", "--all"]).ok();
    let help = &common::shown("`afkconvoy stop`", "json")[1];
    let help: Value = serde_json::from_str(help).unwrap();
    assert_eq!(keys(&stopped), keys(&help), "{stopped}");
    assert_eq!(keys(&stopped["stopped"][0]), keys(&help["stopped"][0]), "{stopped}");
    assert_eq!(stopped["checkouts"], 2, "{stopped}");
    assert_eq!(stopped["host_runner"]["pid"], first["host_runner"]["pid"], "{stopped}");
    let took = "stop: holding no claim, it took its heartbeat with it";
    let mut each: Vec<(Value, Value, Value)> = stopped["stopped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|one| (one["checkout"].clone(), one["instance"].clone(), one["stop"].clone()))
        .collect();
    each.sort_by_key(|(checkout, _, _)| checkout.to_string());
    assert_eq!(
        each,
        [
            (json!(world.clone), first["instance"].clone(), json!(took)),
            (json!(clone2), second["instance"].clone(), json!(took)),
        ]
    );

    // it answered once the host runner ended, every fleet instance's `stop:`
    // line in its tick log, and the list kept
    assert_eq!(world.host_runner(), None);
    assert!(!alive(&first["host_runner"]["pid"]));
    for answer in [&first, &second] {
        let lines = tick_log(answer);
        assert_eq!(kind(lines.last().unwrap()), "stop", "{lines:#?}");
    }
    assert_eq!(world.checkout_list(), before);
    let log = world.runner_log();
    let at = |line: &str| log.iter().position(|said| said == line);
    let told = at("host runner told to stop by `afkconvoy stop --all`: it keeps the checkout list");
    let ended = at("host runner stopped: 2 checkouts kept on the list");
    assert!(told.is_some() && told < ended, "{log:#?}");

    // the next `run` starts every checkout on it again
    let again = world.afk(&["run"]).ok();
    assert_eq!(again["host_runner"]["started"], true, "{again}");
    logged(&world, &format!("fleet instance started: {}: ", shown(&clone2)));
    assert_eq!(world.checkout_list(), before);
}

#[test]
fn with_no_host_runner_stop_changes_the_list_itself_and_stop_all_changes_nothing() {
    let world = World::new();
    world.afk(&RUN).ok();
    world.kill_host_runner("KILL");

    let stopped = world.afk(&["stop"]).ok();
    assert_eq!(stopped["removed"], true, "{stopped}");
    assert_eq!(
        (&stopped["instance"], &stopped["stop"], &stopped["host_runner"]),
        (&Value::Null, &Value::Null, &Value::Null),
        "{stopped}"
    );
    assert_eq!(listed(&world), [] as [String; 0]);
    let removed = format!(
        "checkout removed: {}: by `afkconvoy stop`, with no host runner running",
        shown(&world.clone)
    );
    assert!(world.runner_log().contains(&removed), "{:#?}", world.runner_log());
    each_line_stamped(&world);
    assert_eq!(world.host_runner(), None, "none was started");

    let all = world.afk(&["stop", "--all"]).ok();
    assert_eq!(all, json!({"stopped": [], "checkouts": 0, "host_runner": null}));
    assert_eq!(world.host_runner(), None);
}

#[test]
fn a_newer_afkconvoy_moves_the_host_runner_onto_itself_on_the_same_list() {
    let world = World::new();
    // what a host runner of an older afkconvoy holds: a checkout of another
    // repo on the list, and what the list keeps beside its checkouts
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();
    world.kill_host_runner("TERM");
    let mut list = world.checkout_list();
    list["settings"]["kept"] = json!("as it is");
    let file = world.cache().join("afkconvoy/checkouts.json");
    std::fs::write(file, list.to_string()).unwrap();
    let mut older = world.host_runner_of("0.0.1");
    let old = older.id();

    let answer = world.afk(&RUN).ok();
    let host_runner = &answer["host_runner"];
    assert_eq!(host_runner["replaced"], json!({"pid": old, "version": "0.0.1"}), "{answer}");
    assert_eq!(host_runner["version"], afkconvoy::VERSION, "{answer}");
    assert_eq!(host_runner["started"], true, "{answer}");
    let new = host_runner["pid"].as_u64().unwrap();
    assert_ne!(new, u64::from(old));
    // the older one was told to stop as SIGTERM tells it, and ended
    assert!(older.wait().unwrap().success());
    assert_eq!(world.host_runner(), Some(new as u32));

    // the newer one works the same list, every checkout on it
    assert_eq!(answer["added"], true, "{answer}");
    assert_eq!(listed(&world), [shown(&clone2), shown(&world.clone)]);
    assert_eq!(world.checkout_list()["settings"], list["settings"]);
    let starts = tick_log(&second).iter().filter(|line| kind(line) == "start").count();
    assert_eq!(starts, 2, "{:#?}", tick_log(&second));

    // and its log says what the answer says
    let log = world.runner_log();
    let upgraded = format!(
        "host runner upgraded: from afkconvoy 0.0.1 (pid {old}) to afkconvoy {} (pid {new})",
        afkconvoy::VERSION
    );
    let started = format!("host runner started: afkconvoy {}, pid {new}, ", afkconvoy::VERSION);
    let at = |line: &str| log.iter().position(|said| said.starts_with(line));
    let (started, upgraded) = (at(&started), at(&upgraded));
    assert!(upgraded.is_some() && upgraded == started.map(|started| started + 1), "{log:#?}");
}

#[test]
fn a_run_older_than_the_host_runner_changes_nothing_and_says_so() {
    let world = World::new();
    let mut newer = world.host_runner_of("999.0.0");

    let refused = world.afk(&RUN);
    assert_eq!(refused.code, 1, "{}", refused.json);
    let error = refused.error();
    let said = format!(
        "this afkconvoy, {}, is older than the host runner's, 999.0.0 (pid {}), so it changed \
         nothing",
        afkconvoy::VERSION,
        newer.id()
    );
    assert!(error.starts_with(&said), "{error}");
    // the newer one holds the host as before, and nothing was listed or started
    assert!(newer.try_wait().unwrap().is_none(), "the newer host runner works on");
    assert_eq!(world.host_runner(), Some(newer.id()));
    assert_eq!(world.checkout_list(), json!({"checkouts": []}));
    assert_eq!(world.runner_log(), [] as [String; 0]);
    assert!(!world.cache().join("afkconvoy/runs").exists(), "no tick log");
    newer.kill().unwrap();
    newer.wait().unwrap();
}

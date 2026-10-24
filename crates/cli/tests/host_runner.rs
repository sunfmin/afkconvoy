//! `run` hands its checkout to the host's one host runner, starting it
//! detached where none runs, and returns once that checkout's fleet instance
//! has made its first tick (docs/adr/0023). Every test here runs `run`
//! detached, as an operator does, so the host runner is a process of its own:
//! the world kills it as it ends.

use crate::common;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

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

#[test]
fn run_adds_its_checkout_and_answers_once_its_first_tick_ended() {
    let world = World::new();
    // from a directory in the checkout: the checkout is its root
    let answer = world.afk_in(&world.clone.join("docs"), &RUN).ok();
    let expected = ["added", "checkout", "first_tick", "host_runner", "instance", "repo"];
    let expected = expected.into_iter().chain(["tick_log", "warnings"]).map(String::from);
    assert_eq!(keys(&answer), expected.collect(), "{answer}");
    let about = ["log", "pid", "started", "version"].map(String::from);
    assert_eq!(keys(&answer["host_runner"]), BTreeSet::from(about), "{answer}");
    assert_eq!(answer["checkout"], json!(world.clone), "{answer}");
    assert_eq!(answer["repo"], json!(world.bare), "a remote that is a path is its directory");
    assert_eq!((&answer["added"], &answer["host_runner"]["started"]), (&json!(true), &json!(true)));
    assert_eq!(answer["host_runner"]["version"], afkconvoy::VERSION);
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

    // on the list, with what its `run` named for it alone
    let list = world.checkout_list();
    assert_eq!(list, json!({"checkouts": [{"checkout": world.clone, "named": {"host": "h1"}}]}));
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
    // each line stamped in UTC
    let text = std::fs::read_to_string(&log).unwrap();
    let first = text.lines().next().unwrap();
    assert!(first.starts_with("20") && first.as_bytes()[19] == b' ', "{first}");
}

#[test]
fn a_run_beside_a_host_runner_hands_its_checkout_to_it() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();
    assert_eq!(second["host_runner"]["pid"], first["host_runner"]["pid"], "{second}");
    assert_eq!(second["host_runner"]["started"], false, "{second}");
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
    // it took its own host's claims (docs/adr/0003)
    assert_eq!(world.claims(), [(7, instance.to_string())]);
    // and the other checkout on the list was started too
    let starts = tick_log(&second).iter().filter(|line| kind(line) == "start").count();
    assert_eq!(starts, 2, "{:#?}", tick_log(&second));
    assert_eq!(listed(&world), [shown(&world.clone), shown(&clone2)]);
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

//! `run` hands its checkout to the host's one host runner, starting it
//! detached where none runs, and returns once that checkout's fleet instance
//! has made its first tick (docs/adr/0023). Every test here runs `run`
//! detached, as an operator does, so the host runner is a process of its own:
//! the world kills it as it ends.

use crate::common;

use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use afkconvoy::log::{Entry, What};
use common::{PATIENCE, World, asked, commit, git};
use serde_json::{Value, json};

/// A `run` of the first repo on the host `h1`, its sleeps short.
const RUN: [&str; 7] = ["run", "--tracker", "default", "--host", "h1", "--second-ms", "100"];

#[test]
fn the_initial_panel_does_not_ask_the_host_runner_before_it_can_answer() {
    let world = World::new();
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.root);
    command.args(["host-runner", "--kept-by", "0"]);
    let _runner = common::FixtureProcess::spawn(&mut command);
    let began = Instant::now();
    while !world.cache().join("afkconvoy/host-runner.sock").exists() {
        assert!(began.elapsed() < Duration::from_secs(5), "runner did not listen");
        std::thread::sleep(Duration::from_millis(10));
    }
    world.afk(&["checkouts"]).ok();
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "the initial panel waited for the runner's own unanswered socket: {:?}",
        world.runner_log()
    );
}

#[test]
fn a_host_runner_ends_when_its_test_is_killed() {
    const PROBE: &str = "CONVOY_RUNNER_OWNER_PROBE";
    if let Some(path) = std::env::var_os(PROBE) {
        let world = World::new();
        world.afk(&RUN).ok();
        std::fs::write(path, world.host_runner().unwrap().to_string()).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let rendezvous = tempfile::tempdir().unwrap();
    let pid_file = rendezvous.path().join("runner");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "host_runner::a_host_runner_ends_when_its_test_is_killed"])
        .env(PROBE, &pid_file)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + PATIENCE;
    while !pid_file.exists() {
        assert!(child.try_wait().unwrap().is_none(), "probe ended before starting its runner");
        assert!(Instant::now() < until, "probe did not start its runner");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid: libc::pid_t = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    // SAFETY: the probe reports its runner before it parks; signal zero only observes it.
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    child.kill().unwrap();
    child.wait().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while unsafe { libc::kill(pid, 0) } == 0 {
        if Instant::now() >= until {
            // SAFETY: this is the runner the probe just started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            panic!("host runner {pid} outlived its killed test");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A `run` as `RUN`, for a test that waits for the next tick of a fleet
/// instance that holds claims: a second of its sleeps takes 10ms, so each tick
/// comes 0.9s after the last, not 9s. Not for an idle one: its empty ticks come
/// as fast, and after three it sleeps its idle wait, 15s.
const TICKING: [&str; 5] = ["run", "--host", "h1", "--second-ms", "10"];

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
/// UTC with its zone named: `2026-10-30T13:56:16Z `.
fn each_line_stamped(world: &World) {
    let text = std::fs::read_to_string(world.cache().join("afkconvoy/host-runner.log")).unwrap();
    for line in text.lines() {
        let bytes = line.as_bytes();
        let stamped = line.len() > 21 && bytes[10] == b'T' && &line[19..21] == "Z ";
        assert!(stamped, "{line:?} in:\n{text}");
    }
}

/// Waits up to `PATIENCE` for the host runner's log to hold a line that
/// starts with `start` → that line.
pub(crate) fn logged(world: &World, start: &str) -> String {
    let until = Instant::now() + PATIENCE;
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

    // on the list, with what its `run` named for it alone; it named no host
    // setting, and its answer says it changed none
    let list = world.checkout_list();
    assert_eq!(list["checkouts"], json!([{"checkout": world.clone, "named": {"host": "h1"}}]));
    assert_eq!(answer["settings"], json!({}));
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
    // its login profile sets no XDG_CONFIG_HOME, nor does the host runner:
    // a worker it starts, and the gates that worker's turn runs, read their
    // tools' config where the host user's own shell does (sunfmin/control#765)
    for (call, envs) in gh.iter().chain(&orca) {
        assert!(!envs.contains(&"XDG_CONFIG_HOME".into()), "{call:?} ran with {envs:?}");
    }

    // the control: a fleet held in the session's own process has them all,
    // so the check above can fail
    let control = World::new();
    control.issue(7, &["ready-for-agent"]);
    let held = ["run", "--tracker", "default", "--foreground", "--ticks", "1", "--second-ms", "0"];
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
pub(crate) fn base_branch_gone(world: &World) {
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
fn a_listed_checkout_whose_startup_probe_fails_is_refused_and_stays_on_the_list_broken() {
    let world = World::new();
    world.afk(&RUN).ok();
    world.kill_host_runner("KILL");
    base_branch_gone(&world);

    // the host runner this `run` starts starts the checkout from the list,
    // and the `run` asks for it as well: whichever start is refused, the
    // checkout stays where it was
    let refused = world.afk(&RUN);
    assert_eq!(refused.code, 1, "{}", refused.json);
    let error = refused.json["error"].as_str().unwrap();
    assert!(error.contains("the remote has no branch \"release\""), "{error}");
    let stays = "its checkout stays on the checkout list, and its fleet instance is broken";
    assert!(error.contains(stays), "{error}");
    assert_eq!(refused.json["see"], "afkconvoy help troubleshooting", "{}", refused.json);
    assert_eq!(listed(&world), [shown(&world.clone)]);
    let log = world.runner_log();
    let clone = shown(&world.clone);
    let broken = format!("fleet instance broken: {clone}: ");
    assert!(log.iter().any(|line| line.starts_with(&broken)), "{log:#?}");
    assert!(!log.iter().any(|line| line.starts_with("checkout removed: ")), "{log:#?}");
    let checkouts = world.afk(&["checkouts"]).ok();
    assert_eq!(checkouts["checkouts"][0]["state"], "broken", "{checkouts}");

    // fixed, a `run` starts it at once
    git(&world.clone, &["revert", "--no-edit", "HEAD"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    let again = world.afk(&RUN).ok();
    assert_eq!(again["added"], false, "{again}");
    assert!(again["first_tick"].as_str().unwrap().starts_with("tick 1: "), "{again}");
    logged(&world, &format!("fleet instance back: {clone}: "));
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
fn changing_orca_state_keeps_updates_from_a_call_already_in_progress() {
    let world = World::new();
    world.with_orca(|orca| orca["slow_ms"] = json!(1000));
    let lock = std::fs::File::create(world.root.join("orca.lock")).unwrap();
    let mut call = world
        .command(world.root.join("bin/orca"), &world.clone)
        .args(["status", "--json"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // The stand-in holds this lock after reading its state, until it saves
    // the call. Change that same state while the call is still in progress.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match lock.try_lock() {
            Err(std::fs::TryLockError::WouldBlock) => break,
            Ok(()) => lock.unlock().unwrap(),
            Err(error) => panic!("{error}"),
        }
        assert!(Instant::now() < until, "orca never held its state lock");
        std::thread::sleep(Duration::from_millis(1));
    }
    let clone2 = world.root.join("clone2");
    world.with_orca(|orca| {
        orca["repos"].as_array_mut().unwrap().push(json!({"id": "repo-2", "path": clone2}));
    });
    assert!(call.wait().unwrap().success());
    let state = world.orca();
    assert_eq!(state["repos"][1]["path"], json!(clone2), "{state}");
    assert_eq!(state["calls"], json!([["status", "--json"]]), "{state}");
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
fn a_host_runner_ended_unexpectedly_is_started_again_by_its_keeper_on_the_same_list() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    world.afk_in(&clone2, &["run", "--second-ms", "100"]).ok();
    let (old, keeper) = (world.host_runner().unwrap(), world.keeper().unwrap());
    assert_ne!(old, keeper, "the keeper holds nothing of the host");
    // not from `/`, as the host runner works: what ends every process working
    // in `/` leaves it to start the host runner again (sunfmin/control#831)
    let cwd = Command::new("/usr/sbin/lsof")
        .args(["-a", "-d", "cwd", "-Fn", "-p", &keeper.to_string()])
        .output()
        .unwrap();
    let cwd = String::from_utf8_lossy(&cwd.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(str::to_string);
    let cache = world.cache().join("afkconvoy").canonicalize().unwrap();
    assert_eq!(cwd.map(PathBuf::from), Some(cache), "the keeper works in the cache directory");
    assert_eq!(first["host_runner"]["pid"], old, "{first}");

    // kill -9 of the host runner: nobody runs `afkconvoy run`
    let killed = Command::new("kill").args(["-KILL", &old.to_string()]).status().unwrap();
    assert!(killed.success());
    let began = Instant::now();
    let again =
        until("host runner started again", || world.host_runner().filter(|pid| *pid != old));
    assert!(began.elapsed() < Duration::from_secs(60), "{:?}", began.elapsed());
    assert_eq!(world.keeper(), Some(keeper), "the same keeper starts it again");
    let said = logged(&world, "host runner restarted after an unexpected end: ");
    assert!(said.contains(&format!("pid {old} was ended by signal 9")), "{said}");
    // on the same list, every checkout on it started again
    assert_eq!(listed(&world), [shown(&world.clone), shown(&clone2)]);
    until("every checkout started again", || {
        let answer = world.afk(&["checkouts"]).ok();
        let rows = answer["checkouts"].as_array()?.clone();
        let running =
            rows.iter().all(|row| row["state"] == "running" && row["instance"].is_string());
        (answer["host_runner"]["pid"] == again && rows.len() == 2 && running).then_some(())
    });
    let log = world.runner_log();
    let started = format!(
        "host runner started: afkconvoy {}, pid {again}, 2 checkouts on the list",
        afkconvoy::VERSION
    );
    assert!(log.contains(&started), "{log:#?}");

    // told to stop, it ends, and its keeper with it: nothing starts it again
    world.afk(&["stop", "--all"]).ok();
    let keeper = json!(keeper);
    until("the keeper ended", || (!alive(&keeper)).then_some(()));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(world.host_runner(), None, "{:#?}", world.runner_log());
    let restarted =
        world.runner_log().iter().filter(|line| line.starts_with("host runner restarted")).count();
    assert_eq!(restarted, 1, "{:#?}", world.runner_log());
}

/// The tests of this file that start, stop, replace, kill and restart a host
/// runner, which a cut's gate runs on the host whose own host runner runs
/// that gate.
const ON_THE_HOST: [&str; 5] = [
    "host_runner::stop_all_stops_every_fleet_instance_then_the_host_runner_and_keeps_the_list",
    "host_runner::sigterm_stops_every_fleet_instance_and_keeps_the_list",
    "host_runner::a_newer_afkconvoy_moves_the_host_runner_onto_itself_on_the_same_list",
    "host_runner::a_host_runner_started_again_starts_every_checkout_on_the_list",
    "host_runner::a_host_runner_ended_unexpectedly_is_started_again_by_its_keeper_on_the_same_list",
];

#[test]
fn the_suite_s_host_runner_tests_never_stop_nor_signal_the_host_s_own_host_runner() {
    // The host's own host runner, of a cache of this test's own: the tests
    // below are run as a cut's gate runs them — under its HOME and cache
    // directory, and in its process group, as a gate it ran is
    // (sunfmin/control#820).
    let host = World::new();
    host.afk(&RUN).ok();
    let (runner, keeper) = (host.host_runner().unwrap(), host.keeper().unwrap());
    let before = host.runner_log().len();
    let tests = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "--test-threads", "5"])
        .args(ON_THE_HOST)
        .env("HOME", host.home())
        .env("XDG_CACHE_HOME", host.cache())
        .process_group(runner as i32)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&tests.stdout);
    assert!(tests.status.success(), "{said}{}", String::from_utf8_lossy(&tests.stderr));
    assert!(said.contains(&format!("{} passed", ON_THE_HOST.len())), "{said}");

    // neither stopped, nor ended, nor started again
    assert!(alive(&json!(runner)) && alive(&json!(keeper)));
    assert_eq!((host.host_runner(), host.keeper()), (Some(runner), Some(keeper)));
    let log = host.runner_log();
    let after: Vec<_> = log[before..]
        .iter()
        .filter(|line| {
            ["host runner told", "host runner stopp", "host runner restarted", "host runner moved"]
                .iter()
                .any(|start| line.starts_with(start))
        })
        .collect();
    assert!(after.is_empty(), "{log:#?}");
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
    // A watcher left watching asks orca for the worktrees every sixth sample,
    // each a tenth of a second here and what it takes: well under a second
    // apart. A sample it took as it ended may still be finishing, longer
    // under load: orca is waited on to be asked nothing for 3 seconds, which
    // a watcher left watching never lets it be.
    let calls = || world.orca()["calls"].as_array().unwrap().len();
    let until = Instant::now() + PATIENCE;
    let (mut seen, mut since) = (calls(), Instant::now());
    while since.elapsed() < Duration::from_secs(3) {
        assert!(Instant::now() < until, "orca still asked, {seen} calls, {PATIENCE:?} on");
        std::thread::sleep(Duration::from_millis(100));
        let now = calls();
        if now != seen {
            (seen, since) = (now, Instant::now());
        }
    }
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

/// Waits up to `PATIENCE` for a tick of the fleet instance `answer` names
/// whose line holds `said` → that line.
fn ticked(answer: &Value, said: &str) -> String {
    let until = Instant::now() + PATIENCE;
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
    const RUN: [&str; 5] = TICKING;
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
    assert!(ticked(&first, "dispatched #7").contains("holding 1"));
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
    let named = [
        "--hot-cores",
        "2.5",
        "--hot-memory",
        "8",
        "--admit-disk",
        "0",
        "--exclusive-seconds",
        "1500",
        "--gate-lanes",
        "2",
    ];
    let first = world.afk(&[&RUN[..], &named[..]].concat()).ok();
    let changed = &first["settings"];
    assert_eq!(changed["hot_cores"], json!({"from": null, "to": 2.5}), "{first}");
    assert_eq!(changed["hot_memory"], json!({"from": null, "to": 8.0}), "{first}");
    assert_eq!(changed["admit_disk"], json!({"from": null, "to": 0}), "{first}");
    assert_eq!(changed["exclusive_seconds"], json!({"from": null, "to": 1500}), "{first}");
    assert_eq!(changed["gate_lanes"], json!({"from": null, "to": 2}));
    assert!(changed.get("train_lanes").is_none(), "{changed}");
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
    assert_eq!(said["exclusive_seconds"], 1500);
    assert_eq!(in_force["exclusive_seconds"], 1500);
    assert_eq!(in_force["gate_lanes"], 2);
    assert!(said.get("train_lanes").is_none(), "{settings}");
    assert!(
        in_force.get("train_lanes").is_none() && in_force.get("ignored").is_none(),
        "{settings}"
    );
    let before = again["instance"].clone();
    let unchanged = world.checkout_list();
    let refused = world.afk(&["run", "--train-lanes", "2"]).error();
    assert!(
        refused.contains("unexpected argument") && refused.contains("--train-lanes"),
        "{refused}"
    );
    assert_eq!(world.checkout_list(), unchanged);
    let with_variable = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("AFKCONVOY_TRAIN_LANES", "9")
        .args(["run", "--gate-lanes", "4"])
        .output()
        .unwrap();
    assert!(with_variable.status.success(), "{}", String::from_utf8_lossy(&with_variable.stderr));
    let with_variable: Value = serde_json::from_slice(&with_variable.stdout).unwrap();
    assert!(with_variable["settings"].get("train_lanes").is_none(), "{with_variable}");
    assert!(world.checkout_list()["settings"].get("train_lanes").is_none());
    let changed_live = with_variable;
    assert_eq!(changed_live["instance"], before, "settings change no fleet instance");
    assert_eq!(changed_live["host_runner"]["started"], false);
    let back = world.afk(&["run", "--exclusive-seconds", "default"]).ok();
    assert_eq!(back["settings"]["exclusive_seconds"], json!({"from": 1500, "to": null}));
    let defaulted = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(defaulted["host"]["settings"]["in_force"]["exclusive_seconds"], 1200);
    // what it says none of, its default worked out for this host
    assert_eq!((&said["admit_load"], &in_force["admit_load"]), (&Value::Null, &json!(200)));
    assert_eq!(said["hot_written"], Value::Null);
    assert!(in_force["hot_written"].as_f64().unwrap() >= 1.0, "{settings}");
    assert!(in_force.get("no_host_sleep").is_none(), "{settings}");
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
    let before = world.checkout_list();
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
    assert_eq!(world.checkout_list(), before);
    assert_eq!(world.runner_log(), [] as [String; 0]);
    assert!(!world.cache().join("afkconvoy/runs").exists(), "no tick log");
    newer.kill().unwrap();
    newer.wait().unwrap();
}

/// An afkconvoy installed in `world` as Homebrew installs one: a link,
/// `installed/afkconvoy`, to the one built → the link, which a `run` is
/// started as (`World::afk_as`).
pub(crate) fn installed(world: &World) -> PathBuf {
    let link = world.root.join("installed/afkconvoy");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_afkconvoy"), &link).unwrap();
    link
}

/// Installs another afkconvoy at `link`, as `brew upgrade` does: the link is
/// pointed at a file of its own, which says `version` to `--version`, and
/// runs `otherwise` for anything else.
pub(crate) fn upgraded(link: &Path, version: &str, otherwise: &str) {
    let file = link.parent().unwrap().join(format!("../cellar/{version}/afkconvoy"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'afkconvoy {version}'; exit 0; fi\n\
         {otherwise}\n"
    );
    std::fs::write(&file, script).unwrap();
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let new = link.with_extension("new");
    std::os::unix::fs::symlink(&file, &new).unwrap();
    std::fs::rename(new, link).unwrap();
}

/// What the afkconvoy built is, for anything but `--version`.
pub(crate) fn as_built() -> String {
    format!("exec {} \"$@\"", env!("CARGO_BIN_EXE_afkconvoy"))
}

/// Waits up to `PATIENCE` for `got` to give something → it.
fn until<T>(what: &str, mut got: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + PATIENCE;
    loop {
        if let Some(got) = got() {
            return got;
        }
        assert!(Instant::now() < until, "no {what} within {PATIENCE:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What `doctor` says of the afkconvoy installed where the host runner was
/// started from: `host.installed`.
fn installed_said(world: &World) -> Value {
    world.afk(&["doctor", "--host", "h1"]).json["host"]["installed"].clone()
}

#[test]
fn the_host_runner_moves_onto_a_newer_afkconvoy_installed_where_it_was_started_from() {
    let world = World::new();
    let link = installed(&world);
    let clone2 = world.second_repo();
    let second = world.afk_as(&link, &clone2, &RUN).ok();
    let first = world.afk_as(&link, &world.clone, &RUN).ok();
    let old = first["host_runner"]["pid"].as_u64().unwrap();
    let list = world.checkout_list();

    upgraded(&link, "999.0.0", &as_built());
    let moved = logged(&world, "host runner moved: ");
    let new = world.host_runner().unwrap();
    let (version, at) = (afkconvoy::VERSION, link.display());
    assert_eq!(
        moved,
        format!(
            "host runner moved: from afkconvoy {version} (pid {old}) to afkconvoy {version} \
             (pid {new}), installed at {at}"
        )
    );
    assert_ne!(u64::from(new), old);
    until("end of the host runner that moved", || (!alive(&json!(old))).then_some(()));

    // what it found, then that nothing held it back, then the newer one
    // holding the host on the same list
    let log = world.runner_log();
    let at_line = |start: &str| log.iter().position(|said| said.starts_with(start));
    let found = format!(
        "afkconvoy 999.0.0 is installed at {at}, newer than this host runner's, {version}: it \
         moves onto it between ticks, once no exclusive command runs"
    );
    let started = format!("host runner started: afkconvoy {version}, pid {new}, 2 checkouts");
    let lines = [found.as_str(), "host runner moving: nothing holds it back", &started];
    let lines = lines.map(at_line);
    assert!(lines.iter().all(Option::is_some) && lines.is_sorted(), "{lines:?} in {log:#?}");
    assert_eq!(world.checkout_list(), list);

    // each fleet instance stopped between two ticks, the move a line of its
    // tick log, and the newer one started it again
    for answer in [&first, &second] {
        let starts = || of_kind(&tick_log(answer), "start").len();
        until("start of each fleet instance again", || (starts() == 2).then_some(()));
        let lines = tick_log(answer);
        let moved = of_kind(&lines, "moved");
        assert_eq!(moved.len(), 1, "{lines:#?}");
        let said = serde_json::from_value::<Entry>(moved[0].clone()).unwrap().said();
        let to = format!("from afkconvoy {version} to afkconvoy {version}, installed at {at}");
        assert_eq!(said, format!("moved: the host runner moved {to}"));
        let at = |kind: &str| lines.iter().position(|line| self::kind(line) == kind);
        assert!(at("stop") < at("moved"), "after its fleet instance stopped: {lines:#?}");
    }

    // `checkouts` and `doctor` say the host runner it moved onto
    let checkouts = world.afk(&["checkouts"]).ok();
    assert_eq!(checkouts["host_runner"]["pid"], new, "{checkouts}");
    assert_eq!(checkouts["host_runner"]["version"], version, "{checkouts}");
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let events = doctor["fleet"]["events"].as_array().unwrap();
    assert!(events.iter().any(|event| event["event"] == "moved"), "{doctor}");
}

#[test]
fn a_newer_afkconvoy_that_does_not_start_leaves_the_host_runner_holding_the_host() {
    let world = World::new();
    let link = installed(&world);
    let first = world.afk_as(&link, &world.clone, &RUN).ok();
    let old = first["host_runner"]["pid"].as_u64().unwrap();

    let broken = "dyld: Library not loaded: libgit2.1.9.dylib";
    upgraded(&link, "999.0.0", &format!("echo '{broken}' >&2; exit 1"));
    let not_moved = logged(&world, "host runner not moved: ");
    let (version, at) = (afkconvoy::VERSION, link.display());
    // what it said as it ended, and nothing of this host runner's own
    let said = format!(
        "host runner not moved: afkconvoy 999.0.0, installed at {at}, did not start: it ended as \
         it started (exit status: 1): {broken} — it takes the host back, on afkconvoy {version}, \
         and tries that file no more until it changes"
    );
    assert_eq!(not_moved, said);

    // the host runner takes the host back, and starts its fleet instance again
    logged(&world, &format!("host runner started: afkconvoy {version}, pid {old}, 1 checkouts"));
    assert_eq!(world.host_runner().map(u64::from), Some(old));
    let starts = || of_kind(&tick_log(&first), "start").len();
    until("start of the fleet instance again", || (starts() == 2).then_some(()));
    let lines = tick_log(&first);
    let not_moved = of_kind(&lines, "not_moved");
    assert_eq!(not_moved.len(), 1, "{lines:#?}");
    let said = serde_json::from_value::<Entry>(not_moved[0].clone()).unwrap().said();
    let start = format!("not moved: afkconvoy 999.0.0, installed at {at}, did not start: ");
    assert!(said.starts_with(&start), "{said}");
    assert!(said.ends_with(&format!("· the host runner stays on afkconvoy {version}")), "{said}");

    // `doctor` gives a finding that names it, as long as the host runner
    // works on
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let findings = doctor["findings"].as_array().unwrap();
    let what = format!(
        "the host runner did not move onto afkconvoy 999.0.0, installed at {at}: it did not \
         start, and the host runner works on, of afkconvoy {version}"
    );
    let found = findings.iter().find(|found| found["what"] == what.as_str());
    let found = found.unwrap_or_else(|| panic!("no {what:?} in {doctor}"));
    assert_eq!(found["severity"], "warning", "{found}");
    assert!(found["evidence"].as_str().unwrap().contains(broken), "{found}");
    assert_eq!(found["fix"], format!("{at} --version"));
    let installed = &doctor["host"]["installed"];
    assert_eq!(installed["version"], "999.0.0", "{doctor}");
    assert_eq!(installed["from"], version, "{doctor}");
    assert!(installed["failed"].as_str().unwrap().contains(broken), "{doctor}");
    let events = doctor["fleet"]["events"].as_array().unwrap();
    assert!(events.iter().any(|event| event["event"] == "not_moved"), "{doctor}");

    // and it tries that file no more until it changes
    std::thread::sleep(Duration::from_secs(3));
    let log = world.runner_log();
    let moving = log.iter().filter(|said| said.starts_with("host runner moving: ")).count();
    assert_eq!(moving, 1, "{log:#?}");
}

#[test]
fn the_host_runner_never_moves_onto_an_older_afkconvoy_nor_one_that_does_not_say_its_version() {
    let world = World::new();
    let link = installed(&world);
    let first = world.afk_as(&link, &world.clone, &RUN).ok();
    let old = first["host_runner"]["pid"].as_u64().unwrap();
    let (version, at) = (afkconvoy::VERSION, link.display());

    upgraded(&link, "0.0.1", &as_built());
    logged(
        &world,
        &format!(
            "afkconvoy 0.0.1 is installed at {at}, older than this host runner's, {version}: it \
             never moves onto an older one"
        ),
    );
    assert_eq!(installed_said(&world), Value::Null);

    // one that says no version is a finding, and no move either
    upgraded(&link, "says nothing", "exit 1");
    let unsaid = format!(
        "the host runner did not move onto the afkconvoy installed at {at}: it does not say its \
         version, and the host runner works on, of afkconvoy {version}"
    );
    let found = until("finding of an afkconvoy that says no version", || {
        let doctor = world.afk(&["doctor", "--host", "h1"]).json;
        let findings = doctor["findings"].as_array().unwrap().clone();
        findings.into_iter().find(|found| found["what"] == unsaid.as_str())
    });
    let evidence = found["evidence"].as_str().unwrap();
    assert!(evidence.contains("`--version` said \"afkconvoy says nothing\""), "{found}");
    logged(&world, &format!("{at}, which this host runner was started from, changed and does not"));

    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(world.host_runner().map(u64::from), Some(old));
    let log = world.runner_log();
    assert!(!log.iter().any(|said| said.starts_with("host runner moving: ")), "{log:#?}");
    assert_eq!(of_kind(&tick_log(&first), "start").len(), 1);
}

#[test]
fn the_host_runner_moves_with_claims_and_an_open_pr_after_only_the_running_command_ends() {
    let world = World::new();
    world.set_gate("true");
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    let link = installed(&world);
    let first = world.afk_as(&link, &world.clone, &TICKING).ok();
    let old = first["host_runner"]["pid"].as_u64().unwrap();
    assert!(ticked(&first, "dispatched #7").contains("holding 2"));

    // Keep the runner here while a worker joins its PR and another command
    // queues. The queued command must run only after the replacement starts.
    // The running command ends with the world if the test fails before `go`.
    let go = world.root.join("go");
    let (at, root) = (go.display(), world.root.display());
    let script = format!("while [ ! -e '{at}' ] && [ -d '{root}' ]; do sleep 0.05; done");
    let mut exclusive = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["exclusive", "--", "sh", "-c", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    until("exclusive command started", || {
        let said = world.afk(&["doctor", "--host", "h1"]).json;
        said["host"]["exclusive"]["running"][0]["ran"].is_u64().then_some(())
    });
    let worker = world
        .worktrees()
        .into_iter()
        .find(|path| {
            world.orca()["worktrees"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["path"] == json!(path) && row["linkedIssue"] == 7)
        })
        .unwrap();
    commit(&worker, "finished.txt", "done\n");
    git(&worker, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&worker, &["symbolic-ref", "--short", "HEAD"]));
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let terminals = world.orca()["terminals"].clone();
    let ran = world.root.join("queued-ran");
    let queued_script = format!(
        "cp '{}' '{}'",
        world.cache().join("afkconvoy/host-runner.lock").display(),
        ran.display()
    );
    let mut queued = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["exclusive", "--", "sh", "-c", &queued_script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    until("queued command", || {
        let said = world.afk(&["doctor", "--host", "h1"]).json;
        (said["host"]["exclusive"]["waiting"].as_array().is_some_and(|queue| queue.len() == 1))
            .then_some(())
    });
    upgraded(&link, "999.0.0", &as_built());
    until("exclusive command holding the move back alone", || {
        let held = installed_said(&world)["held_back"].clone();
        (held == json!(["1 exclusive command(s) running"])).then_some(())
    });
    assert_eq!(world.host_runner().map(u64::from), Some(old));
    let log = world.runner_log();
    assert!(!log.iter().any(|said| said.starts_with("host runner moving: ")), "{log:#?}");

    std::fs::write(&go, "").unwrap();
    assert!(exclusive.wait().unwrap().success());
    logged(&world, "host runner moved: ");
    assert_ne!(world.host_runner().map(u64::from), Some(old));
    assert!(queued.wait().unwrap().success());
    let ran_under: Value = serde_json::from_str(&std::fs::read_to_string(&ran).unwrap()).unwrap();
    assert_eq!(
        ran_under["pid"].as_u64(),
        world.host_runner().map(u64::from),
        "the waiter runs under the replacement: {ran_under}"
    );
    until("replacement first tick", || {
        (of_kind(&tick_log(&first), "start").len() == 2
            && of_kind(&tick_log(&first), "tick").len() >= 2)
            .then_some(())
    });
    let claims = world.afk(&["takeover", "--list", "--host", "h1"]).ok();
    let instances = claims["instances"].as_array().unwrap();
    let held: Vec<_> =
        instances.iter().flat_map(|instance| instance["claims"].as_array().unwrap()).collect();
    assert!(held.contains(&&json!(8)), "{claims}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
    for terminal in terminals.as_array().unwrap() {
        assert!(
            world.orca()["terminals"].as_array().unwrap().contains(terminal),
            "worker terminal changed"
        );
    }
    let moved = of_kind(&tick_log(&first), "moved");
    let carried = moved[0]["moved"]["carried"].to_string();
    assert!(
        carried.contains("claim #8 and its worker") && carried.contains("claim #7 and its worker"),
        "{moved:#?}"
    );
}

/// Waits up to `PATIENCE` for the host runner's log to hold `count` lines
/// that start with `start` → the first `count` of them.
fn logged_times(world: &World, start: &str, count: usize) -> Vec<String> {
    let until = Instant::now() + PATIENCE;
    loop {
        let log = world.runner_log();
        let lines: Vec<String> = log.into_iter().filter(|line| line.starts_with(start)).collect();
        if lines.len() >= count {
            return lines[..count].to_vec();
        }
        assert!(Instant::now() < until, "not {count} {start:?} in {:#?}", world.runner_log());
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The lines of a tick log of one kind, each as it is kept.
fn of_kind(lines: &[Value], of: &str) -> Vec<Value> {
    lines.iter().filter(|line| kind(line) == of).cloned().collect()
}

#[test]
fn a_fleet_instance_that_panics_breaks_alone_and_is_started_again_after_a_backoff_that_grows() {
    let world = World::new();
    let clone2 = world.second_repo();
    let other = world.afk_in(&clone2, &RUN).ok();
    // its first tick answers this `run`, and each start of it panics as its
    // second tick ends
    let panics = ["run", "--host", "h1", "--second-ms", "10", "--panic-at-tick", "2"];
    let first = world.afk(&panics).ok();
    let pid = &first["host_runner"]["pid"];
    assert_eq!(pid, &other["host_runner"]["pid"], "one host runner holds both");
    let clone = shown(&world.clone);

    // broken, with why and since when, and back again — then broken again
    // soon after, waiting twice as long
    let why = "it panicked: told to panic as tick 2 ended (--panic-at-tick)";
    let breaks =
        logged_times(&world, &format!("fleet instance broken: {clone}: {why} · since "), 2);
    assert!(breaks[0].ends_with(" UTC, in 60s"), "{breaks:#?}");
    assert!(breaks[1].ends_with(" UTC, in 120s"), "{breaks:#?}");
    let back = logged_times(&world, &format!("fleet instance back: {clone}: "), 1);
    assert!(back[0].contains(", broken since "), "{back:#?}");
    let log = world.runner_log();
    let at = |line: &str| log.iter().position(|said| said == line).unwrap();
    assert!(at(&breaks[0]) < at(&back[0]) && at(&back[0]) < at(&breaks[1]), "{log:#?}");
    // the panic is a line of the host runner's log of its own, stamped
    let panicked = log.iter().find(|line| line.starts_with("panicked at "));
    let panicked = panicked.unwrap_or_else(|| panic!("{log:#?}"));
    assert!(panicked.ends_with("told to panic as tick 2 ended (--panic-at-tick)"), "{panicked}");
    each_line_stamped(&world);

    // its tick log says so too: the instance that broke, and the one back
    let lines = tick_log(&first);
    let broken = of_kind(&lines, "broken");
    assert!(broken.len() >= 2, "{lines:#?}");
    assert_eq!(broken[0]["instance"], first["instance"], "the one that broke");
    assert_eq!(broken[0]["broken"]["why"], why);
    let (since, next) = (&broken[0]["broken"]["since"], &broken[0]["broken"]["next"]);
    assert_eq!(next.as_u64().unwrap() - since.as_u64().unwrap(), 60, "{lines:#?}");
    let back = of_kind(&lines, "back");
    assert_eq!(back[0]["back"]["since"], *since, "{lines:#?}");
    assert_ne!(back[0]["instance"], first["instance"], "a start mints its own: {lines:#?}");
    assert!(of_kind(&lines, "start").len() >= 2, "started again: {lines:#?}");

    // the host runner and the other fleet instance went on, the list as it was
    assert!(alive(pid));
    assert_eq!(world.host_runner(), pid.as_u64().map(|pid| pid as u32));
    assert_eq!(listed(&world), [shown(&clone2), clone.clone()]);
    let ticked = |answer: &Value| of_kind(&tick_log(answer), "tick").len();
    let before = ticked(&other);
    let until = Instant::now() + PATIENCE;
    while ticked(&other) == before {
        assert!(Instant::now() < until, "the other ticks on: {:#?}", tick_log(&other));
        std::thread::sleep(Duration::from_millis(50));
    }
    let others = tick_log(&other);
    assert_eq!(of_kind(&others, "start").len(), 1, "never broken: {others:#?}");
    assert_eq!(of_kind(&others, "broken"), [] as [Value; 0]);
    assert!(
        others
            .iter()
            .filter(|line| line["instance"] != "host-watch")
            .all(|line| line["instance"] == other["instance"]),
        "{others:#?}"
    );
}

#[test]
fn a_listed_checkout_whose_probe_no_longer_passes_is_broken_and_doctor_says_when_it_is_tried() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    world.afk_in(&clone2, &RUN).ok();
    world.kill_host_runner("KILL");
    base_branch_gone(&world);

    // the other checkout's `run` starts a host runner, which starts every
    // checkout on the list: this one is broken, and tried again in a minute
    let other = world.afk_in(&clone2, &["run"]).ok();
    let clone = shown(&world.clone);
    let line = logged(&world, &format!("fleet instance broken: {clone}: "));
    let why = afkconvoy_core::layout::no_base_branch("release");
    assert!(line.contains(&why) && line.ends_with(" UTC, in 60s"), "{line}");
    assert_eq!(listed(&world), [clone.clone(), shown(&clone2)], "it stays on the list");
    assert!(alive(&other["host_runner"]["pid"]));
    let lines = tick_log(&first);
    let broken = of_kind(&lines, "broken");
    assert_eq!(broken.len(), 1, "{lines:#?}");
    assert!(broken[0]["broken"]["why"].as_str().unwrap().contains(&why), "{lines:#?}");

    // `doctor` of that checkout says it is broken, why, and when it is tried
    let doctor = world.afk(&["doctor", "--host", "h1"]);
    assert_eq!(doctor.code, 1, "a critical finding: {}", doctor.json);
    let read = &doctor.json["fleet"]["broken"];
    assert_eq!(read["since"], broken[0]["broken"]["since"], "{read}");
    assert_eq!(read["next"], broken[0]["broken"]["next"], "{read}");
    assert!(read["why"].as_str().unwrap().contains(&why), "{read}");
    let findings = doctor.json["findings"].as_array().unwrap();
    let found = findings.iter().find(|finding| {
        finding["what"]
            .as_str()
            .unwrap()
            .starts_with("the fleet instance of this checkout is broken: ")
    });
    let found = found.unwrap_or_else(|| panic!("{findings:#?}"));
    assert_eq!(found["severity"], "critical");
    assert!(found["what"].as_str().unwrap().contains(&why), "{found}");
    let evidence = found["evidence"].as_str().unwrap();
    assert!(evidence.contains("; the host runner starts it again at "), "{evidence}");
    assert_eq!(found["fix"], format!("afkconvoy run '{}'", world.clone.display()));
    let alive_or_not = findings
        .iter()
        .any(|finding| finding["what"].as_str().unwrap().starts_with("no `afkconvoy run`"));
    assert!(!alive_or_not, "said as broken, not as dead: {findings:#?}");
    // a wake reaches it no more: it is told why
    let woke = world.afk(&["wake"]).ok();
    assert_eq!(woke["woke"], false, "{woke}");
    let detail = woke["detail"].as_str().unwrap();
    assert!(detail.contains("is broken, and waits to be started again"), "{detail}");

    // put right, a `run` there starts it at once, and it is back
    commit(&world.clone, "docs/agents/afkconvoy.md", common::CONFIG);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    let again = world.afk(&["run"]).ok();
    assert_eq!(again["added"], false, "{again}");
    assert_eq!(again["host_runner"]["pid"], other["host_runner"]["pid"], "{again}");
    let instance = again["instance"].as_str().unwrap();
    logged(&world, &format!("fleet instance back: {clone}: {instance}, broken since "));
    assert_eq!(of_kind(&tick_log(&first), "back").len(), 1);
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(doctor["fleet"]["broken"], Value::Null, "{doctor}");
}

#[test]
fn stop_takes_a_broken_checkout_off_the_list_at_once_and_it_is_broken_no_more() {
    let world = World::new();
    let first = world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    world.afk_in(&clone2, &RUN).ok();
    world.kill_host_runner("KILL");
    base_branch_gone(&world);
    let other = world.afk_in(&clone2, &["run"]).ok();
    let clone = shown(&world.clone);
    logged(&world, &format!("fleet instance broken: {clone}: "));

    // it answers without waiting out the minute its backoff has left
    let asked = Instant::now();
    let stopped = world.afk(&["stop"]).ok();
    assert!(asked.elapsed() < Duration::from_secs(30), "{:?}", asked.elapsed());
    assert_eq!(stopped["removed"], true, "{stopped}");
    let why = "it was broken, and `afkconvoy stop` took its checkout off the list";
    assert_eq!(stopped["stop"], format!("stop: {why}"), "{stopped}");
    assert_eq!(listed(&world), [shown(&clone2)]);
    logged(&world, &format!("fleet instance ended: {clone}: {why}"));
    assert_eq!(kind(tick_log(&first).last().unwrap()), "stop");
    assert!(alive(&other["host_runner"]["pid"]));

    // what is off the list is no longer said to be broken
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(doctor["fleet"]["broken"], Value::Null, "{doctor}");
}

/// The findings of a `doctor` answer that say a concurrency is still named,
/// which is ignored.
fn retired_concurrency(doctor: &Value) -> Vec<Value> {
    let found = doctor["findings"].as_array().unwrap().iter();
    let about = |finding: &&Value| finding["what"].as_str().unwrap().contains("which is ignored");
    found.filter(about).cloned().collect()
}

#[test]
fn a_concurrency_the_repo_config_names_changes_nothing_and_doctor_says_to_delete_it() {
    let world = World::new();
    let file = world.clone.join("docs/agents/afkconvoy.md");
    let config = |more: &str| {
        let yaml = "base_branch: main\ngate_command: ./gate.sh\ngate_seconds: 600\n";
        std::fs::write(&file, format!("```yaml\n{yaml}{more}```\n")).unwrap();
    };
    config("concurrency: 2\n");
    for issue in [7, 8, 9, 10] {
        world.issue(issue, &["ready-for-agent"]);
    }
    // no number bounds the fleet: its first tick starts the whole frontier
    let first = world.afk(&TICKING).ok();
    let first_tick = first["first_tick"].as_str().unwrap();
    assert!(first_tick.contains("holding 4 · frontier 0"), "{first}");

    // `doctor` explains it: nothing is held up, and the key is to delete
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let found = retired_concurrency(&doctor);
    assert_eq!(found.len(), 1, "{doctor}");
    assert_eq!(found[0]["severity"], "info");
    let what = found[0]["what"].as_str().unwrap();
    assert!(what.contains("names `concurrency: 2`"), "{what}");
    assert!(what.contains("admission alone decides each start"), "{what}");
    assert_eq!(found[0]["fix"], "docs/agents/afkconvoy.md: concurrency", "{doctor}");
    config("");
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(retired_concurrency(&doctor), Vec::<Value>::new(), "{doctor}");
    assert_eq!(doctor["fleet"].get("concurrency"), None, "{doctor}");
}

#[test]
fn a_run_that_names_a_concurrency_is_accepted_does_nothing_and_doctor_says_so() {
    let world = World::new();
    for issue in [7, 8, 9, 10] {
        world.issue(issue, &["ready-for-agent"]);
    }
    // accepted, and ignored: the first tick starts the whole frontier
    let first = world.afk(&[&RUN[..], &["--concurrency", "1"]].concat()).ok();
    assert!(first["first_tick"].as_str().unwrap().contains("holding 4 · frontier 0"), "{first}");
    assert_eq!(first["concurrency"]["ignored"], 1, "{first}");
    assert!(first["concurrency"]["why"].as_str().unwrap().contains("admission alone"), "{first}");
    assert_eq!(world.checkout_list()["checkouts"][0]["concurrency"], 1);

    // `doctor` says it is ignored, with the line that drops it
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let found = retired_concurrency(&doctor);
    assert_eq!(found.len(), 1, "{doctor}");
    assert_eq!(found[0]["severity"], "info");
    let line = format!("afkconvoy run {} --concurrency default", shown(&world.clone));
    assert_eq!(found[0]["fix"], line, "{doctor}");
    // `plan` and `checkouts` say no concurrency at all
    assert_eq!(world.afk(&["plan"]).ok().get("concurrency"), None);
    assert_eq!(world.afk(&["checkouts"]).ok()["checkouts"][0].get("concurrency"), None);

    // that line, run as it is said, drops it
    let words: Vec<&str> = line.split(' ').skip(1).collect();
    let dropped = world.afk(&words).ok();
    assert_eq!(dropped["concurrency"]["ignored"], "default", "{dropped}");
    assert_eq!(world.checkout_list()["checkouts"][0].get("concurrency"), None);
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(retired_concurrency(&doctor), Vec::<Value>::new(), "{doctor}");

    // what is no whole number of at least one is still refused
    for wrong in ["0", "1.5"] {
        let error = world.afk(&["run", "--concurrency", wrong]).error();
        let taken = format!("`{wrong}` is not a whole number of at least 1, nor `default`");
        assert!(error.contains(&taken), "{error}");
    }
}

#[test]
fn a_running_fleet_uses_commands_and_codex_preferences_changed_after_it_started() {
    let world = World::new();
    world.shell_help(Some(
        "--add-dir --config --model --dangerously-bypass-hook-trust --setting-sources --settings",
    ));
    let ran = world.afk(&RUN).ok();
    let pid = ran["host_runner"]["pid"].clone();
    for (number, command, model, effort, service_tier) in [
        (7, "agent --added-codex", "user-first", "xhigh", "default"),
        (8, "agent --changed-codex", "user-next", "high", "priority"),
    ] {
        world.kit_command("codex", command);
        world.kit_models("codex", &["fleet-model", model]);
        common::write(
            &world.home(),
            ".codex/config.toml",
            &format!(
                "model = '{model}'\nmodel_reasoning_effort = '{effort}'\nservice_tier = '{service_tier}'\n"
            ),
        );
        world.afk(&["switch", "--harness", "codex"]).ok();
        world.issue(number, &["ready-for-agent", "harness:codex"]);
        world.afk(&["wake"]).ok();
        let until = Instant::now() + PATIENCE;
        loop {
            if world.orca()["terminals"].as_array().unwrap().iter().any(|terminal| {
                terminal["command"].as_str().is_some_and(|line| {
                    if !line.contains(&format!(" && {command} ")) {
                        return false;
                    }
                    assert!(line.contains(&format!("--model '{model}'")), "{line}");
                    assert!(
                        line.contains(&format!("model_reasoning_effort=\"{effort}\"")),
                        "{line}"
                    );
                    assert!(line.contains(&format!("service_tier=\"{service_tier}\"")), "{line}");
                    true
                })
            }) {
                break;
            }
            assert!(Instant::now() < until, "{command} never started: {:#?}", tick_log(&ran));
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(world.host_runner(), pid.as_u64().map(|pid| pid as u32));
    }
}

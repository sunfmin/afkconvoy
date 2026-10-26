//! `afkconvoy exclusive`: a heavy command runs alone on its host, across the
//! fleets of every repo there, in the queue its host runner holds.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::{World, git};
use serde_json::json;

#[test]
fn a_command_is_ended_after_contended_time_and_the_next_runs_after_its_descendants_end() {
    let world = World::new();
    world.say_host_settings(json!({"exclusive_seconds": 1}));
    let _host_runner = world.host_held();
    let cache = world.cache();
    let held = exclusive(
        &world,
        &cache,
        11,
        "trap '' TERM\nsleep 10 & echo $! > \"$CACHE/descendant\"; wait",
    );
    until(|| cache.join("descendant").exists());
    // A late waiter must receive the whole bound: time alone did not count.
    std::thread::sleep(Duration::from_millis(1200));
    let asked = Instant::now();
    let waiting = exclusive(&world, &cache, 22, "echo next");
    let held = held.wait_with_output().unwrap();
    let waiting = waiting.wait_with_output().unwrap();
    assert!(asked.elapsed() >= Duration::from_secs(1));
    assert_eq!(held.status.code(), Some(124));
    let stderr = text(&held.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("[afkconvoy] exclusive: ended at the exclusive bound"), "{stderr}");
    assert!(
        stderr.contains("run each step of the work as its own `afkconvoy exclusive`"),
        "{stderr}"
    );
    assert!(waiting.status.success(), "{}", text(&waiting.stderr));
    assert_eq!(waiting.stdout, b"next\n");
    let pid = std::fs::read_to_string(cache.join("descendant")).unwrap();
    let state = Command::new("ps").args(["-p", pid.trim(), "-o", "stat="]).output().unwrap();
    let state = text(&state.stdout);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "descendant still runs: {state}"
    );
    until(|| tickets(&world) == 0);
    let said = logged(&cache);
    let bounded: Vec<_> =
        said.iter().filter(|line| line.contains("ended at the exclusive bound")).collect();
    assert_eq!(bounded.len(), 1, "{said:?}");
    assert!(bounded[0].contains("with 1 commands waiting"), "{bounded:?}");
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let found: Vec<_> = doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| {
            finding["what"].as_str().unwrap().contains("ended at the exclusive bound")
        })
        .collect();
    assert_eq!(found.len(), 1, "{doctor}");
    assert_eq!(found[0]["severity"], "warning");
    assert!(found[0]["evidence"].as_str().unwrap().contains("with 1 commands waiting"));
}

#[test]
fn a_long_command_with_nothing_waiting_finishes_and_a_live_setting_changes_the_bound() {
    let world = World::new();
    world.say_host_settings(json!({"exclusive_seconds": 0}));
    let _host_runner = world.host_held();
    let cache = world.cache();
    let alone = exclusive(&world, &cache, 11, "sleep 1.2; echo finished");
    let out = alone.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(out.stdout, b"finished\n");
    assert!(out.stderr.is_empty());
    world.say_host_settings(json!({"exclusive_seconds": 60}));
    let held = exclusive(&world, &cache, 12, "touch \"$CACHE/held\"; sleep 30");
    until(|| cache.join("held").exists());
    let next = exclusive(&world, &cache, 13, "echo next");
    until(|| tickets(&world) == 2);
    world.say_host_settings(json!({"exclusive_seconds": 0}));
    assert_eq!(held.wait_with_output().unwrap().status.code(), Some(124));
    assert!(next.wait_with_output().unwrap().status.success());
    let child_code = exclusive(&world, &cache, 14, "exit 124").wait_with_output().unwrap();
    assert_eq!(child_code.status.code(), Some(1), "124 is reserved for the bound");
}

#[test]
fn a_gate_timeout_still_ends_a_nested_exclusive_command_and_its_descendants() {
    let world = World::new();
    let cache = world.cache();
    let line = afkconvoy_core::line::Line::new()
        .program(env!("CARGO_BIN_EXE_afkconvoy"))
        .fixed(" exclusive -- sh -c ")
        .word("sleep 30 & echo $! > descendant; wait")
        .own();
    world.set_gate_config(&format!("gate_command: {line}\ngate_seconds: 1"));
    let _host_runner = world.host_held();
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["timed_out"], true, "{answer}");
    let pid = std::fs::read_to_string(world.clone.join("descendant")).unwrap();
    let state = Command::new("ps").args(["-p", pid.trim(), "-o", "stat="]).output().unwrap();
    let state = text(&state.stdout);
    assert!(state.trim().is_empty() || state.trim().starts_with('Z'), "{state}");
    assert!(
        exclusive(&world, &cache, 15, "echo next").wait_with_output().unwrap().status.success()
    );
}

/// `afkconvoy exclusive -- sh -c <script>`, started in `world`'s checkout on
/// the host whose cache is `cache`, as the worker of `issue`. The script
/// names the cache `$CACHE`: a tick log keeps only the start of a long
/// command, and a temporary directory's path may be long.
fn exclusive(world: &World, cache: &Path, issue: u64, script: &str) -> Child {
    world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .env("XDG_CACHE_HOME", cache)
        .env("CACHE", cache)
        .env("AFKCONVOY_ISSUE", issue.to_string())
        .args(["exclusive", "--", "sh", "-c", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// How many commands are in the queue of `world`'s host, running or waiting,
/// as `doctor` says it.
fn tickets(world: &World) -> usize {
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    let exclusive = &doctor["host"]["exclusive"];
    let waiting = exclusive["waiting"].as_array().map_or(0, Vec::len);
    waiting + usize::from(!exclusive["running"].is_null())
}

/// Waits, five seconds at most, until `done`.
fn until(done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(5), "waited too long");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn lines(file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines().map(str::to_string).collect()
}

/// Every command line of every tick log under `cache`.
fn commands(cache: &Path) -> Vec<serde_json::Value> {
    let runs = cache.join("afkconvoy").join("runs");
    let mut said = vec![];
    for entry in std::fs::read_dir(runs).unwrap().flatten() {
        if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
            for line in lines(&entry.path()) {
                let line: serde_json::Value = serde_json::from_str(&line).unwrap();
                if let Some(command) = line.get("command") {
                    said.push(command.clone());
                }
            }
        }
    }
    said
}

/// What every tick log under `cache` says of `exclusive`.
fn logged(cache: &Path) -> Vec<String> {
    commands(cache)
        .into_iter()
        .filter(|command| command["command"] == "exclusive")
        .map(|command| {
            format!("{} {}", command["on"].as_str().unwrap_or_default(), command["outcome"])
        })
        .collect()
}

fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

#[test]
fn an_unbound_worker_s_gate_names_its_issue_in_the_tick_log_and_the_host_s_queue() {
    let world = World::new();
    let binary = env!("CARGO_BIN_EXE_afkconvoy");
    let gate = afkconvoy_core::line::Line::new()
        .program(binary)
        .fixed(" exclusive -- sh -c ")
        .word(
            "touch \"$CACHE/worker-ran\"; \
             while test -d \"$CACHE\" && test ! -f \"$CACHE/worker-release\"; do sleep 0.02; done; \
             test ! -f \"$CACHE/red\"",
        )
        .own();
    world.set_gate(&gate);
    // The command remains a shell string: export the issue for all of it,
    // including a harness reached through a shell function.
    world.kit_command("claude-code", "issue_worker ; true");
    world.issue(12, &["ready-for-agent"]);
    let dispatched =
        world.afk(&["dispatch", "--issue", "12", "--instance", "alpha", "--host", "h1"]).ok();
    let dir = Path::new(dispatched["worktree"].as_str().unwrap());
    let orca = world.orca();
    let terminal = orca["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|terminal| terminal["handle"] == dispatched["terminal"])
        .unwrap();
    let line = terminal["line"].as_str().unwrap();
    let script = format!("issue_worker() {{ \"$AFKCONVOY_BIN\" gate --now 1100; }}; {line}");
    let cache = world.cache();
    let _host_runner = world.host_held();
    let holder = world
        .command(binary, &world.clone)
        .env("CACHE", &cache)
        .args([
            "exclusive",
            "--",
            "sh",
            "-c",
            "while test -d \"$CACHE\" && test ! -f \"$CACHE/holder-release\"; do sleep 0.02; done",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    until(|| tickets(&world) == 1);
    let run = || {
        world
            .command("sh", dir)
            .env("CACHE", &cache)
            .env("AFKCONVOY_BIN", binary)
            .args(["-c", &script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let worker = run();
    until(|| tickets(&world) == 2);
    let shown = world.afk(&["doctor", "--host", "h1"]).json;
    let queue = &shown["host"]["exclusive"];
    assert!(queue["running"]["issue"].is_null(), "{queue}");
    assert_eq!(queue["waiting"][0]["issue"], 12, "{queue}");
    std::fs::write(cache.join("holder-release"), "").unwrap();
    assert!(holder.wait_with_output().unwrap().status.success());
    until(|| cache.join("worker-ran").exists());
    let shown = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(shown["host"]["exclusive"]["running"]["issue"], 12, "{shown}");
    std::fs::write(cache.join("worker-release"), "").unwrap();
    let out = worker.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["status"], "green", "{answer}");
    assert_eq!(answer["recorded"], true, "{answer}");

    std::fs::write(cache.join("red"), "").unwrap();
    let out = run().wait_with_output().unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["status"], "red", "{answer}");
    let said = commands(&cache);
    let gates: Vec<_> = said.iter().filter(|line| line["command"] == "gate").collect();
    assert_eq!(gates.len(), 4, "{gates:?}");
    for gate in &gates {
        assert_eq!(gate["on"], "#12", "{gate}");
    }
    assert_eq!(gates[1]["outcome"], "green");
    assert_eq!(gates[3]["outcome"], "red");
    let said = logged(&cache);
    for outcome in ["waits behind", "runs", "(exit 0)", "(exit 1)"] {
        assert!(
            said.iter().any(|line| line.starts_with("#12 ") && line.contains(outcome)),
            "{said:?}"
        );
    }

    // The same gate run by no issue's worker carries no issue, including
    // its nested exclusive command.
    let before = commands(&cache).len();
    let out =
        world.command(binary, &world.clone).env("CACHE", &cache).arg("gate").output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let said = commands(&cache);
    assert_eq!(said[before..].len(), 4, "{said:?}");
    assert!(said[before..].iter().all(|line| line["on"].is_null()), "{said:?}");
}

#[test]
fn two_commands_of_two_repos_fleets_run_one_after_the_other_in_the_order_they_asked() {
    let (first, second) = (World::new(), World::new());
    let _host_runner = first.host_held();
    let cache = first.cache();
    let order = cache.join("order");
    let say = |what: &str| format!("echo {what} >> \"$CACHE/order\"");
    let held =
        exclusive(&first, &cache, 11, &format!("{}; sleep 1; {}", say("a-start"), say("a-end")));
    until(|| lines(&order).len() == 1);
    let waiting = exclusive(&second, &cache, 22, &format!("{}; {}", say("b-start"), say("b-end")));
    until(|| tickets(&first) == 2);
    let (held, waiting) = (held.wait_with_output().unwrap(), waiting.wait_with_output().unwrap());
    assert!(held.status.success() && waiting.status.success());
    assert_eq!(lines(&order), ["a-start", "a-end", "b-start", "b-end"]);
    let said = text(&waiting.stderr);
    assert!(said.contains("[afkconvoy] exclusive: waits behind 1 exclusive command"), "{said}");
    assert_eq!(text(&held.stderr), "", "one that waits for nobody says nothing");
    assert_eq!(tickets(&first), 0, "each left the queue as it ended");

    // each repo's tick log has a line as its command waited, ran and ended
    let said = logged(&cache);
    let waits = "#22 \"waits behind 1 exclusive command on this host to run `sh -c";
    assert!(said.iter().any(|line| line.starts_with(waits)), "{said:?}");
    for issue in ["#11", "#22"] {
        let runs = format!("{issue} \"runs `sh -c");
        assert!(said.iter().any(|line| line.starts_with(&runs)), "{said:?}");
        let ended = said.iter().any(|line| line.starts_with(issue) && line.contains("(exit 0)"));
        assert!(ended, "{said:?}");
    }
}

#[test]
fn trains_and_cuts_gate_ahead_of_waiting_workers_without_interrupting_what_runs() {
    let (first, second) = (World::new(), World::new());
    let binary = env!("CARGO_BIN_EXE_afkconvoy");
    let gate = afkconvoy_core::line::Line::new()
        .program(binary)
        .fixed(" exclusive -- sh -c ")
        .word("echo \"$MARK\" >> \"$CACHE/order\"")
        .own();
    first.set_gate_config(&format!("gate_command: {gate}\ngate_seconds: 2"));
    second.set_gate(&gate);
    let worker = second.worker(7, "seven.txt", "7\n");
    assert_eq!(second.afk_in(&worker, &["land", "--issue", "7"]).ok()["outcome"], "joined");
    let train = |world: &World, name: &str| {
        let dir = world.root.join(name);
        git(
            &world.clone,
            &["worktree", "add", "-q", "--detach", dir.to_str().unwrap(), "origin/main"],
        );
        dir
    };
    let train_a = train(&first, "convoy-train-2");
    let train_b = train(&second, "convoy-train");
    let cut = train(&first, "afkconvoy-cut-2");
    let cache = first.cache();
    let _host_runner = first.host_held();
    let order = cache.join("order");
    let held = exclusive(
        &first,
        &cache,
        11,
        "echo holder-start >> \"$CACHE/order\"; \
         while test -d \"$CACHE\" && test ! -f \"$CACHE/release\"; do sleep 0.02; done; \
         echo holder-end >> \"$CACHE/order\"",
    );
    until(|| lines(&order) == ["holder-start"]);
    let run = |world: &World, dir: &Path, mark: &str, args: &[&str]| {
        world
            .command(binary, dir)
            .env("XDG_CACHE_HOME", &cache)
            .env("CACHE", &cache)
            .env("MARK", mark)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    // --train on an issue worker's gate only merges what it lands behind;
    // it must not confer the train worker's priority.
    let worker_a = run(&first, &first.clone, "worker-a", &["gate", "--train"]);
    until(|| tickets(&first) == 2);
    let worker_b = exclusive(&second, &cache, 22, "sleep 3; echo worker-b >> \"$CACHE/order\"");
    until(|| tickets(&first) == 3);
    let first_train = run(&first, &train_a, "train-a", &["gate", "--train"]);
    until(|| tickets(&first) == 4);
    let second_train = run(&second, &train_b, "train-b", &["land", "--train"]);
    until(|| tickets(&first) == 5);
    // A cut's gate has only two seconds, less than the three-second worker
    // already queued. It must get the same priority as the train to pass.
    let cutting = run(&first, &cut, "cut", &["gate", "--train"]);
    until(|| tickets(&first) == 6);
    let shown = first.afk(&["doctor", "--host", "h1"]).json;
    let queue = &shown["host"]["exclusive"];
    assert_eq!(queue["running"]["issue"], 11, "{queue}");
    let dirs: Vec<_> = queue["waiting"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| Path::new(t["dir"].as_str().unwrap()).file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(dirs, ["convoy-train-2", "convoy-train", "afkconvoy-cut-2", "clone", "clone"]);
    assert_eq!(lines(&order), ["holder-start"], "priority never interrupts the holder");
    std::fs::write(cache.join("release"), "").unwrap();
    for child in [held, first_train, second_train, cutting, worker_a, worker_b] {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        if let Ok(answer) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            if answer.get("status").is_some() {
                assert_eq!(answer["status"], "green", "{answer}");
                assert_eq!(answer["recorded"], true, "{answer}");
            }
        }
    }
    assert_eq!(
        lines(&order),
        ["holder-start", "holder-end", "train-a", "train-b", "cut", "worker-a", "worker-b"]
    );
    until(|| tickets(&first) == 0);
    let passed: Vec<_> =
        logged(&cache).into_iter().filter(|line| line.contains("runs ahead of")).collect();
    assert_eq!(passed.len(), 3, "{passed:?}");
    assert!(
        passed.iter().all(|line| line.contains("runs ahead of 2 waiting commands")),
        "{passed:?}"
    );
    assert!(passed.iter().any(|line| line.contains("as a cut's")), "{passed:?}");
}

#[test]
fn the_command_s_output_and_exit_status_are_its_own() {
    let world = World::new();
    let _host_runner = world.host_held();
    let cache = world.cache();
    let ran = |script: &str| -> Output {
        exclusive(&world, &cache, 7, script).wait_with_output().unwrap()
    };
    let out = ran("echo out; echo err >&2; exit 3");
    assert_eq!((text(&out.stdout), text(&out.stderr)), ("out\n".into(), "err\n".into()));
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(ran("kill -TERM $$").status.code(), Some(128 + 15));
    let missing = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["exclusive", "--", "no-such-command-anywhere"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(127));
    assert!(text(&missing.stderr).contains("could not be started"));
    assert_eq!(tickets(&world), 0);
}

#[test]
fn with_no_host_runner_or_one_that_cannot_hold_it_the_command_runs_at_once_saying_so() {
    let world = World::new();
    let cache = world.cache();
    let out = exclusive(&world, &cache, 7, "echo out").wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "out\n");
    assert_eq!(
        text(&out.stderr),
        "[afkconvoy] exclusive: runs `sh -c echo out` at once, in no queue: no host runner runs \
         on this host\n"
    );
    let said = logged(&cache);
    let at_once = "#7 \"runs `sh -c echo out` at once, in no queue: no host runner runs";
    assert!(said.iter().any(|line| line.starts_with(at_once)), "{said:?}");

    // an older one, which cannot read the question
    let mut older = world.host_runner_of("0.5.0");
    let out = exclusive(&world, &cache, 7, "echo out").wait_with_output().unwrap();
    older.kill().unwrap();
    older.wait().unwrap();
    assert!(out.status.success());
    let said = text(&out.stderr);
    let cannot = "[afkconvoy] exclusive: runs `sh -c echo out` at once, in no queue: the host \
                  runner cannot hold it: this host runner (afkconvoy 0.5.0) cannot read";
    assert!(said.starts_with(cannot), "{said}");
}

#[test]
fn one_that_waits_as_its_host_runner_ends_waits_in_the_next_one_s_queue() {
    let world = World::new();
    let first = world.host_held();
    let cache = world.cache();
    let order = cache.join("order");
    let say = |what: &str| format!("echo {what} >> \"$CACHE/order\"");
    let running = exclusive(&world, &cache, 1, &format!("{}; sleep 5; {}", say("a"), say("a-end")));
    until(|| lines(&order).len() == 1);
    let waiting = exclusive(&world, &cache, 2, &say("b"));
    until(|| tickets(&world) == 2);

    // the host runner killed: what ran runs on, and the next one, unknown
    // to the host runner after, may run beside it
    world.kill_host_runner("KILL");
    drop(first);
    let _next = world.host_held();
    let waiting = waiting.wait_with_output().unwrap();
    assert!(waiting.status.success());
    let said = text(&waiting.stderr);
    let rejoins = "[afkconvoy] exclusive: the host runner ended as `sh -c echo b >> \"$CACHE/order\"` \
                   waited: it waits in the next one's queue, from when it asked";
    assert!(said.contains(rejoins), "{said}");
    assert!(!said.contains("at once"), "the next host runner placed it: {said}");
    assert!(running.wait_with_output().unwrap().status.success());
    assert_eq!(lines(&order), ["a", "b", "a-end"]);
}

/// Kills the process `child` with SIGKILL: it has no chance to tidy up.
fn kill(child: &mut Child) {
    Command::new("kill").args(["-9", &child.id().to_string()]).status().unwrap();
    child.wait().unwrap();
}

#[test]
fn a_waiter_killed_leaves_the_queue_and_a_holder_that_dies_frees_its_place() {
    let world = World::new();
    let _host_runner = world.host_held();
    let cache = world.cache();
    let pid: PathBuf = cache.join("pid");
    let order = cache.join("order");
    let say = |what: &str| format!("echo {what} >> \"$CACHE/order\"");
    // the holder's command: it runs on past its holder, and is ended at the end
    let mut holder = exclusive(&world, &cache, 1, "echo $$ > \"$CACHE/pid\"; sleep 5");
    until(|| pid.exists() && !lines(&pid).is_empty());
    let mut killed = exclusive(&world, &cache, 2, &say("killed-ran"));
    until(|| tickets(&world) == 2);
    let next = exclusive(&world, &cache, 3, &say("next-ran"));
    until(|| tickets(&world) == 3);

    // a waiter killed: it never runs, and holds up nobody
    kill(&mut killed);
    // the holder killed while its command runs: its place is free
    let started = Instant::now();
    kill(&mut holder);
    let next = next.wait_with_output().unwrap();
    assert!(next.status.success());
    assert!(started.elapsed() < Duration::from_secs(4), "it did not wait for the orphan's sleep");
    assert_eq!(lines(&order), ["next-ran"]);
    assert_eq!(tickets(&world), 0, "what the killed left is gone");
    let orphan = lines(&pid)[0].clone();
    let _ = Command::new("kill").arg(orphan).status();
}

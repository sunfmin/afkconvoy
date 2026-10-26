//! The host runner watches its host: no model while a look finds nothing to
//! act on, and one watch in the operator's directory when one does — kept
//! across a restart of the host runner, continued on another harness where
//! its own gives out, and ended at its hour. The stand-in orca runs no
//! command: a test writes what a watch's session printed to its transcript.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use afkconvoy_core::host_watch::EXITED;
use serde_json::{Value, json};

use crate::common::{World, asked};

/// The operator's own terminal in the operator's directory: no watch ends it.
const OPERATORS: &str = "operators-own";

/// The operator's directory, known to orca as a repo's main checkout, with a
/// terminal of the operator's own open in it.
fn operator(world: &World) -> PathBuf {
    let dir = world.root.join("operator");
    std::fs::create_dir_all(&dir).unwrap();
    world.with_orca(|orca| {
        orca["worktrees"].as_array_mut().unwrap().push(
            json!({"path": dir, "repoId": "operator", "isMainWorktree": true, "isArchived": false}),
        );
        orca["terminals"].as_array_mut().unwrap().push(
            json!({"handle": OPERATORS, "worktreePath": dir, "command": "claude", "sent": [],
                "open": true}),
        );
    });
    dir
}

/// `run` with this host watching itself from `dir`, looking every second.
fn run(world: &World, dir: &Path) -> Value {
    let dir = dir.to_str().unwrap();
    let run = ["run", "--host", "h1", "--second-ms", "100", "--watch-dir", dir];
    world.afk(&[&run[..], &["--watch-every", "1"]].concat()).ok()
}

fn until<T>(world: &World, what: &str, mut got: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(found) = got() {
            return found;
        }
        let log = world.runner_log().join("\n");
        assert!(Instant::now() < until, "no {what} within 60s: {}\n{log}", state(world));
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Pauses `harness` on this host now: its probe is not due for an hour, so it
/// stays paused, held by the host kit or not. `doctor` finds it paused.
fn paused(world: &World, harness: &str, why: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    world.pause(harness, now.as_secs(), why);
}

/// The watch as the host keeps it.
fn state(world: &World) -> Value {
    let file = world.cache().join("afkconvoy/watch/state.json");
    std::fs::read(file).map_or(json!({}), |bytes| serde_json::from_slice(&bytes).unwrap())
}

fn running(world: &World) -> Option<Value> {
    Some(state(world)["running"].clone()).filter(|running| !running.is_null())
}

fn ended(world: &World, outcome: &str) -> Option<Value> {
    Some(state(world)["last"].clone()).filter(|last| last["outcome"] == outcome)
}

/// The line each terminal started for a watch runs, as orca was asked to.
fn starts(world: &World) -> Vec<String> {
    let calls = world.orca_called_with().into_iter().map(|(call, _)| call);
    let watch = |call: &Vec<String>| call.windows(2).any(|w| w == ["--title", "afkconvoy watch"]);
    let calls = calls.filter(|call| asked(call) == "terminal create" && watch(call));
    let line = |call: Vec<String>| {
        let at = call.iter().position(|arg| arg == "--command").unwrap();
        call[at + 1].clone()
    };
    calls.map(line).collect()
}

/// The terminals open in `dir`, by handle.
fn open(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap().iter();
    let here = terminals.filter(|t| t["open"] == true && t["worktreePath"] == json!(dir));
    here.map(|t| t["handle"].as_str().unwrap().to_string()).collect()
}

/// What the brief a watch's session was pointed at says.
fn brief(line: &str) -> String {
    let at =
        line.find("Your task brief is the file ").unwrap() + "Your task brief is the file ".len();
    let file = &line[at..line[at..].find(" — ").unwrap() + at];
    std::fs::read_to_string(file).unwrap()
}

/// Has the session of `watch` print `lines`, and its harness exit with
/// `status` where it says one: the shell then exits, and its terminal closes.
fn printed(world: &World, watch: &Value, lines: &[&str], status: Option<i32>) {
    let mut text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if let Some(status) = status {
        text.push_str(&format!("{EXITED} {status}\n"));
    }
    std::fs::write(watch["transcript"].as_str().unwrap(), text).unwrap();
    if status.is_some() {
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                if terminal["handle"] == watch["terminal"] {
                    terminal["open"] = json!(false);
                }
            }
        });
    }
}

#[test]
fn a_look_with_nothing_to_act_on_starts_no_model_and_moves_its_until_on() {
    let world = World::new();
    let dir = operator(&world);
    run(&world, &dir);
    let first = until(&world, "first look", || state(&world)["until"].as_u64());
    until(&world, "later look", || state(&world)["until"].as_u64().filter(|until| *until > first));
    assert!(starts(&world).is_empty(), "{:?}", starts(&world));
    assert!(state(&world)["running"].is_null() && state(&world)["last"].is_null());
    assert_eq!(open(&world, &dir), [OPERATORS]);
}

#[test]
fn a_watch_can_fix_a_fleet_fault_and_hand_it_back_while_a_human_decision_stays() {
    let world = World::new();
    let dir = operator(&world);
    let other = world.second_repo();
    world.afk_in(&other, &["run", "--host", "h1"]).ok();
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-human"]);
    world.comment(8, "<!--convoy:escalation claim=old-->\nShould we migrate the customer's data now or wait for approval?");
    world.with_orca(|orca| orca["trust_prompt"] = json!(true));
    let fault = world.afk(&["dispatch", "--issue", "7", "--instance", "alpha", "--host", "h1"]);
    assert!(fault.error().contains("worktree trust gap"));
    // Register both checkouts before enabling watches: a setting is in
    // force before run's first tick puts a new checkout on the list.
    world.afk(&["run", "--host", "h1"]).ok();
    // A watch is independent of the issue worker's trust gap: its launch is
    // non-interactive and already trusted. The fake's gap affects dispatch.
    run(&world, &dir);
    let watch = until(&world, "the watch of the fleet fault", || running(&world));
    let told = brief(&starts(&world)[0]);
    for required in [
        "every repo on this host's checkout list",
        "trust a worktree for its harness",
        "waiting_on_human.answer",
        "actually dispatches or continues",
        "An escalation that asks a human for a decision stays with the human",
        "afkconvoy-watch-<digits>",
        "afkconvoy gate --train",
        "afkconvoy land --inspection",
        "the next idle tick starts a due cut",
        "On the new version repeat the check",
        "never report the fix verified before checking it",
    ] {
        assert!(told.contains(required), "brief lacks {required}: {told}");
    }
    assert!(told.contains(other.to_str().unwrap()), "the look includes the other repo");
    assert!(told.contains("customer's data"), "the human's question is given whole: {told}");

    let doctor = world.afk(&["doctor"]).json;
    let waiting = doctor["waiting_on_human"].as_array().unwrap();
    let answer =
        waiting.iter().find(|item| item["issue"] == 7).unwrap()["answer"].as_str().unwrap();
    // The operator fixes the cause before running precisely the current
    // answer line. Only gh and orca are stood in for; dispatch is real.
    world.with_orca(|orca| orca["trust_prompt"] = json!(false));
    let handed = world.command("sh", &world.clone).args(["-c", answer]).output().unwrap();
    assert!(handed.status.success(), "{}", String::from_utf8_lossy(&handed.stderr));
    world.afk(&["wake"]).ok();
    until(&world, "the recovered issue to dispatch", || {
        let orca = world.orca();
        orca["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|terminal| {
                terminal["open"] == true
                    && terminal["sent"].as_array().is_some_and(|sent| {
                        sent.iter().any(|line| {
                            line["text"].as_str().is_some_and(|line| line.contains("task brief"))
                        })
                    })
            })
            .cloned()
    });
    let human = world.gh()["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["number"] == 8)
        .unwrap()
        .clone();
    assert_eq!(human["labels"], json!(["ready-for-human"]));
    printed(
        &world,
        &watch,
        &[
            "fixed trust and checked #7 took its brief",
            "left #8 with the human: decide when to migrate the customer's data",
        ],
        Some(0),
    );
    let last = until(&world, "the watch's record", || ended(&world, "ended"));
    assert!(last["did"][1].as_str().unwrap().contains("left #8 with the human"));
}

#[test]
fn one_watch_covers_an_episode_is_kept_across_a_restart_and_is_recorded() {
    let world = World::new();
    let dir = operator(&world);
    // Something an operator would act on: a harness the host kit does not
    // hold, paused all the same.
    paused(&world, "pi", "it refused every prompt");
    let first = run(&world, &dir);
    let watch = until(&world, "a watch", || running(&world));
    assert_eq!(watch["harness"], "claude-code");
    let started = starts(&world);
    assert_eq!(started.len(), 1, "{started:?}");
    let line = &started[0];
    assert!(line.contains(" --print "), "non-interactive: {line}");
    assert!(line.contains("--dangerously-skip-permissions"), "trusted: {line}");
    assert!(line.contains(&format!("tee -a '{}'", watch["transcript"].as_str().unwrap())));
    let told = brief(&started[0]);
    assert!(told.contains("the harness `pi` is paused on this host"), "{told}");
    assert!(told.contains("it refused every prompt"), "the look, whole: {told}");
    assert_eq!(
        open(&world, &dir),
        [OPERATORS.to_string(), watch["terminal"].as_str().unwrap().into()]
    );

    // What a look would find while it runs waits for it to end.
    paused(&world, "dsh", "it refused every prompt");
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(starts(&world).len(), 1, "no second watch beside the first");

    world.kill_host_runner("KILL");
    let again = run(&world, &dir);
    assert_ne!(again["host_runner"]["pid"], first["host_runner"]["pid"]);
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(starts(&world).len(), 1, "the running watch is kept, and none started beside it");
    assert_eq!(running(&world).unwrap()["id"], watch["id"]);

    printed(&world, &watch, &["looked at pi", "left dsh for the next look"], Some(0));
    let last = until(&world, "the watch to end", || ended(&world, "ended"));
    assert_eq!(last["id"], watch["id"]);
    assert_eq!(last["did"], json!(["looked at pi", "left dsh for the next look"]));
    assert!(open(&world, &dir).contains(&OPERATORS.to_string()), "the operator's own is kept");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["last"]["id"], watch["id"], "{}", doctor["host"]["watch"]);
    let log = std::fs::read_to_string(first["tick_log"].as_str().unwrap()).unwrap();
    for said in ["watch_started", "looked at pi", "watch_ended"] {
        assert!(log.contains(said), "{said}: {log}");
    }

    // pi's episode goes on, and starts nothing more; dsh's is new, and found
    // by the first look after the watch ended.
    let next = until(&world, "the next watch", || {
        running(&world).filter(|next| next["id"] != watch["id"])
    });
    let items: Vec<&str> =
        next["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].contains("`dsh`"), "{items:?}");
    let doctor = world.afk(&["doctor", "--all"]).json;
    assert_eq!(doctor["host"]["watch"]["running"]["id"], next["id"]);
}

#[test]
fn a_watch_whose_harness_runs_out_goes_on_on_the_next_with_the_same_look() {
    let world = World::new();
    let dir = operator(&world);
    world.hold_every_harness();
    paused(&world, "kimi", "it refused every prompt");
    // The harness this host last started a worker on that took its brief.
    let delivered = world.cache().join("afkconvoy/watch/delivered.json");
    std::fs::create_dir_all(delivered.parent().unwrap()).unwrap();
    std::fs::write(delivered, r#"["codex"]"#).unwrap();
    run(&world, &dir);
    let first = until(&world, "a Codex watch", || running(&world));
    assert_eq!(first["harness"], "codex");
    assert!(starts(&world)[0].contains(" exec "), "{:?}", starts(&world));

    let out = "Usage limit reached; increase your limits to continue using Codex";
    printed(&world, &first, &["checked kimi", out], None);
    let next = until(&world, "the watch on Claude Code", || {
        running(&world).filter(|running| running["harness"] == "claude-code")
    });
    assert_eq!((&next["id"], &next["started"]), (&first["id"], &first["started"]));
    assert_eq!(next["attempts"][0]["harness"], "codex");
    assert_eq!(next["attempts"][0]["did"], json!(["checked kimi", out]));
    let started = starts(&world);
    assert_eq!(started.len(), 2, "{started:?}");
    let told = brief(&started[1]);
    assert!(told.contains("checked kimi") && told.contains("`kimi` is paused"), "{told}");
    let live = open(&world, &dir);
    assert_eq!(live, [OPERATORS.to_string(), next["terminal"].as_str().unwrap().into()]);
    let log = world.runner_log().join("\n");
    assert!(log.contains("continued from codex to claude-code"), "{log}");
    let doctor = world.afk(&["doctor", "--all"]).json;
    let faults = doctor["host"]["host_kit"]["worker_health"]["faults"].as_array().unwrap();
    assert!(
        faults.iter().any(|f| f["harness"] == "codex" && f["scope"] == "account"),
        "{faults:?}"
    );

    // A harness that fails its session, and says nothing of why, gives out too.
    printed(&world, &next, &["API Error: 500"], Some(1));
    let third = until(&world, "the watch on pi", || {
        running(&world).filter(|running| running["harness"] == "pi")
    });
    assert_eq!(third["attempts"][1]["harness"], "claude-code");
    assert_eq!(third["id"], first["id"]);
    printed(&world, &third, &["fixed it"], Some(0));
    let last = until(&world, "the watch to end", || ended(&world, "ended"));
    assert_eq!(last["did"], json!(["checked kimi", out, "API Error: 500", "fixed it"]));
}

#[test]
fn a_watch_is_ended_at_its_hour_and_one_no_harness_can_start_is_a_finding() {
    let world = World::new();
    let dir = operator(&world);
    paused(&world, "pi", "it refused every prompt");
    run(&world, &dir);
    let watch = until(&world, "a watch", || running(&world));
    let file = world.cache().join("afkconvoy/watch/state.json");
    let mut kept = state(&world);
    let hour_ago = kept["running"]["started"].as_u64().unwrap() - 3600;
    kept["running"]["started"] = json!(hour_ago);
    std::fs::write(&file, kept.to_string()).unwrap();
    let last = until(&world, "the watch to reach its hour", || ended(&world, "time_limit"));
    assert_eq!(last["id"], watch["id"]);
    assert_eq!(open(&world, &dir), [OPERATORS], "its own terminal alone is ended");

    paused(&world, "claude-code", "out of tokens");
    let why = until(&world, "no harness to start one", || {
        state(&world)["unavailable"].as_str().map(String::from)
    });
    assert!(why.contains("no harness free of a fault"), "{why}");
    assert_eq!(starts(&world).len(), 1);
    let doctor = world.afk(&["doctor", "--all"]);
    assert_eq!(doctor.code, 1, "{}", doctor.json);
    let findings = doctor.json["findings"].as_array().unwrap();
    assert!(findings.iter().any(|f| f["what"] == "no harness can start this host's watch"));
    assert_eq!(doctor.json["host"]["watch"]["unavailable"], json!(why));
}

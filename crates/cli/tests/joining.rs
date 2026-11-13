use crate::common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{World, commit, git};
use serde_json::{Value, json};

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        // the status boards a tick brings up to date are `board.rs`'s to test
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

fn worktree(world: &World) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == 7).expect("a worktree of issue 7");
    PathBuf::from(row["path"].as_str().unwrap())
}

/// A world whose worker on issue 7, dispatched at 700 — a grace period and
/// more before anything asks after it — pushed its work and opened PR #70,
/// and stopped there, short of the landing train.
fn joining() -> World {
    let world = World::new();
    joining_in(world)
}

fn joining_in(world: World) -> World {
    world.issue(7, &["ready-for-agent", "bug"]);
    assert_eq!(did(&tick(&world, 700)), ["dispatch 7 done", "heartbeat done"]);
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    world
}

#[test]
fn an_idle_unbound_codex_worker_has_its_waiting_turn_asked_again_without_a_harness() {
    let world = World::new();
    world.kit_command("codex", afkconvoy_core::harness::Harness::Codex.stock());
    world.with_kit(|kit| {
        kit["harnesses"].as_array_mut().unwrap().retain(|held| held["harness"] == "codex");
    });
    let world = joining_in(world);
    let dir = worktree(&world);
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    // Let health observe the worker before its recovery judgment is due.
    assert!(did(&tick(&world, 1000)).contains(&"watch 7 nothing".to_string()));
    std::fs::write(
        notes.join("convoy-turn.json"),
        json!({"outcome": "waiting_for_capacity", "at": 1000}).to_string(),
    )
    .unwrap();
    // Read the rollout itself, independent of Orca's agent-state receipt.
    // An unbound worker keeps it in the home recorded at launch.
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = Value::Null;
        }
    });
    std::fs::remove_dir_all(world.home().join(".codex/sessions")).unwrap();
    let home = std::fs::read_to_string(notes.join("convoy-codex-home")).unwrap();
    let sessions = PathBuf::from(home).join("sessions/1970/01/01");
    std::fs::create_dir_all(&sessions).unwrap();
    let rollout = [
        json!({"type": "session_meta", "payload": {"cwd": dir, "source": "cli"}}),
        json!({"type": "event_msg", "timestamp": "1970-01-01T00:16:40Z",
            "payload": {"type": "task_started", "started_at": 1000, "turn_id": "worker"}}),
        json!({"type": "event_msg", "timestamp": "1970-01-01T00:17:30Z",
            "payload": {"type": "task_complete", "started_at": 1000,
                "completed_at": 1050, "turn_id": "worker"}}),
    ];
    std::fs::write(
        sessions.join("rollout-idle.jsonl"),
        rollout.iter().map(Value::to_string).collect::<Vec<_>>().join("\n"),
    )
    .unwrap();
    let terminals = open_terminals(&world, &dir);
    let ticked = tick(&world, 1500);
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(did(&ticked).contains(&"watch 7 asked".to_string()), "{ticked}");
    assert!(notes.join("convoy-turn.log").exists());
    assert_eq!(open_terminals(&world, &dir), terminals);
    let health: Value = serde_json::from_slice(
        &std::fs::read(world.cache().join("afkconvoy/worker-health.json")).unwrap(),
    )
    .unwrap();
    assert!(
        health["workers"].as_array().unwrap().iter().all(|worker| worker["path"] != json!(dir)),
        "{health}"
    );
}

#[test]
fn a_blocked_turn_waits_for_a_blocker_to_close_or_the_dependency_list_to_change() {
    for close in [true, false] {
        let world = joining();
        world.issue(8, &[]);
        world.issue(9, &[]);
        world.with_gh(|gh| gh["edges"]["7"] = json!([8]));
        let dir = worktree(&world);
        let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_050_000});
                terminal["lastOutputAt"] = json!(1_050_000);
            }
        });
        std::fs::write(
            notes.join("convoy-turn.json"),
            json!({"outcome": "blocked", "answer": {"outcome": "blocked", "blocked_by": [8]},
                "dependencies": [{"number": 8}], "at": 1050})
            .to_string(),
        )
        .unwrap();
        for now in 1500..1503 {
            let ticked = tick(&world, now);
            assert_eq!(ticked["errors"], json!([]), "{ticked}");
            assert!(did(&ticked).contains(&"watch 7 nothing".to_string()), "{ticked}");
            assert!(
                !notes.join("convoy-turn.log").exists(),
                "an unchanged blocked turn was asked again"
            );
        }
        if close {
            world.close_issue(8);
        } else {
            world.with_gh(|gh| gh["edges"]["7"] = json!([8, 9]));
        }
        let ticked = tick(&world, 1503);
        assert!(did(&ticked).contains(&"watch 7 asked".to_string()), "close={close}: {ticked}");
        assert!(notes.join("convoy-turn.log").exists(), "the changed blocked turn was not asked");
    }
}

#[test]
fn a_fleet_asked_red_turn_hands_its_answer_to_the_idle_worker_on_the_next_tick() {
    let world = World::new();
    // The base passes; the candidate's change makes its gate fail.
    world.set_gate("if test -f seven.txt; then echo 'FAIL checkout::worker_brief'; exit 1; fi");
    let world = joining_in(world);
    world.list_checkout(&world.clone);
    let dir = worktree(&world);
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    let file = notes.join("convoy-turn.json");
    std::fs::write(&file, json!({"outcome": "waiting_for_capacity", "at": 1000}).to_string())
        .unwrap();
    world.with_orca(|orca| {
        orca["notes"] = json!("taken");
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_050_000});
            terminal["lastOutputAt"] = json!(1_050_000);
        }
    });
    let terminals = open_terminals(&world, &dir);
    let asked = tick(&world, 1500);
    assert!(did(&asked).contains(&"watch 7 asked".to_string()), "{asked}");
    let began = Instant::now();
    loop {
        let note: Value = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if note["outcome"] == "gate_red" {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "the retried turn did not answer gate_red: {note}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let (lines, _) = world.afk_lines(&[
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--second-ms",
        "0",
        "--ticks",
        "1",
        "--now",
        "1800",
    ]);
    assert!(lines[0].contains("told its turn's answer to #7"), "{lines:?}");
    assert!(lines[0].contains("holding 1"), "{lines:?}");
    assert!(!lines[0].contains("nothing to do"), "{lines:?}");
    let sends = turn_answer_lines(&world);
    assert_eq!(sends.len(), 1, "{sends:?}");
    let line = &sends[0];
    for expected in [
        "gate_red",
        "checkout::worker_brief",
        "fix your change",
        "afkconvoy land --issue",
        notes.join("convoy-turn.log").to_str().unwrap(),
    ] {
        assert!(line.contains(expected), "missing {expected}: {line}");
    }
    assert_eq!(open_terminals(&world, &dir), terminals, "the idle worker took the answer");
    world.issue(8, &["ready-for-agent"]);
    let next = tick(&world, 1801);
    assert_eq!(next["frontier_remaining"], 1, "the handed worker holds its slot: {next}");
    assert_eq!(turn_answer_lines(&world).len(), 1, "no duplicate handoff");
    // The worker asks again: its turn replaces the handoff receipt.
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "working", "stateStartedAt": 1_801_000});
        }
    });
    let answer = world.afk_in(&dir, &["land", "--issue", "7"]).ok();
    assert_eq!(answer["outcome"], "gate_red", "{answer}");
    let note: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(note["outcome"], "gate_red", "{note}");
    assert!(note["handoff"].is_null(), "the worker asked this turn: {note}");
    let next = tick(&world, 1802);
    assert_eq!(next["errors"], json!([]), "{next}");
    assert_eq!(turn_answer_lines(&world).len(), 1, "no duplicate handoff");
}

fn turn_answer_lines(world: &World) -> Vec<String> {
    world.orca()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|terminal| terminal["sent"].as_array().into_iter().flatten())
        .filter_map(|sent| sent["text"].as_str())
        .filter(|text| text.contains("turn-answer:"))
        .map(str::to_string)
        .collect()
}

#[test]
fn fleet_asked_conflicts_are_handed_once_even_after_the_fleet_changed_the_tree() {
    for (outcome, gone) in
        [("conflict", false), ("speculative_conflict", false), ("gate_red", true)]
    {
        let world = joining();
        let dir = worktree(&world);
        let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
        // These recent fleet writes used to count as the idle worker's life.
        commit(&dir, "fleet-merge.txt", "the round's tip\n");
        std::fs::write(dir.join("conflicted.txt"), "resolve me\n").unwrap();
        let at =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        std::fs::write(notes.join("convoy-turn.json"), json!({
            "outcome": outcome, "at": at, "handoff": outcome,
            "answer": {"outcome": outcome, "files": ["conflicted.txt"], "excerpt": "FAIL round_test", "round": [{"pr": 70, "issue": 7}]}
        }).to_string()).unwrap();
        world.with_orca(|orca| {
            orca["notes"] = json!("taken");
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_050_000});
                terminal["lastOutputAt"] = json!(1_050_000);
                if gone {
                    terminal["open"] = json!(false);
                }
            }
        });
        let ticked = tick(&world, at);
        assert_eq!(ticked["errors"], json!([]), "{ticked}");
        assert!(did(&ticked).contains(&"watch 7 told".to_string()), "{outcome}: {ticked}");
        let text = if gone { brief(&dir) } else { turn_answer_lines(&world).join("\n") };
        for expected in [outcome, "conflicted.txt", "FAIL round_test", "convoy-turn.log"] {
            assert!(text.contains(expected), "{outcome} missing {expected}: {text}");
        }
        assert_eq!(std::fs::read_to_string(dir.join("conflicted.txt")).unwrap(), "resolve me\n");
        let before = turn_answer_lines(&world).len();
        assert_eq!(tick(&world, at + 1)["errors"], json!([]));
        assert_eq!(turn_answer_lines(&world).len(), before);
    }
}

#[test]
fn an_answer_queued_in_the_harness_is_pending_until_the_model_takes_it() {
    let world = joining();
    let dir = worktree(&world);
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    let file = notes.join("convoy-turn.json");
    std::fs::write(&file, json!({"outcome": "conflict", "at": 1000, "handoff": "queued", "answer": {"files": ["README.md"]}}).to_string()).unwrap();
    world.with_orca(|orca| orca["notes"] = json!("queued"));
    let failed = tick(&world, 1500);
    assert!(!failed["errors"].as_array().unwrap().is_empty(), "{failed}");
    assert!(!notes.join("convoy-turn-told.json").exists(), "queued is not delivered");
    world.with_orca(|orca| orca["notes"] = json!("taken"));
    let told = tick(&world, 1501);
    assert!(did(&told).contains(&"watch 7 told".to_string()), "{told}");
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(notes.join("convoy-turn-told.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["at"], 1501, "{receipt}");
    assert_eq!(receipt["handoff"], "queued", "{receipt}");
}

#[test]
fn doctor_exempts_landing_turn_waits_but_still_reports_workers_that_need_to_change_the_tree() {
    use afkconvoy_core::ending::Ending;

    let world = joining();
    let dir = worktree(&world);
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_050_000});
            terminal["lastOutputAt"] = json!(1_050_000);
        }
    });
    let stopped = || {
        let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1800"]).json;
        assert_eq!(doctor["claims"][0]["status"], "landing", "{doctor}");
        doctor["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "worker_stopped")
    };
    assert!(stopped(), "a worker still preparing its branch can be silent");
    for outcome in Ending::ALL.iter().map(|ending| ending.word()).chain(["error", "unknown"]) {
        std::fs::write(
            notes.join("convoy-turn.json"),
            json!({"outcome": outcome, "at": 1050}).to_string(),
        )
        .unwrap();
        let waits = outcome == "error"
            || Ending::ALL.iter().any(|ending| ending.word() == outcome && ending.waits());
        assert_eq!(stopped(), !waits, "{outcome}");
    }
    std::fs::write(notes.join("convoy-turn.json"), "not a turn note").unwrap();
    assert!(stopped(), "an unreadable note grants no exemption");
}

#[test]
fn doctor_does_not_call_a_landing_worker_stopped_when_the_tick_asks_again() {
    for outcome in ["cancelled", "error"] {
        let world = World::new();
        let started = world.root.join("gate-started");
        let release = world.root.join("release-gate");
        let line = afkconvoy_core::line::Line::new()
            .fixed("touch ")
            .word(&started.to_string_lossy())
            .fixed("; while test -d ")
            .word(&world.root.to_string_lossy())
            .fixed(" && test ! -f ")
            .word(&release.to_string_lossy())
            .fixed("; do sleep 0.02; done")
            .own();
        world.set_gate(&line);
        let world = joining_in(world);
        let dir = worktree(&world);
        let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
        std::fs::write(
            notes.join("convoy-turn.json"),
            json!({"outcome": outcome, "at": 1050}).to_string(),
        )
        .unwrap();
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_050_000});
                terminal["lastOutputAt"] = json!(1_050_000);
            }
        });
        let terminals = open_terminals(&world, &dir);
        let ticked = tick(&world, 1500);
        assert_eq!(ticked["errors"], json!([]), "{outcome}: {ticked}");
        assert!(did(&ticked).contains(&"watch 7 asked".to_string()), "{outcome}: {ticked}");
        let began = Instant::now();
        while !started.exists() {
            assert!(
                began.elapsed() < Duration::from_secs(15),
                "{outcome}: the retried turn did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let asking: Value =
            serde_json::from_slice(&std::fs::read(notes.join("convoy-turn.json")).unwrap())
                .unwrap();
        let doctor = world.afk(&["doctor", "--host", "h1", "--now", "1800"]).json;
        std::fs::write(release, "").unwrap();
        assert!(asking["asking"]["pid"].is_number(), "{outcome}: {asking}");
        assert_eq!(doctor["claims"][0]["status"], "landing", "{outcome}: {doctor}");
        assert!(
            doctor["findings"].as_array().unwrap().iter().all(|f| f["kind"] != "worker_stopped"),
            "{outcome}: {doctor}"
        );
        assert_eq!(
            open_terminals(&world, &dir),
            terminals,
            "{outcome}: no harness was started for the retry"
        );
    }
}

#[test]
fn a_running_landing_turn_is_left_alone_with_its_worker_idle() {
    let world = World::new();
    let started = world.root.join("gate-started");
    let release = world.root.join("release-gate");
    let line = afkconvoy_core::line::Line::new()
        .fixed("touch ")
        .word(&started.to_string_lossy())
        .fixed("; while test -d ")
        .word(&world.root.to_string_lossy())
        .fixed(" && test ! -f ")
        .word(&release.to_string_lossy())
        .fixed("; do sleep 0.02; done")
        .own();
    world.set_gate(&line);
    let world = joining_in(world);
    let dir = worktree(&world);
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    let child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &dir)
        .args(["land", "--issue", "7"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let began = Instant::now();
    while !started.exists() {
        assert!(began.elapsed() < Duration::from_secs(15), "the landing gate did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let asking: Value =
        serde_json::from_slice(&std::fs::read(notes.join("convoy-turn.json")).unwrap()).unwrap();
    let terminals = open_terminals(&world, &dir);
    let ticked = tick(&world, 1100);
    let retried = notes.join("convoy-turn.log").exists();
    let after = open_terminals(&world, &dir);
    std::fs::write(release, "").unwrap();
    let finished = child.wait_with_output().unwrap();
    assert!(finished.status.success(), "{}", String::from_utf8_lossy(&finished.stderr));
    assert!(asking["asking"]["pid"].is_number(), "{asking}");
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(did(&ticked).contains(&"watch 7 nothing".to_string()), "{ticked}");
    assert!(!retried, "a live land process must not be asked for again");
    assert_eq!(after, terminals);
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

fn joins_only(prompt: &str) -> bool {
    prompt.contains("PR #70 is open for it") && !prompt.contains("**Implement**")
}

/// The handles of the terminals open in `dir`.
fn open_terminals(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    here.filter(|t| t["open"] == true).map(|t| t["handle"].as_str().unwrap().to_string()).collect()
}

/// The records of a restart PR #70 carries.
fn restarts(world: &World) -> Vec<String> {
    world
        .comments(70)
        .into_iter()
        .filter(|body| body.contains("convoy:landing") || body.contains("convoy:join"))
        .collect()
}

fn pr_is_open(world: &World) -> bool {
    world.gh()["prs"][0]["state"] == "open"
}

/// A stopped issue worker with a PR and a recorded green gate, ready to join.
fn gated() -> World {
    let world = World::new();
    world.set_gate("true");
    world.issue(7, &["ready-for-agent", "bug"]);
    world.afk_lines(&[
        "run",
        "--foreground",
        "--host",
        "h1",
        "--instance",
        "alpha",
        "--second-ms",
        "0",
        "--ticks",
        "1",
        "--now",
        "700",
    ]);
    let dir = worktree(&world);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    let gate = world.afk_in(&dir, &["gate", "--now", "1040"]).ok();
    assert_eq!((&gate["status"], &gate["recorded"]), (&json!("green"), &json!(true)));
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_050_000});
            terminal["lastOutputAt"] = json!(1_050_000);
        }
    });
    world
}

#[test]
fn a_landed_worker_leaves_health_and_is_settled_without_another_worker() {
    let world = gated();
    let dir = worktree(&world);
    let landed = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1050"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    for now in [1300, 1600, 1900, 2200, 4900, 8500] {
        let ticked = tick(&world, now);
        assert_eq!(ticked["errors"], json!([]), "{ticked}");
        let doctor = world.afk(&["doctor", "--host", "h1", "--now", &now.to_string()]).json;
        let health = &doctor["host"]["host_kit"]["worker_health"];
        assert!(
            health["workers"].as_array().unwrap().iter().all(|w| w["path"] != json!(dir)),
            "{health}"
        );
        assert_eq!(world.claims(), []);
        assert_eq!(world.state(7), "closed");
        assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), landed["commit"]);
        assert!(!world.root.join("wt/convoy-train").exists());
    }
    assert!(!world.root.join("typesafe-request").exists());
}

#[test]
fn a_landing_workers_turn_wait_retires_health_without_an_operator_finding() {
    for outcome in ["round_red", "waiting_for_capacity"] {
        let world = gated();
        world.list_checkout(&world.clone);
        let dir = worktree(&world);
        tick(&world, 1300);
        tick(&world, 1600);
        let path = world.cache().join("afkconvoy/worker-health.json");
        let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(book["workers"][0]["action"], "waiting for operator", "{book}");
        let before = world.afk(&["doctor", "--all", "--now", "1600"]).json;
        assert!(
            before["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "worker_health"),
            "{before}"
        );

        let git_dir = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
        std::fs::write(
            git_dir.join("convoy-turn.json"),
            json!({"outcome": outcome, "at": 1700}).to_string(),
        )
        .unwrap();
        let doctor = world.afk(&["doctor", "--all", "--now", "1800"]).json;
        assert!(
            doctor["findings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|finding| { finding["kind"] != "worker_health" }),
            "{outcome}: {doctor}"
        );
        let workers = &doctor["host"]["host_kit"]["worker_health"]["workers"];
        assert!(
            workers.as_array().unwrap().iter().all(|w| w["path"] != json!(dir)),
            "{outcome}: {workers}"
        );

        tick(&world, 1900);
        let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(book["workers"], json!([]), "{outcome}: {book}");
        assert_eq!(book["observations_failed"], json!([]), "{outcome}: {book}");
        assert!(!world.root.join("typesafe-request").exists());
    }
}

#[test]
fn new_work_restores_health_after_a_landing_turn_wait() {
    let world = gated();
    world.list_checkout(&world.clone);
    let dir = worktree(&world);
    let git_dir = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    let note = git_dir.join("convoy-turn.json");
    for action in ["tell", "continue", "dispatch"] {
        std::fs::write(&note, json!({"outcome": "round_red", "at": 1700}).to_string()).unwrap();
        if action == "tell" {
            world.with_orca(|orca| orca["notes"] = json!("queued"));
            world.afk(&["tell", "7", "--text", "Please revise the change", "--host", "h1"]).error();
            assert!(note.exists(), "an undelivered instruction changes nothing");
            world.with_orca(|orca| orca["notes"] = json!("taken"));
            world.afk(&["tell", "7", "--text", "Please revise the change", "--host", "h1"]).ok();
        } else {
            let mut args = vec![
                "dispatch",
                "--issue",
                "7",
                "--instance",
                "alpha",
                "--host",
                "h1",
                "--now",
                "1800",
            ];
            if action == "dispatch" {
                args.extend(["--start", "fresh"]);
            }
            world.afk(&args).ok();
        }
        assert!(!note.exists(), "{action} forgets the previous turn's wait");
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                terminal["agent"] = json!({"state": "done", "stateStartedAt": 1_850_000});
                terminal["lastOutputAt"] = json!(1_850_000);
            }
        });
        tick(&world, 2200);
        tick(&world, 2500);
        let current = worktree(&world);
        let doctor = world.afk(&["doctor", "--all", "--now", "2500"]).json;
        let workers = &doctor["host"]["host_kit"]["worker_health"]["workers"];
        assert!(
            workers
                .as_array()
                .unwrap()
                .iter()
                .any(|w| { w["path"] == json!(current) && w["action"] == "waiting for operator" }),
            "{action}: {workers}"
        );
        assert!(
            doctor["findings"].as_array().unwrap().iter().any(|f| { f["kind"] == "worker_health" }),
            "{action}: {doctor}"
        );
    }
}

#[test]
fn landing_retires_existing_health_even_when_its_recovery_window_has_expired() {
    for (waiting, next_tick, broken_gitdir) in
        [(false, 1400, false), (true, 2200, false), (true, 5000, true)]
    {
        let world = gated();
        let dir = worktree(&world);
        tick(&world, 1300);
        let path = world.cache().join("afkconvoy/worker-health.json");
        let mut book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(book["workers"][0]["path"], json!(dir), "{book}");
        if waiting {
            book["workers"][0]["action"] = json!("waiting for operator");
            book["workers"][0]["operator_reason"] = json!("no Jev key command is set");
        }
        book["observations_failed"] = json!([[dir, "old diagnosis failure", 1600]]);
        std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
        let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1350"]).ok();
        assert_eq!(joined["outcome"], "merged", "{joined}");
        if broken_gitdir {
            // A finished worker's local checkout need not remain usable.
            std::fs::write(dir.join(".git"), "gitdir: /missing/finished-worker\n").unwrap();
        }
        let ticked = tick(&world, next_tick);
        assert_eq!(ticked["errors"], json!([]), "{ticked}");
        let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(book["workers"], json!([]), "{book}");
        assert_eq!(book["observations_failed"], json!([]), "{book}");
        assert!(!world.root.join("typesafe-request").exists());
        assert_eq!(world.claims(), []);
        assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);
        assert_eq!(world.state(7), "closed");
    }
}

#[test]
fn a_landing_pushed_before_closing_the_issue_also_retires_expired_health() {
    let world = gated();
    let dir = worktree(&world);
    tick(&world, 1300);
    let path = world.cache().join("afkconvoy/worker-health.json");
    let mut book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    book["workers"][0]["action"] = json!("waiting for operator");
    std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
    let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1350"]).ok();
    assert_eq!(joined["outcome"], "merged", "{joined}");
    // Simulate interruption after the base push, before closing the issue.
    world.with_gh(|gh| gh["issues"][0]["state"] = json!("open"));
    assert_eq!(world.state(7), "open");

    let ticked = tick(&world, 5000);
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(did(&ticked).contains(&"settle 7 done".to_string()), "{ticked}");
    assert_eq!(world.state(7), "closed");
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["ready-for-agent", "bug"]);
    let book: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(book["workers"], json!([]), "{book}");
    assert!(!world.root.join("typesafe-request").exists());
}

#[test]
fn a_landing_that_closed_the_issue_before_the_next_tick_also_retires_health() {
    let world = gated();
    let dir = worktree(&world);
    tick(&world, 1300);
    let joined = world.afk_in(&dir, &["land", "--issue", "7", "--now", "1350"]).ok();
    assert_eq!(joined["outcome"], "merged", "{joined}");
    assert_eq!(world.state(7), "closed");

    let ticked = tick(&world, 5000);
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    let path = world.cache().join("afkconvoy/worker-health.json");
    let book: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(book["workers"], json!([]), "{book}");
    assert!(!world.root.join("typesafe-request").exists());
    assert_eq!(world.claims(), []);
}

#[test]
fn a_stopped_screen_and_a_complete_judgment_do_not_exempt_unfinished_work() {
    for outcome in ["open", "advanced", "none"] {
        let world = gated();
        let dir = worktree(&world);
        if outcome == "advanced" {
            commit(&dir, "more.txt", "more work\n");
            git(&dir, &["push", "-q", "origin", "HEAD"]);
        } else if outcome == "none" {
            world.with_gh(|gh| gh["prs"] = json!([]));
        }
        world.hold_every_harness();
        world.jev_key_command("printf key");
        let (screen, _) = world.jev_recorded("finished screen");
        world.with_orca(|orca| {
            for terminal in orca["terminals"].as_array_mut().unwrap() {
                if terminal["worktreePath"] == json!(dir) {
                    terminal["screen"] = json!(screen);
                }
            }
        });
        for now in [1300, 1600, 1900, 2200, 2500] {
            tick(&world, now);
        }
        let path = world.cache().join("afkconvoy/worker-health.json");
        let book: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let worker =
            book["workers"].as_array().unwrap().iter().find(|w| w["path"] == json!(dir)).unwrap();
        assert_eq!(worker["diagnosis"]["class"], "complete", "{book}");
        assert_eq!(worker["source"], "jev", "{book}");
        assert_eq!(worker["action"], "complete", "{book}");
        assert_eq!(world.claims(), [(7, "alpha".into())]);
    }
}

fn restart(world: &World, now: &str) -> common::Answer {
    let args = ["restart", "--issue", "7", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&[&args[..], &[]].concat())
}

fn manual_nudge(world: &World, now: &str) {
    world.afk(&["nudge", "7", "--instance", "alpha", "--host", "h1", "--now", now]).ok();
}

#[test]
fn a_silent_joining_worker_waits_for_the_operator_without_consuming_its_restart() {
    let world = joining();
    let dir = worktree(&world);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("scratch.txt"), "uncommitted").unwrap();
    let first = open_terminals(&world, &dir);
    for now in [1090, 1390, 1690] {
        assert_eq!(did(&tick(&world, now))[0], "watch 7 nothing");
    }
    assert_eq!(open_terminals(&world, &dir), first);
    assert_eq!(restarts(&world), [] as [String; 0]);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(dir.join("scratch.txt")).unwrap(), "uncommitted");
    assert_eq!(world.claims().len(), 1);
    assert!(pr_is_open(&world));
}

#[test]
fn the_loop_keeps_the_joining_claim_while_waiting_for_the_operator() {
    let world = joining();
    world.list_checkout(&world.clone);
    tick(&world, 1090);
    let args = [
        "run",
        "--foreground",
        "--host",
        "h1",
        "--second-ms",
        "0",
        "--instance",
        "alpha",
        "--ticks",
        "1",
        "--now",
        "1390",
    ];
    let lines = world.afk_lines(&args).0;
    assert!(lines[0].contains("nothing to do · holding 0 · 1 taking no slot"), "{lines:?}");
    assert_eq!(world.claims().len(), 1);
    assert!(restarts(&world).is_empty());
}

#[test]
fn a_second_restart_is_refused_by_hand_and_by_any_host_that_reads_the_pr() {
    let world = joining();
    // not before the nudge, nor within its grace
    assert!(restart(&world, "1090").error().contains("it was not nudged yet"));
    manual_nudge(&world, "1090");
    assert!(restart(&world, "1180").error().contains("sign of life within the grace period"));
    assert_eq!(restarts(&world), [] as [String; 0]);

    let restarted = restart(&world, "1390").ok();
    assert_eq!((&restarted["action"], &restarted["pr"]), (&json!("restarted"), &json!(70)));
    assert_eq!(restarted["prompt"], "land");
    assert_eq!(restarts(&world).len(), 1);

    // the new worker goes silent too: nudged, and past that there is no second restart
    manual_nudge(&world, "1690");
    let refused = restart(&world, "1990").error();
    assert!(refused.contains("already had its one restart"), "{refused}");
    assert_eq!(restarts(&world).len(), 1);
    assert_eq!(open_terminals(&world, &worktree(&world)).len(), 1);

    // a restart another host recorded counts here just the same
    let elsewhere = joining();
    elsewhere.comment(70, "<!--convoy:join restarted=1100-->\nrestarted on another host");
    manual_nudge(&elsewhere, "1400");
    assert!(restart(&elsewhere, "1700").error().contains("already had its one restart"));
    assert_eq!(restarts(&elsewhere).len(), 1);
    assert!(pr_is_open(&elsewhere));
}

#[test]
fn a_worker_whose_terminal_is_gone_is_continued_onto_the_join_and_keeps_its_restart() {
    let world = joining();
    let dir = worktree(&world);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
    world.orca_calls();

    assert_eq!(did(&tick(&world, 1090)), ["watch 7 continued", "heartbeat done"]);
    assert_eq!(restarts(&world), [] as [String; 0], "a continuation is not the restart");
    assert!(joins_only(&brief(&dir)));
    assert_eq!(open_terminals(&world, &dir).len(), 1);
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!calls.contains(&"worktree create".to_string()), "{calls:?}");
    assert_eq!(world.claims().len(), 1);

    // the join still has its whole ladder: a grace period from its start,
    // the nudge, then the one restart
    assert_eq!(did(&tick(&world, 1180))[0], "watch 7 nothing");
    manual_nudge(&world, "1390");
    restart(&world, "1690").ok();
    assert_eq!(restarts(&world).len(), 1);
}

#[test]
fn a_worker_is_continued_onto_its_own_pr_not_a_fork_s_numbered_higher() {
    let world = joining();
    world.fork_pr(99, 7, "patch-1");
    let dir = worktree(&world);
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });

    assert_eq!(did(&tick(&world, 1090)), ["watch 7 continued", "heartbeat done"]);
    let brief = brief(&dir);
    assert!(joins_only(&brief) && !brief.contains("#99"), "{brief}");
    assert_eq!(world.comments(99), [] as [String; 0]);
}

#[test]
fn with_no_worktree_on_this_host_the_join_is_continued_from_the_pushed_branch() {
    let world = joining();
    let head = world.remote(&["rev-parse", "refs/heads/tester/issue-7-issue-7"]);
    world.with_orca(|orca| {
        orca["worktrees"] = json!([]);
        orca["terminals"] = json!([]);
    });
    assert_eq!(did(&tick(&world, 1090))[0], "watch 7 continued");
    let dir = worktree(&world);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert!(joins_only(&brief(&dir)));
    assert_eq!(restarts(&world), [] as [String; 0]);
    assert!(pr_is_open(&world));
}

#[test]
fn a_worker_at_work_on_its_join_is_left_with_no_github_read() {
    let world = joining();
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "working", "stateStartedAt": 1_000_000});
            terminal["lastOutputAt"] = json!(1_170_000);
        }
    });
    // the tick that first sees the PR brings the status board up to date
    assert_eq!(did(&tick(&world, 1090)), ["watch 7 nothing", "heartbeat done"]);
    world.gh_calls();
    assert_eq!(did(&tick(&world, 1180)), ["watch 7 nothing", "heartbeat done"]);
    let calls: Vec<String> = world.gh_calls().iter().map(|call| call.join(" ")).collect();
    assert!(!calls.iter().any(|call| call.contains("/comments")), "{calls:?}");
    assert_eq!(calls.iter().filter(|call| call.starts_with("pr list")).count(), 1, "{calls:?}");
}

#[test]
fn a_worker_stopped_on_a_join_conflict_is_on_the_same_rungs_with_its_merge_left_as_it_is() {
    let world = joining();
    let dir = worktree(&world);
    commit(&dir, "README.md", "seven's\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.land_elsewhere("README.md");
    git(&dir, &["fetch", "-q", "origin"]);
    let merge = world.command("git", &dir).args(["merge", "-q", "origin/main"]).output().unwrap();
    assert!(!merge.status.success(), "the join conflicts");
    let unmerged = || git(&dir, &["diff", "--name-only", "--diff-filter=U"]);
    assert_eq!(unmerged(), "README.md");

    manual_nudge(&world, "1090");
    restart(&world, "1390").ok();
    assert_eq!(unmerged(), "README.md", "the merge in progress is the new worker's to finish");
    assert!(brief(&dir).contains("A merge left in progress is a"));
    assert_eq!(did(&tick(&world, 1690))[0], "watch 7 nothing");
    assert_eq!(did(&tick(&world, 1990))[0], "watch 7 nothing");
    assert_eq!(unmerged(), "README.md");
    assert!(pr_is_open(&world));
}

/// sunfmin/control#511: a landing whose turn answered `locked` waits, its
/// worker idle, while the lock stands; once the lock is gone the next tick asks
/// for its turn again — which merges onto the base tip as it is then — and its
/// line names the lock that closed. The worker's Claude Code session ends in a
/// compaction made while idle, which opens no turn of its own.
#[test]
fn a_landing_stopped_at_a_fleet_lock_is_asked_again_by_the_tick_once_the_lock_closes() {
    let world = joining();
    let dir = worktree(&world);
    let notes = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    world.issue(8, &["fleet-lock"]);
    std::fs::write(
        notes.join("convoy-turn.json"),
        json!({"outcome": "locked", "answer": {"outcome": "locked", "locks": [8]}, "at": 1050})
            .to_string(),
    )
    .unwrap();
    // Orca reports no agent: the session alone says the worker is idle.
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = Value::Null;
        }
    });
    let sessions = world.home().join(".claude/projects");
    let session = std::fs::read_dir(&sessions)
        .unwrap()
        .map(|dir| dir.unwrap().path().join("afkconvoy-turn.jsonl"))
        .find(|file| {
            std::fs::read_to_string(file).is_ok_and(|text| text.contains(dir.to_str().unwrap()))
        })
        .expect("the worker's Claude Code session");
    let iso = |at: u64| format!("1970-01-01T00:{:02}:{:02}Z", at / 60, at % 60);
    let entries = [
        json!({"type": "user", "timestamp": iso(1000), "cwd": dir, "isSidechain": false,
            "message": {"role": "user", "content": "brief"}}),
        json!({"type": "assistant", "timestamp": iso(1050), "cwd": dir, "isSidechain": false,
            "message": {"role": "assistant", "stop_reason": "end_turn"}}),
        json!({"type": "system", "subtype": "compact_boundary", "timestamp": iso(1400),
            "cwd": dir, "isSidechain": false}),
        json!({"type": "user", "timestamp": iso(1400), "cwd": dir, "isSidechain": false,
            "isCompactSummary": true, "isVisibleInTranscriptOnly": true,
            "message": {"role": "user", "content": "This session is being continued"}}),
    ];
    let text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    std::fs::write(&session, text).unwrap();
    let ticked = tick(&world, 1500);
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(!did(&ticked).iter().any(|step| step.starts_with("watch 7")), "{ticked}");
    assert!(!notes.join("convoy-turn.log").exists(), "asked again while the lock stood");

    world.close_issue(8);
    let ticked = tick(&world, 1501);
    assert_eq!(ticked["errors"], json!([]), "{ticked}");
    assert!(did(&ticked).contains(&"watch 7 asked".to_string()), "{ticked}");
    let watched = ticked["did"].as_array().unwrap().iter().find(|step| step["do"] == "watch");
    assert_eq!(watched.unwrap()["after_locks"], json!([8]), "{ticked}");
    assert!(notes.join("convoy-turn.log").exists(), "the turn was not asked again");
}

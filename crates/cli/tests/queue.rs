//! The operator command, through the real binary, with a local runner only.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::common::World;

struct Local {
    root: TempDir,
    short: TempDir,
}

impl Local {
    fn new() -> Self {
        let local = Self {
            root: tempfile::tempdir().unwrap(),
            short: tempfile::tempdir_in("/tmp").unwrap(),
        };
        std::fs::create_dir_all(local.cache()).unwrap();
        std::fs::create_dir(local.root.path().join("bin")).unwrap();
        std::os::unix::fs::symlink(local.cache(), local.short.path().join("cache")).unwrap();
        // Every external read or probe would leave evidence, even if its
        // failure were swallowed. No external program is needed for queue.
        for program in [
            "git", "gh", "orca", "sh", "bash", "zsh", "ps", "sysctl", "df", "claude", "codex",
            "pi", "dsh", "kimi", "grok", "kill",
        ] {
            let file = local.root.path().join("bin").join(program);
            std::fs::write(&file, "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> \"$CALLED\"\nexit 99\n")
                .unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        local
    }

    fn cache(&self) -> PathBuf {
        self.root.path().join("cache/afkconvoy")
    }

    fn socket(&self) -> PathBuf {
        self.short.path().join("cache/host-runner.sock")
    }

    fn settings(&self, lanes: u64) {
        std::fs::write(
            self.cache().join("checkouts.json"),
            json!({"settings": {"gate_lanes": lanes}, "checkouts": []}).to_string(),
        )
        .unwrap();
    }

    fn serve(&self, reply: impl FnOnce(UnixStream) + Send + 'static) -> Receiver<Value> {
        let listener = UnixListener::bind(self.socket()).unwrap();
        let (send, received) = channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            send.send(serde_json::from_str(&request).unwrap()).unwrap();
            reply(stream);
        });
        received
    }

    fn answer(&self, response: Value) -> Receiver<Value> {
        self.serve(move |mut stream| {
            let _ = writeln!(stream, "{response}");
        })
    }

    fn query(&self) -> (i32, Value) {
        let before = files(self.root.path());
        let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy"))
            .env_clear()
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("HOME", self.root.path())
            .env("PATH", self.root.path().join("bin"))
            .env("CALLED", self.root.path().join("called"))
            .current_dir(self.root.path())
            .args(["--now", "1000", "queue"])
            .output()
            .unwrap();
        let answer: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
            panic!("not one JSON object: {error}: {}", String::from_utf8_lossy(&out.stdout))
        });
        assert!(answer.is_object());
        assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            files(self.root.path()),
            before,
            "queue wrote a file or invoked an external command"
        );
        (out.status.code().unwrap(), answer)
    }
}

fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            result.extend(files(&entry.path()));
        } else {
            result.insert(
                entry.path(),
                if entry.file_type().unwrap().is_file() {
                    std::fs::read(entry.path()).unwrap()
                } else {
                    vec![]
                },
            );
        }
    }
    result
}

fn response(tickets: Vec<Value>) -> Value {
    json!({"exclusive": tickets, "exclusive_paused": false, "host_runner": {"pid": 4242, "version": "test", "log": "/cache/host-runner.log", "panel": null}})
}

fn ticket(number: u64, repo: &str, asked: u64, ran: Option<u64>, repo_lane: bool) -> Value {
    json!({"number": number, "pid": 9000 + number, "command": format!("afkconvoy turn {number}"), "dir": format!("/worktrees/{number}/same-name"), "issue": null, "repo": repo, "repo_lane": if repo_lane { Some(repo) } else { None }, "is_turn": true, "asked": asked, "ran": ran})
}

fn request(answer: &Value, number: u64) -> &Value {
    answer["running"]
        .as_array()
        .unwrap()
        .iter()
        .chain(answer["waiting"].as_array().unwrap())
        .find(|request| request["number"] == number)
        .unwrap()
}

#[test]
fn one_local_observation_explains_free_capacity_reservations_repo_lanes_and_elapsed_time() {
    let local = Local::new();
    local.settings(3);
    let mut a = ticket(1, "/repos/a/.git", 700, Some(800), false);
    a["issue"] = json!(128);
    let received = local.answer(response(vec![
        ticket(10, "/repos/r/.git", 855, None, true),
        ticket(9, "/repos/l/.git", 850, None, true),
        ticket(8, "/repos/r/.git", 845, None, false),
        ticket(7, "/repos/r/.git", 600, Some(650), true),
        ticket(6, "/repos/b/.git", 840, None, false),
        ticket(5, "/repos/d/.git", 830, None, false),
        ticket(4, "/repos/c/.git", 820, None, false),
        ticket(3, "/repos/b/.git", 820, None, false),
        ticket(2, "/repos/a/.git", 810, None, false),
        a,
    ]));
    let (code, answer) = local.query();
    assert_eq!(code, 0, "{answer}");
    assert_eq!(received.recv().unwrap(), json!({"op": "shared"}), "the only request is read-only");
    assert_eq!(answer["status"], "available");
    assert_eq!(answer["observed_at"], 1000);
    assert_eq!(answer["host_runner"]["pid"], 4242);
    assert_eq!(answer["worker"], json!({"capacity": 3, "used": 1, "free": 2}));
    assert_eq!(
        answer["running"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["number"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [7, 1]
    );
    assert_eq!(
        answer["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["number"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2, 3, 4, 5, 6, 8, 9, 10]
    );
    assert_eq!(request(&answer, 1)["elapsed_seconds"], 200);
    assert_eq!(request(&answer, 1)["waiting_seconds"], 100);
    assert_eq!(request(&answer, 2)["elapsed_seconds"], Value::Null);
    assert_eq!(request(&answer, 2)["waiting_seconds"], 190);
    assert_eq!(request(&answer, 1)["issue"], 128);
    assert_eq!(request(&answer, 3)["issue"], Value::Null);
    assert_eq!(
        request(&answer, 2)["repo"],
        "/repos/a/.git",
        "identity is shared across different worktrees"
    );
    assert_eq!(request(&answer, 2)["scheduling"]["state"], "waiting");
    let reason = &request(&answer, 2)["scheduling"]["reasons"][0];
    assert_eq!(reason["kind"], "same_repo");
    assert_eq!(reason["requests"], json!([{"number": 1, "state": "running"}]));
    for eligible in [3, 4, 9] {
        assert_eq!(request(&answer, eligible)["scheduling"]["state"], "eligible");
        assert_eq!(request(&answer, eligible)["scheduling"]["reasons"], json!([]));
    }
    let capacity = &request(&answer, 5)["scheduling"]["reasons"][0];
    assert_eq!(capacity["kind"], "worker_capacity");
    assert_eq!(capacity["capacity"], 3);
    assert_eq!(
        capacity["requests"],
        json!([{"number": 1, "state": "running"}, {"number": 3, "state": "eligible"}, {"number": 4, "state": "eligible"}])
    );
    assert_eq!(
        request(&answer, 6)["scheduling"]["reasons"][0]["requests"],
        json!([{"number": 3, "state": "eligible"}])
    );
    assert_eq!(
        request(&answer, 8)["scheduling"]["reasons"][0]["requests"],
        json!([{"number": 7, "state": "running"}])
    );
    assert!(
        request(&answer, 10)["scheduling"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "repo_capacity")
    );
    assert_eq!(
        answer["repos"],
        json!([
            {"repo": "/repos/l/.git", "capacity": 1, "used": 0, "free": 1},
            {"repo": "/repos/r/.git", "capacity": 1, "used": 1, "free": 0},
        ])
    );
    for waiting in answer["waiting"].as_array().unwrap() {
        for reason in waiting["scheduling"]["reasons"].as_array().unwrap() {
            assert!(!reason["explanation"].as_str().unwrap().is_empty());
        }
    }
}

#[test]
fn cuts_reserve_capacity_ahead_of_older_waiters_without_interrupting_running_requests() {
    let local = Local::new();
    local.settings(1);
    let mut cut = ticket(3, "/repos/cut/.git", 900, None, false);
    cut["cut"] = json!(true);
    let received = local.answer(response(vec![
        ticket(2, "/repos/worker/.git", 200, None, false),
        cut,
        ticket(1, "/repos/light/.git", 100, Some(110), true),
    ]));
    let (code, answer) = local.query();
    received.recv().unwrap();
    assert_eq!(code, 0, "{answer}");
    assert_eq!(answer["running"][0]["number"], 1);
    assert_eq!(answer["waiting"][0]["number"], 3);
    assert_eq!(answer["waiting"][0]["cut"], true);
    assert_eq!(answer["waiting"][0]["scheduling"]["state"], "eligible");
    assert_eq!(answer["waiting"][1]["number"], 2);
    assert_eq!(answer["waiting"][1]["cut"], false);
    assert_eq!(
        answer["waiting"][1]["scheduling"]["reasons"][0]["requests"],
        json!([{"number": 3, "state": "eligible"}])
    );
}

#[test]
fn empty_queue_missing_runner_and_stale_socket_are_distinct_successful_observations() {
    let local = Local::new();
    // No settings and no socket: no files or locks may be created.
    let (code, absent) = local.query();
    assert_eq!(code, 0, "{absent}");
    assert_eq!(absent["status"], "no_runner");
    assert_eq!(absent["host_runner"], Value::Null);
    assert_eq!(
        absent["worker"],
        json!({"capacity": afkconvoy::host_settings::GATE_LANES, "used": 0, "free": afkconvoy::host_settings::GATE_LANES})
    );
    let received = local.answer(response(vec![]));
    let (code, empty) = local.query();
    assert_eq!(code, 0, "{empty}");
    received.recv().unwrap();
    assert_eq!(empty["status"], "available");
    assert_eq!(empty["running"], json!([]));
    assert_eq!(empty["waiting"], json!([]));
    assert_eq!(empty["repos"], json!([]));
    // Its listener is gone; the stale socket remains untouched.
    let (code, stale) = local.query();
    assert_eq!(code, 0, "{stale}");
    assert_eq!(stale["status"], "no_runner");
    assert!(local.cache().join("host-runner.sock").exists());
}

#[test]
fn paused_and_old_ticket_metadata_are_reported_without_guessing_and_clock_skew_clamps() {
    let local = Local::new();
    local.settings(3);
    let old = json!({"number": 1, "pid": 4, "command": "cargo test", "dir": "/outside", "issue": null, "asked": 1100, "ran": null});
    let mut paused = response(vec![old]);
    paused["exclusive_paused"] = json!(true);
    let received = local.answer(paused);
    let (code, answer) = local.query();
    assert_eq!(code, 0, "{answer}");
    received.recv().unwrap();
    let request = request(&answer, 1);
    assert_eq!(request["repo"], Value::Null);
    assert!(request.as_object().unwrap().contains_key("repo"));
    assert_eq!(request["issue"], Value::Null);
    assert_eq!(request["repo_lane"], Value::Null);
    assert_eq!(request["is_turn"], false);
    assert_eq!(request["waiting_seconds"], 0);
    assert_eq!(request["scheduling"]["state"], "waiting");
    assert_eq!(request["scheduling"]["reasons"][0]["kind"], "paused");
    assert_eq!(request["scheduling"]["reasons"][0]["requests"], json!([]));
}

#[test]
fn failed_or_unsupported_observations_never_fabricate_an_empty_queue() {
    for reply in [
        "not json\n".to_string(),
        "".to_string(),
        "{\"error\":\"unsupported\"}\n".to_string(),
        "{}\n".to_string(),
        "{\"exclusive\":null}\n".to_string(),
        "{\"exclusive\":[],\"host_runner\":{\"pid\":1,\"version\":\"older\"}}\n".to_string(),
    ] {
        let local = Local::new();
        let received = local.serve(move |mut stream| {
            let _ = stream.write_all(reply.as_bytes());
        });
        let (code, answer) = local.query();
        received.recv().unwrap();
        assert_ne!(code, 0, "{answer}");
        assert_eq!(answer.as_object().unwrap().len(), 1, "{answer}");
        assert!(answer["error"].as_str().unwrap().contains("retry `afkconvoy queue`"));
    }
    let local = Local::new();
    std::fs::write(local.cache().join("checkouts.json"), "unreadable settings").unwrap();
    let (code, answer) = local.query();
    assert_ne!(code, 0, "{answer}");
    assert!(answer["error"].as_str().unwrap().contains("checkouts.json"));
    let local = Local::new();
    std::fs::create_dir(local.cache().join("host-runner.sock")).unwrap();
    assert_ne!(local.query().0, 0, "an inaccessible socket is not an absent runner");
    let local = Local::new();
    let lock = std::fs::File::create(local.cache().join("host-runner.lock")).unwrap();
    lock.lock().unwrap();
    let (code, answer) = local.query();
    assert_ne!(code, 0, "a runner holding the host but not listening is unavailable");
    assert!(answer["error"].as_str().unwrap().contains("holds the host"));
}

#[test]
fn a_partial_response_is_bounded_even_when_the_runner_keeps_sending_bytes() {
    let local = Local::new();
    let received = local.serve(|mut stream| {
        while stream.write_all(b" ").is_ok() {
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let began = Instant::now();
    let (code, answer) = local.query();
    received.recv().unwrap();
    assert_ne!(code, 0, "{answer}");
    assert!(answer["error"].as_str().unwrap().contains("did not answer within"), "{answer}");
    assert!(began.elapsed() < afkconvoy::queue::WITHIN + Duration::from_secs(5));
}

#[test]
fn the_real_scheduler_admits_another_repo_while_a_same_repo_waits_with_two_free_lanes() {
    let world = World::new();
    world.say_host_settings(json!({"gate_lanes": 3}));
    let _runner = world.host_held();
    let short = tempfile::tempdir_in("/tmp").unwrap();
    std::os::unix::fs::symlink(world.cache().join("afkconvoy"), short.path().join("cache"))
        .unwrap();
    let joins = |ticket: Value| {
        let stream = UnixStream::connect(short.path().join("cache/host-runner.sock")).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        writeln!(&stream, "{}", json!({"op": "exclusive", "ticket": ticket})).unwrap();
        let mut stream = BufReader::new(stream);
        let mut first = String::new();
        stream.read_line(&mut first).unwrap();
        assert!(serde_json::from_str::<Value>(&first).unwrap()["ahead"].is_number());
        stream
    };
    let running = |stream: &mut BufReader<UnixStream>| {
        let mut line = String::new();
        stream.read_line(&mut line).unwrap();
        assert!(serde_json::from_str::<Value>(&line).unwrap()["runs"].is_number(), "{line}");
    };
    let mut a = joins(ticket(0, "/actual/a/.git", 100, None, false));
    running(&mut a);
    let _same = joins(ticket(0, "/actual/a/.git", 101, None, false));
    let before = world.afk_in(&world.root, &["queue"]).ok();
    assert_eq!(before["worker"], json!({"capacity": 3, "used": 1, "free": 2}));
    assert_eq!(before["waiting"][0]["scheduling"]["reasons"][0]["kind"], "same_repo");
    let mut b = joins(ticket(0, "/actual/b/.git", 102, None, false));
    running(&mut b);
    let after = world.afk_in(&world.root, &["--now", "200", "queue"]).ok();
    assert_eq!(after["worker"], json!({"capacity": 3, "used": 2, "free": 1}));
    assert_eq!(after["waiting"].as_array().unwrap().len(), 1);
    let doctor = world.afk(&["--now", "200", "doctor"]).json;
    let common = &doctor["host"]["exclusive"]["observation"];
    for field in ["worker", "repos", "paused", "running", "waiting"] {
        assert_eq!(common[field], after[field], "doctor and queue disagree on {field}: {doctor}");
    }
    assert_eq!(
        doctor["host"]["exclusive"]["lanes"]["worker"]["running_all"].as_array().unwrap().len(),
        2,
        "legacy doctor projection remains available"
    );
}

#[test]
fn queue_is_discoverable_in_operator_help_and_its_contract_is_complete() {
    let help = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args(args).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    assert!(help(&["--help"]).contains("queue"));
    let manual = help(&["help", "all"]);
    let operators =
        manual.split("### Run by the operator").nth(1).unwrap().split("\n### ").next().unwrap();
    assert!(operators.contains("`afkconvoy queue`"));
    let own = help(&["queue", "--help"]);
    for required in [
        "observed_at",
        "host_runner",
        "capacity",
        "used",
        "free",
        "same_repo",
        "worker_capacity",
        "repo_capacity",
        "eligible",
        "no_runner",
        "GitHub",
        "exit 1",
        "--since",
        "reservation",
        "no fixed future start order",
        "not shown as the script",
        "no `NAME=value`",
        "cargo nextest",
    ] {
        assert!(own.contains(required), "queue help omits {required}");
    }
    let doctor = help(&["doctor", "--help"]);
    for required in ["not shown as the script", "no `NAME=value`", "cargo nextest"] {
        assert!(doctor.contains(required), "doctor help omits {required}");
    }
}

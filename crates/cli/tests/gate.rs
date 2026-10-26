use crate::common;

use afkconvoy::gate::recorded;
use afkconvoy::git::Repo;
use afkconvoy_core::gate::{GateCommand, RECORD_TTL, Tree};
use common::{World, git};
use serde_json::json;

use std::os::unix::net::UnixListener;
use std::process::{Child, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use afkconvoy::runner_socket::{self, Kept, Reply, Request, SOCKET};

const GATE_BINARY: &str = env!("CARGO_BIN_EXE_afkconvoy");

/// A gate held by an isolated runner until the test explicitly grants it.
/// Drop cleans up even when an assertion fails before the grant.
struct QueuedGate {
    _socket: tempfile::TempDir,
    child: Option<Child>,
    group: i32,
    place: Kept,
}

impl QueuedGate {
    fn start(world: &World) -> Self {
        let dir = world.cache().join("afkconvoy");
        std::fs::create_dir_all(&dir).unwrap();
        let socket = tempfile::tempdir_in("/tmp").unwrap();
        let path = socket.path().join(SOCKET);
        let listener = UnixListener::bind(&path).unwrap();
        std::os::unix::fs::symlink(path, dir.join(SOCKET)).unwrap();
        let (send, receive) = mpsc::channel();
        runner_socket::serve(listener, json!({}), move |request| match request {
            Request::Exclusive { ticket } => {
                let send = send.clone();
                Reply::Kept(
                    json!({"ahead": 7}),
                    Box::new(move |kept| {
                        let _ = send.send((ticket.pid, kept));
                    }),
                )
            }
            _ => Reply::Line(json!({"woke": false})),
        });
        let child = world
            .command(GATE_BINARY, &world.clone)
            .env("EXECUTED", world.root.join("executed"))
            .env("FAIL_GATE", world.root.join("fail-gate"))
            .args(["gate"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (pid, place) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        // SAFETY: pid is this gate's exclusive child in the isolated runner.
        let group = unsafe { libc::getpgid(pid as i32) };
        assert!(group > 0);
        Self { _socket: socket, child: Some(child), group, place }
    }

    fn observe(&mut self, world: &World) {
        for interval in 0..3 {
            // Exercise the real socket read between the gate's child polls;
            // only an explicit runs message grants the command its turn.
            assert!(self.place.say(json!({"observation": interval})));
            std::thread::sleep(Duration::from_millis(100));
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "gate ended while queued"
            );
            assert!(!world.root.join("executed").exists());
            assert!(records(world).is_empty());
        }
    }

    fn interrupt(&self) {
        // SAFETY: this is the gate child's own process group, never ours.
        assert_eq!(unsafe { libc::kill(-self.group, libc::SIGKILL) }, 0);
    }

    fn finish(&mut self) -> Output {
        let out = self.child.take().unwrap().wait_with_output().unwrap();
        eprintln!(
            "stdout:\n{}stderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}

impl Drop for QueuedGate {
    fn drop(&mut self) {
        self.place.close();
        if let Some(child) = self.child.as_mut()
            && child.try_wait().is_ok_and(|status| status.is_none())
        {
            // SAFETY: group belongs to the child this test still owns.
            unsafe { libc::kill(-self.group, libc::SIGKILL) };
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn queue_command(world: &World) {
    let command = afkconvoy_core::line::Line::new()
        .fixed("exec ")
        .program(GATE_BINARY)
        .fixed(" exclusive -- sh -c ")
        .word("echo executed >> \"$EXECUTED\"; if test -f \"$FAIL_GATE\"; then exit 3; fi")
        .own();
    // Markers are outside the committed tree so the gate can record a pass.
    world.set_gate_config(&format!("gate_command: {command}\ngate_seconds: 10"));
}

#[test]
fn a_queued_gate_survives_observation_and_runs_exactly_once_when_granted() {
    let world = World::new();
    queue_command(&world);
    for code in [0, 3] {
        if code == 3 {
            std::fs::write(world.root.join("fail-gate"), "").unwrap();
            std::fs::remove_file(world.root.join("executed")).unwrap();
            std::fs::remove_file(world.cache().join("afkconvoy").join(SOCKET)).unwrap();
        }
        let mut gate = QueuedGate::start(&world);
        // The earlier green record remains until the second run ends.
        if code == 0 {
            gate.observe(&world);
        }
        assert!(gate.place.say(json!({"runs": 5000})));
        let out = gate.finish();
        let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(answer["exit_code"], code, "{answer}");
        assert_eq!(answer["timed_out"], false);
        assert_eq!(answer["recorded"], code == 0);
        assert_eq!(std::fs::read_to_string(world.root.join("executed")).unwrap(), "executed\n");
    }
    assert!(records(&world).is_empty(), "the failed run invalidated the earlier pass");
}

#[test]
fn a_signalled_queued_gate_keeps_termination_evidence_without_claiming_checks_ran() {
    let world = World::new();
    queue_command(&world);
    let mut gate = QueuedGate::start(&world);
    gate.observe(&world);
    gate.interrupt();
    let out = gate.finish();
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(answer["status"], "red");
    assert_eq!(answer["exit_code"], -1, "preserve the existing exit-code contract");
    assert_eq!(answer["timed_out"], false);
    assert_eq!(answer["termination"]["signal"], libc::SIGKILL, "{answer}");
    assert_eq!(answer["termination"]["pid"], gate.group, "{answer}");
    assert_eq!(answer["termination"]["core_dumped"], false);
    assert_eq!(answer["recorded"], false);
    assert!(!world.root.join("executed").exists());
    assert!(records(&world).is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("signal 9"), "{stderr}");
    assert!(answer["detail"].as_str().unwrap().contains("checks may not have started"), "{answer}");
    let runs = world.cache().join("afkconvoy/runs");
    let log = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
        .collect::<String>();
    assert!(log.contains("signal 9"), "{log}");
}

/// A world whose gate is red exactly while the file `red` exists beside it.
fn gated() -> (World, String) {
    let world = World::new();
    let command = format!("echo gate-ran; test ! -f {}", world.root.join("red").display());
    world.set_gate(&command);
    (world, command)
}

fn records(world: &World) -> Vec<String> {
    let listed = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/gate"]);
    listed.lines().map(str::to_string).collect()
}

#[test]
fn a_green_run_on_a_committed_tree_is_put_on_record() {
    let (world, command) = gated();
    let gate = world.afk(&["gate", "--now", "5000", "--host", "mini"]);
    assert!(gate.stderr.contains("gate-ran"), "the log streams to the terminal: {}", gate.stderr);
    let answer = gate.ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["recorded"], true);
    assert_eq!(answer["command"], command.as_str());
    assert_eq!(answer["head"], git(&world.clone, &["rev-parse", "HEAD"]));
    let seconds = answer["seconds"].as_u64().expect("the answer says how long the run took");
    assert!(seconds < 60, "{answer}");

    let tree = git(&world.clone, &["rev-parse", "HEAD^{tree}"]);
    let refs = records(&world);
    assert_eq!(refs.len(), 1);
    assert!(refs[0].starts_with(&format!("refs/convoy/gate/{tree}-")), "{refs:?}");
    // the record is a commit of the tree it is about
    assert_eq!(world.remote(&["rev-parse", &format!("{}^{{tree}}", refs[0])]), tree);
    let subject = world.remote(&["log", "-1", "--format=%s", &refs[0]]);
    assert!(subject.starts_with("convoy:gate tree="), "{subject}");
    // …and says when the run finished, how long it took and on which host
    assert!(subject.ends_with(&format!(" at=5000 seconds={seconds} host=mini")), "{subject}");
}

#[test]
fn a_run_is_timed_from_the_start_of_the_gate_command_to_its_end() {
    let world = World::new();
    world.set_gate("sleep 2");
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["recorded"], true, "{answer}");
    assert!((2..30).contains(&answer["seconds"].as_u64().unwrap()), "{answer}");
    let subject = world.remote(&["log", "-1", "--format=%s", &records(&world)[0]]);
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let said = format!(" seconds={} host={host}", answer["seconds"]);
    assert!(subject.ends_with(&said), "the host is what the fleet calls this one: {subject}");
}

#[test]
fn a_red_run_records_nothing_and_takes_the_trees_record_away() {
    let (world, _) = gated();
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    assert_eq!(records(&world).len(), 1);

    std::fs::write(world.root.join("red"), "").unwrap();
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["status"], "red");
    assert_eq!(answer["exit_code"], 1);
    assert_eq!(answer["recorded"], false);
    assert!(answer["seconds"].is_u64(), "the run is timed, though no record keeps it: {answer}");
    assert_eq!(records(&world), [] as [String; 0], "the latest run of a tree is the one believed");
}

#[test]
fn a_run_that_reaches_the_configs_gate_limit_is_ended_and_red() {
    let world = World::new();
    world.set_gate_config("gate_command: sleep 30\ngate_seconds: 1");
    let started = std::time::Instant::now();
    let answer = world.afk(&["gate"]).ok();
    assert!(started.elapsed().as_secs() < 10, "the run was killed");
    assert_eq!(answer["status"], "red");
    assert_eq!((&answer["timed_out"], &answer["exit_code"]), (&json!(true), &json!(124)));
    assert_eq!(answer["termination"], serde_json::Value::Null);
    assert_eq!(answer["recorded"], false);
    let detail = answer["detail"].as_str().unwrap();
    assert!(detail.contains("reached the gate limit of 1s"), "{detail}");
}

#[test]
fn a_red_run_that_ended_by_itself_says_nothing_of_the_gate_limit() {
    let (world, _) = gated();
    std::fs::write(world.root.join("red"), "").unwrap();
    let answer = world.afk(&["gate"]).ok();
    assert_eq!((&answer["status"], &answer["timed_out"]), (&json!("red"), &json!(false)));
    assert!(!answer["detail"].as_str().unwrap().contains("gate limit"), "{answer}");
}

#[test]
fn a_signal_is_distinct_from_an_ordinary_shell_exit_and_a_gate_timeout() {
    let world = World::new();
    world.set_gate("kill -TERM $$");
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["termination"]["signal"], libc::SIGTERM, "{answer}");
    assert_eq!(answer["exit_code"], -1);
    assert_eq!(answer["timed_out"], false);
    assert_eq!(answer["recorded"], false);

    world.set_gate("exit 137");
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["exit_code"], 137);
    assert_eq!(answer["termination"], serde_json::Value::Null);
    assert_eq!(answer["timed_out"], false);
    assert_eq!(answer["recorded"], false);
}

#[test]
fn a_run_over_uncommitted_or_untracked_files_is_not_recorded() {
    let (world, _) = gated();
    std::fs::write(world.clone.join("scratch.txt"), "x").unwrap();
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["recorded"], false);
    assert_eq!(answer["uncommitted"][0], "?? scratch.txt");
    assert!(answer["detail"].as_str().unwrap().contains("not on a committed tree"));
    assert_eq!(records(&world), [] as [String; 0]);

    // …and a red one over them leaves an earlier record alone: it tested no commit's tree
    std::fs::remove_file(world.clone.join("scratch.txt")).unwrap();
    world.afk(&["gate"]).ok();
    std::fs::write(world.clone.join("scratch.txt"), "x").unwrap();
    std::fs::write(world.root.join("red"), "").unwrap();
    assert_eq!(world.afk(&["gate"]).ok()["status"], "red");
    assert_eq!(records(&world).len(), 1);
}

#[test]
fn a_run_that_rewrites_a_tracked_file_is_not_recorded() {
    let world = World::new();
    world.set_gate("echo generated >> README.md");
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["recorded"], false);
    assert_eq!(answer["uncommitted"][0], " M README.md");
    assert_eq!(records(&world), [] as [String; 0]);
}

#[test]
fn a_run_that_commits_is_not_a_run_of_the_commit_it_started_on() {
    let world = World::new();
    world.set_gate("git commit -q --allow-empty -m sneaky");
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["recorded"], false);
    assert!(answer["uncommitted"][0].as_str().unwrap().starts_with("HEAD moved from"));
}

#[test]
fn a_record_that_cannot_be_written_is_not_an_error() {
    let (world, _) = gated();
    world.watch_pushes(Some("convoy/gate"));
    let answer = world.afk(&["gate"]).ok();
    assert_eq!(answer["status"], "green");
    assert_eq!(answer["recorded"], false);
    assert!(answer["not_recorded"].as_str().unwrap().contains("rejected by the test"));
}

#[test]
fn a_record_stands_for_the_same_tree_and_command_for_one_day() {
    let (world, command) = gated();
    world.afk(&["gate", "--now", "100000"]).ok();
    let repo = Repo::at(&world.clone).unwrap();
    let tree = Tree::named(&git(&world.clone, &["rev-parse", "HEAD^{tree}"]));
    let command = GateCommand::new(&command);
    let at = |now: u64| recorded(&repo, &tree, &command, now).unwrap().map(|run| run.at());

    assert_eq!(at(100_000), Some(100_000));
    assert_eq!(at(100_000 + RECORD_TTL), Some(100_000));
    assert_eq!(at(100_001 + RECORD_TTL), None, "a day old");
    assert_eq!(at(90_000), None, "stamped from the future: it never stood");

    let other_command = GateCommand::new("true");
    assert!(recorded(&repo, &tree, &other_command, 100_000).unwrap().is_none());
    let other_tree = Tree::named("4b825dc642cb6eb9a060e54bf8d69288fbee4904");
    assert!(recorded(&repo, &other_tree, &command, 100_000).unwrap().is_none());
}

#[test]
fn a_gate_run_in_a_worktree_reads_the_config_committed_there() {
    let (world, _) = gated();
    world.issue(7, &["ready-for-agent"]);
    let started = world.afk(&["dispatch", "--issue", "7", "--instance", "alpha"]).ok();
    let worktree = std::path::Path::new(started["worktree"].as_str().unwrap());
    let answer = world.afk_in(worktree, &["gate"]).ok();
    assert_eq!(answer["recorded"], true, "{answer}");
}

#[test]
fn a_workers_first_green_run_records_its_fresh_build_and_a_later_one_leaves_it() {
    let world = World::new();
    let red = world.root.join("red");
    world.set_gate(&format!(
        "test ! -f {} && mkdir -p out && printf 0123456789 >> out/build",
        red.display()
    ));
    common::commit(&world.clone, ".gitignore", "out/\n");
    let repo = Repo::new(world.clone.clone());

    // a red run may have built nothing: no measure of what it should hold
    std::fs::write(&red, "").unwrap();
    assert_eq!(world.afk(&["gate"]).ok()["status"], "red");
    assert_eq!(afkconvoy::measure::fresh(&repo).unwrap(), None);

    std::fs::remove_file(&red).unwrap();
    assert_eq!(world.afk(&["gate"]).ok()["status"], "green");
    assert_eq!(afkconvoy::measure::fresh(&repo).unwrap(), Some(10));
    assert_eq!(world.afk(&["gate"]).ok()["status"], "green");
    assert_eq!(afkconvoy::measure::fresh(&repo).unwrap(), Some(10), "recorded once");
}

//! The gate crate as another program uses it (docs/adr/0059): from outside,
//! through its public API alone, with no fleet beside it. A host process
//! embeds a queue holder, runs a repo's fast gate in the fast lane on a
//! scratch repo, and reads back the recorded gate run; heavy commands wait
//! their turn in the order they asked.

use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use afkconvoy_gate::declared;
use afkconvoy_gate::exclusive::{Gate, Lanes, Plain, Ticket};
use afkconvoy_gate::gate::{GateCommand, Judged, TestTime};
use afkconvoy_gate::holder::{Holder, Host, Turn};
use afkconvoy_gate::layout::Layout;
use afkconvoy_gate::run::{self, Git, Settled};

/// A host with one place in each lane, and a bound no test reaches.
struct OneEach;

impl Host<Plain> for OneEach {
    fn now(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    fn lanes(&self) -> Lanes {
        Lanes { worker: 1, fast: 1, slow: 1 }
    }

    fn bound(&self) -> u64 {
        3600
    }
}

fn ticket(command: &str, dir: &str, asked: u64, gate: Option<Gate>) -> Ticket {
    Ticket {
        number: 0,
        pid: std::process::id(),
        command: command.into(),
        dir: dir.into(),
        cut: false,
        repo_lane: None,
        repo: Some(format!("{dir}/.git")),
        is_turn: false,
        candidate: false,
        gate,
        asked,
        ran: None,
        note: Plain::default(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A scratch repo whose declaration splits its gate in two, committed and
/// pushed to a bare remote beside it.
const DECLARED: &str = "# The repo's gate\n\n```yaml gates\nfast:\n  command: ./gate.sh fast\n  seconds: 60\n  budget_seconds: 30\nslow:\n  command: ./gate.sh slow\n  seconds: 120\n  budget_seconds: 60\n```\n";

/// A gate whose fast part times one test as nextest does, and says its CPU
/// time as its runner measured it; whose slow part times none.
const GATE_SH: &str = "#!/bin/sh\necho \"checking $1\"\nif [ \"$1\" = fast ]; then\n  echo '        PASS [   0.012s] (1/1) nx tests::quick'\n  echo 'gate: cpu [   0.010s] nx tests::quick'\nfi\n";

fn scratch(root: &Path) -> Git {
    let remote = root.join("remote.git");
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(root, &["init", "--quiet", "--bare", remote.to_str().unwrap()]);
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    for (key, value) in [("user.name", "someone"), ("user.email", "someone@example.com")] {
        git(&work, &["config", key, value]);
    }
    std::fs::write(work.join("gate.md"), DECLARED).unwrap();
    std::fs::write(work.join("gate.sh"), GATE_SH).unwrap();
    let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(work.join("gate.sh"), executable).unwrap();
    git(&work, &["add", "gate.md", "gate.sh"]);
    git(&work, &["commit", "--quiet", "-m", "a repo with a gate"]);
    git(&work, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&work, &["push", "--quiet", "origin", "main"]);
    Git { worktree: work, remote: "origin".into(), layout: Layout::Hidden }
}

#[test]
fn a_repos_fast_gate_runs_in_the_fast_lane_and_its_run_reads_back_from_the_record() {
    let root = tempfile::tempdir().unwrap();
    let repo = scratch(root.path());
    let declared = std::fs::read_to_string(repo.worktree.join("gate.md")).unwrap();
    let gates = declared::gates(&declared, 180, None).unwrap().expect("a fast and a slow gate");
    let fast = GateCommand::new(&gates.fast.command);
    let holder = Holder::new(OneEach);
    let dir = repo.worktree.to_str().unwrap().to_string();
    let now = OneEach.now();
    let limit = Duration::from_secs(gates.fast.seconds);
    let ran = holder
        .alone(ticket(fast.text(), &dir, now, Some(Gate::Fast)), |_| {
            run::gate(&repo, &fast, "scratch-host", now, limit, &[("by", "a-daemon")])
        })
        .expect("its turn came")
        .expect("the gate ran");
    assert_eq!(ran.judged, Judged::Green, "{:?}", ran.output);
    assert!(ran.output.iter().any(|line| line == "checking fast"), "{:?}", ran.output);
    assert!(matches!(ran.settled, Settled::Written { not_recorded: None, .. }), "{ran:?}");
    assert!(holder.observation().unwrap().0.is_empty(), "it left the queue");

    // Another worktree of the same repo reads the run back off the remote.
    let other = root.path().join("other");
    git(
        root.path(),
        &["clone", "--quiet", root.path().join("remote.git").to_str().unwrap(), "other"],
    );
    let reader = Git { worktree: other.clone(), ..repo.clone() };
    let (_, tree) = run::head(&other).unwrap();
    assert_eq!(tree, ran.tree, "the same committed tree");
    let read = run::recorded(&reader, &tree, &fast, now).unwrap().expect("on record");
    let took = read.took().expect("it says how long it took, and where");
    assert_eq!(took.host(), "scratch-host");
    assert_eq!(took.seconds(), ran.ran.seconds, "the run's own duration");
    assert_eq!(read.at(), now);
    // Read from its record's whole message, it times each test its output timed.
    let timed = run::recorded_with_tests(&reader, &tree, &fast, now).unwrap().expect("on record");
    let quick = TestTime { name: "nx tests::quick".into(), wall_ms: Some(12), cpu_ms: Some(10) };
    assert_eq!(timed.tests(), [quick]);
    assert_eq!((timed.at(), timed.took()), (read.at(), read.took()));
    assert!(read.tests().is_empty(), "a subject alone times no test");
    // The slow gate never ran on it: nothing of it is on record.
    let slow = GateCommand::new(&gates.slow.command);
    assert_eq!(run::recorded(&reader, &tree, &slow, now).unwrap(), None);
    assert_eq!(run::recorded_with_tests(&reader, &tree, &slow, now).unwrap(), None);
}

#[test]
fn a_run_whose_output_times_no_test_reads_back_as_the_same_run_with_none() {
    let root = tempfile::tempdir().unwrap();
    let repo = scratch(root.path());
    let slow = GateCommand::new("./gate.sh slow");
    let now = OneEach.now();
    let ran = run::gate(&repo, &slow, "scratch-host", now, Duration::from_secs(60), &[]).unwrap();
    assert_eq!(ran.judged, Judged::Green, "{:?}", ran.output);
    let read = run::recorded_with_tests(&repo, &ran.tree, &slow, now).unwrap().expect("on record");
    assert!(read.tests().is_empty());
    assert_eq!(Some(read), run::recorded(&repo, &ran.tree, &slow, now).unwrap());
}

#[test]
fn a_heavy_command_waits_its_turn_behind_those_that_asked_before_it() {
    let holder = Holder::new(OneEach);
    let (first, ahead) = holder.join(ticket("cargo build", "/w/a", 100, None));
    assert_eq!(ahead, 0);
    assert!(matches!(holder.turn(first), Turn::Runs(_)), "nothing is ahead of it");
    let (second, ahead) = holder.join(ticket("cargo test", "/w/b", 101, None));
    assert_eq!(ahead, 1);
    let (third, ahead) = holder.join(ticket("cargo doc", "/w/c", 102, None));
    assert_eq!(ahead, 2);
    // A fast gate takes the fast lane: no heavy command of the worker lane
    // holds it up.
    let (fast, ahead) = holder.join(ticket("./gate.sh fast", "/w/d", 103, Some(Gate::Fast)));
    assert_eq!(ahead, 0);
    assert!(matches!(holder.turn(fast), Turn::Runs(_)));
    holder.leave(fast);

    let (ran, order) = mpsc::channel();
    for number in [third, second] {
        let (holder, ran) = (holder.clone(), ran.clone());
        std::thread::spawn(move || {
            if matches!(holder.turn(number), Turn::Runs(_)) {
                ran.send(number).unwrap();
                std::thread::sleep(Duration::from_millis(200));
                holder.leave(number);
            }
        });
    }
    assert!(order.recv_timeout(Duration::from_millis(300)).is_err(), "both wait while one runs");
    holder.leave(first);
    let wait = Duration::from_secs(5);
    assert_eq!(order.recv_timeout(wait).unwrap(), second, "the one that asked first runs first");
    assert_eq!(order.recv_timeout(wait).unwrap(), third);
}

/// What the crate depends on: Flux's attributes, serde and YAML — never
/// afkconvoy-core, whose fleet concepts its API is to stay free of.
#[test]
fn the_crate_depends_on_nothing_of_the_fleet() {
    let manifest = include_str!("../Cargo.toml");
    let section = manifest.split("[dependencies]").nth(1).unwrap().split("\n[").next().unwrap();
    let mut names: Vec<&str> = section
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once(['=', '.']).map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names, ["flux-alloc", "flux-core", "flux-rs", "serde", "serde_json", "serde_yaml"]);
}

/// The crate's public API names no fleet concept: none of its public items
/// and fields is named for a team, role, stage, issue, claim, patrol,
/// intake, tracker or GitHub.
#[test]
fn its_public_api_names_no_fleet_concept() {
    const FLEET: [&str; 9] =
        ["team", "role", "stage", "issue", "claim", "patrol", "intake", "tracker", "github"];
    let sources = [
        include_str!("../src/lib.rs"),
        include_str!("../src/declared.rs"),
        include_str!("../src/exclusive.rs"),
        include_str!("../src/gate.rs"),
        include_str!("../src/holder.rs"),
        include_str!("../src/layout.rs"),
        include_str!("../src/record.rs"),
        include_str!("../src/run.rs"),
        include_str!("../src/shell.rs"),
        include_str!("../src/slow_gate.rs"),
    ];
    for source in sources {
        let public = source.split("#[cfg(test)]").next().unwrap();
        for line in public.lines().map(str::trim).filter(|line| line.starts_with("pub ")) {
            let lower = line.to_lowercase();
            let words: Vec<&str> = lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            for concept in FLEET {
                assert!(
                    !words.iter().any(|word| word.starts_with(concept)),
                    "`{line}` names a fleet concept: {concept}"
                );
            }
        }
    }
}

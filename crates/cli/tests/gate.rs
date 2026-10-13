mod common;

use afkconvoy::gate::recorded;
use afkconvoy::git::Repo;
use afkconvoy_core::gate::{GateCommand, RECORD_TTL, Tree};
use common::{World, git};

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
fn a_run_that_times_out_is_red() {
    let world = World::new();
    world.set_gate("sleep 30");
    let started = std::time::Instant::now();
    let answer = world.afk(&["gate", "--gate-timeout", "1"]).ok();
    assert!(started.elapsed().as_secs() < 10, "the run was killed");
    assert_eq!(answer["status"], "red");
    assert_eq!(answer["timed_out"], true);
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
    let started = world
        .afk(&["dispatch", "--issue", "7", "--instance", "alpha", "--worker-command", "agent"])
        .ok();
    let worktree = std::path::Path::new(started["worktree"].as_str().unwrap());
    let answer = world.afk_in(worktree, &["gate"]).ok();
    assert_eq!(answer["recorded"], true, "{answer}");
}

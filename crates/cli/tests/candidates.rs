//! Real binary, real remotes and kernel processes. Gates wait on controlled
//! release files; assertions inspect refs, original heads and child exit.
use crate::common::{World, commit, git, worktree_listed};
use serde_json::{Value, json};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn quote(path: &Path) -> String {
    afkconvoy_core::line::Line::new().word(&path.to_string_lossy()).own()
}

fn until(mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(30), "candidate transition never happened");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A world whose gates, and what they started, end with a failing test: a
/// gate outliving it would keep whatever lane lease the test ran under.
struct Fleet(World);

impl Deref for Fleet {
    type Target = World;
    fn deref(&self) -> &World {
        &self.0
    }
}

impl Drop for Fleet {
    fn drop(&mut self) {
        // A passing test has already seen its gates end; their pids may be reused.
        if !std::thread::panicking() {
            return;
        }
        // A pid kept in a file may since be another process's, under another
        // group — the host runner's own, whose children are in it, among them
        // (sunfmin/control#820). So a gate's group is ended only while the
        // gate is still in it, and a pid only while it is in that group.
        let group_of = |pid: &str| {
            let group = Command::new("ps").args(["-p", pid, "-o", "pgid="]).output().unwrap();
            String::from_utf8_lossy(&group.stdout).trim().to_string()
        };
        let read = |kind: &str, name: &str| {
            let kept = std::fs::read_to_string(self.root.join(format!("{kind}-{name}")));
            kept.ok().map(|pid| pid.trim().to_string()).filter(|pid| !pid.is_empty())
        };
        for name in ["a", "b", "c", "d", "e"] {
            let (Some(gate), Some(group)) = (read("gate", name), read("group", name)) else {
                continue;
            };
            // SAFETY: getpgrp only reads this process's group.
            let ours = unsafe { libc::getpgrp() }.to_string();
            if group != ours && group_of(&gate) == group {
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{group}")]).status();
            }
            for pid in std::iter::once(gate).chain(read("child", name)) {
                if group_of(&pid) == group {
                    let _ = Command::new("kill").args(["-KILL", &pid]).status();
                }
            }
        }
    }
}

fn world(lanes: u64) -> Fleet {
    let world = World::new();
    let root = quote(&world.root);
    let script = format!(
        r#"#!/bin/sh
set -e
root={root}
name=
for n in a b c d e; do if test -f "$n.txt"; then name=$n; fi; done
case "${{AFKCONVOY_ISSUE:-}}" in 1) name=a;; 2) name=b;; 3) name=c;; 4) name=d;; 5) name=e;; esac
test -n "$name" || exit 0
echo $$ > "$root/gate-$name"
/bin/ps -o pgid= -p $$ | tr -d ' ' > "$root/group-$name"
sleep 120 &
echo $! > "$root/child-$name"
touch "$root/start-$name"
while test -d "$root" && test ! -f "$root/release-$name"; do sleep 0.02; done
{said}
test ! -f "$root/fail-$name"
"#,
        said = said("$name")
    );
    commit(&world.clone, "gate.sh", &script);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    let command = afkconvoy_core::line::Line::new()
        .program(env!("CARGO_BIN_EXE_afkconvoy"))
        .fixed(" exclusive -- sh ./gate.sh")
        .own();
    world.set_gate(&command);
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    world.say_host_settings(json!({"gate_lanes": lanes}));
    Fleet(world)
}

fn start(world: &World, dir: &Path, cache: &Path, n: u64) -> Child {
    start_with_record_push_limit(world, dir, cache, n, None)
}

fn start_with_record_push_limit(
    world: &World,
    dir: &Path,
    cache: &Path,
    n: u64,
    seconds: Option<u64>,
) -> Child {
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), dir);
    command
        .env("XDG_CACHE_HOME", cache)
        .env("AFKCONVOY_ISSUE", n.to_string())
        .args(["land", "--issue", &n.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(seconds) = seconds {
        command.env("AFKCONVOY_GATE_RECORD_PUSH_SECONDS", seconds.to_string());
    }
    command.spawn().unwrap()
}

fn finish(child: Child) -> Value {
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}; stdout: {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The gate script's lines that end it red on one TIMEOUT where the test
/// put `overloaded-<at>` (the gate says the host is overloaded) or
/// `timeout-<at>` (it says it is quiet) in its root.
fn said(at: &str) -> String {
    let timeout = "echo '     TIMEOUT [   6.004s] (40/669) afkconvoy::cli dsh::the_line'";
    format!(
        "if test -f \"$root/overloaded-{at}\"; then {timeout}; echo 'gate: host overloaded or \
         load unavailable (load 23.57, CPUs 12); cannot classify TIMEOUT tests. Retry on a quiet \
         host.' >&2; exit 100; fi\nif test -f \"$root/timeout-{at}\"; then {timeout}; echo \
         'gate: afkconvoy::cli dsh::the_line was still TIMEOUT at 2s on a quiet host.' >&2; exit \
         100; fi"
    )
}

fn release(world: &World, name: &str, red: bool) {
    if red {
        std::fs::write(world.root.join(format!("fail-{name}")), "").unwrap();
    }
    std::fs::write(world.root.join(format!("release-{name}")), "").unwrap();
}

fn began(world: &World, name: &str) {
    eprintln!("waiting for gate {name} in {}", world.root.display());
    until(|| world.root.join(format!("start-{name}")).exists());
}

fn chain(world: &World) -> Value {
    serde_json::from_str(&world.remote(&[
        "show",
        "-s",
        "--format=%B",
        "refs/convoy/candidates/main",
    ]))
    .unwrap()
}

fn gone(world: &World, name: &str) {
    for kind in ["gate", "child"] {
        let pid = std::fs::read_to_string(world.root.join(format!("{kind}-{name}"))).unwrap();
        until(|| {
            let ps = Command::new("ps").args(["-p", pid.trim(), "-o", "stat="]).output().unwrap();
            let status = String::from_utf8_lossy(&ps.stdout);
            status.trim().is_empty() || status.trim().starts_with('Z')
        });
    }
}

/// A turn stopped by SIGSTOP until dropped, so a panic never leaves it frozen.
struct Held(String);

impl Held {
    fn stop(turn: &Child) -> Held {
        let pid = turn.id().to_string();
        assert!(Command::new("kill").args(["-STOP", &pid]).status().unwrap().success());
        Held(pid)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = Command::new("kill").args(["-CONT", &self.0]).status();
    }
}

/// Starts the turns of `turns`, each a worker's checkout and its issue, into
/// one round (docs/adr/0083): a turn of issue 5's worker, `e`, takes a round
/// of its own first and holds it in its gate; the others ask during it and
/// wait pending, in the order given; `release_holder` lets its gate go, it
/// lands, and the next round takes them all. They would stack on its round
/// as they ask while its whole gate runs (sunfmin/control#1140): a barrier's
/// place the holder's process holds keeps them pending until it ends.
fn one_round(
    world: &World,
    holder: &Path,
    turns: &[(&Path, u64)],
    began_holder: impl Fn(),
    release_holder: impl Fn(),
) -> Vec<Child> {
    let held = start(world, holder, &world.cache(), 5);
    began_holder();
    hold_pending(world);
    // One at a time, each pending before the next asks: the order they ask
    // in is the round's.
    let mut children: Vec<Child> = vec![];
    for (dir, n) in turns {
        children.push(start(world, dir, &world.cache(), *n));
        let asked = children.len();
        until(|| chain(world)["pending"].as_array().is_some_and(|p| p.len() == asked));
    }
    release_holder();
    let landed = finish(held);
    assert_eq!(landed["outcome"], "merged", "the holder's round: {landed}");
    children
}

/// Adds to the chain a barrier's place held by the process of the open
/// round's first PR's turn: turns that ask wait pending while it runs, and
/// the place goes as that process ends, as a dead owner's does.
fn hold_pending(world: &World) {
    let name = "refs/convoy/candidates/main";
    loop {
        let old = world.remote(&["rev-parse", name]);
        let mut state = chain(world);
        let owner = state["round"]["asked"][0]["owner"].clone();
        assert!(owner.is_object(), "no round holds a turn: {state}");
        let barriers = state["barriers"].as_array_mut().unwrap();
        barriers.push(json!({"id": "held-by-the-holder", "owner": owner}));
        let tree = world.remote(&["hash-object", "-w", "-t", "tree", "/dev/null"]);
        let message = state.to_string();
        let new = world.remote(&["commit-tree", &tree, "-p", &old, "-m", &message]);
        let moved = Command::new("git")
            .current_dir(&world.bare)
            .args(["update-ref", name, &new, &old])
            .status()
            .unwrap();
        if moved.success() {
            return;
        }
    }
}

/// The pairs of the open round's pairing tree, each as its level and the
/// PR whose turn gates it.
fn pairs(world: &World) -> Vec<(u64, u64)> {
    let state = chain(world);
    let rows = state["candidates"].as_array().cloned().unwrap_or_default();
    rows.iter()
        .filter(|c| !c["pair"].is_null())
        .map(|c| (c["pair"]["level"].as_u64().unwrap(), c["pr"].as_u64().unwrap()))
        .collect()
}

/// The candidates of the chain, without the pairs beside it.
fn chained(world: &World) -> Vec<Value> {
    let state = chain(world);
    let rows = state["candidates"].as_array().cloned().unwrap_or_default();
    rows.into_iter().filter(|c| c["pair"].is_null()).collect()
}

/// Moves the head of the worker's branch in `dir` on by one commit, pushed.
fn move_head(dir: &Path) {
    commit(dir, "changed.txt", "new head");
    git(dir, &["push", "-q", "origin", "HEAD"]);
}

/// The pids of the gate of `name` and of what it started, as it wrote them.
fn gate_pids(world: &World, name: &str) -> Vec<String> {
    ["gate", "child"]
        .into_iter()
        .map(|kind| {
            let kept = std::fs::read_to_string(world.root.join(format!("{kind}-{name}"))).unwrap();
            kept.trim().to_string()
        })
        .collect()
}

/// Every pid of `pids` has ended.
fn ended_all(pids: &[String]) {
    for pid in pids {
        until(|| {
            let ps = Command::new("ps").args(["-p", pid, "-o", "stat="]).output().unwrap();
            let status = String::from_utf8_lossy(&ps.stdout);
            status.trim().is_empty() || status.trim().starts_with('Z')
        });
    }
}

fn worker(world: &World, n: u64, name: &str) -> (PathBuf, String) {
    let dir = world.worker(n, &format!("{name}.txt"), name);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    (dir, head)
}

#[test]
fn a_candidates_gate_never_finds_the_checkouts_configuration_above_it() {
    let world = World::new();
    commit(&world.clone, "checkout-config-marker", "**/*\n");
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    world.set_gate(
        r#"parent=$(dirname "$PWD"); while :; do
test ! -f "$parent/checkout-config-marker" || exit 9
test "$parent" != / || break
parent=$(dirname "$parent")
done"#,
    );
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (source, _) = worker(&world, 1, "a");
    let landed = world.afk_in(&source, &["land", "--issue", "1"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let candidate = candidate_of(&world, 1);
    let path = Path::new(candidate["worktree"].as_str().unwrap());
    assert!(path.starts_with(world.cache().join("afkconvoy/candidates")), "{candidate}");
    assert!(!path.starts_with(&world.clone));
    assert_eq!(candidate["phase"], "landed");
    assert!(!path.exists(), "the stopped candidate is removed from the new root");
    assert!(
        !world.remote(&["for-each-ref", "--format=%(objectname)", "refs/convoy/gate"]).is_empty()
    );
}

#[test]
fn a_cache_inside_any_checkout_is_refused_before_a_candidate_is_assigned() {
    let world = World::new();
    world.set_gate("true");
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (source, _) = worker(&world, 1, "a");
    let other = world.second_repo();
    for checkout in [&world.clone, &source, &other] {
        let out = world
            .command(env!("CARGO_BIN_EXE_afkconvoy"), &source)
            .env("XDG_CACHE_HOME", checkout.join(".candidate-cache"))
            .args(["land", "--issue", "1"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
        let error = answer["error"].as_str().unwrap();
        assert!(
            error.contains("candidate cache") && error.contains("is inside checkout"),
            "{error}"
        );
        assert!(error.contains("set XDG_CACHE_HOME"), "{error}");
    }
    assert!(
        world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/candidates"]).is_empty()
    );
}

#[test]
fn stopped_candidates_in_the_old_root_are_removed_and_other_directories_are_left() {
    let world = World::new();
    world.set_gate("true");
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (source, _) = worker(&world, 1, "a");
    let landed = world.afk_in(&source, &["land", "--issue", "1"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let mut state = chain(&world);
    let template = candidate_of(&world, 1);
    let path = Path::new(template["worktree"].as_str().unwrap());
    let root = path.parent().unwrap();
    let legacy = world.clone.join(".git/convoy-candidates/legacy");
    git(
        &world.clone,
        &["worktree", "add", "--quiet", "--detach", legacy.to_str().unwrap(), "HEAD"],
    );
    let paths = [
        ("legacy", legacy.clone(), true),
        ("leftover", root.join("leftover"), true),
        ("outside", world.root.join("outside"), false),
        ("nested", root.join("extra/nested"), false),
        ("wrong-id", root.join("different-id"), false),
        ("foreign", root.parent().unwrap().join("another-repo/foreign"), false),
    ];
    for (id, path, _) in &paths {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("keep"), id).unwrap();
        std::fs::write(format!("{}.processes", path.display()), "scope").unwrap();
        let mut row = template.clone();
        row["id"] = json!(id);
        row["worktree"] = json!(path);
        state["candidates"].as_array_mut().unwrap().push(row);
    }
    let file = world.root.join("seed.json");
    std::fs::write(&file, state.to_string()).unwrap();
    let tip = chain_tip(&world);
    let tree = world.remote(&["hash-object", "-t", "tree", "/dev/null"]);
    let seeded = world.remote(&["commit-tree", &tree, "-p", &tip, "-F", file.to_str().unwrap()]);
    world.remote(&["update-ref", "refs/convoy/candidates/main", &seeded]);
    tick(&world);
    for (_, path, removed) in &paths {
        assert_eq!(path.exists(), !removed, "{}", path.display());
        assert_eq!(Path::new(&format!("{}.processes", path.display())).exists(), !removed);
    }
    assert!(!listed(&world, &legacy));
    assert!(listed(&world, &source));
    assert_eq!(chain_tip(&world), seeded, "removal leaves the candidate evidence intact");
}

#[test]
fn interleaved_targets_and_default_publish_only_to_their_own_chains() {
    let world = world(3);
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    for target in ["delivery/one", "delivery/two"] {
        git(&world.clone, &["push", "-q", "origin", &format!("HEAD:refs/heads/{target}")]);
    }
    let a = worker(&world, 1, "a").0;
    let b = worker(&world, 2, "b").0;
    let c = worker(&world, 3, "c").0;
    for (n, target) in [(1, "delivery/one"), (2, "delivery/two")] {
        world.afk(&["target", "--issue", &n.to_string(), "--branch", target]).ok();
        world.with_gh(|gh| gh["prs"][(n - 1) as usize]["baseRefName"] = json!(target));
    }
    let _runner = world.host_held();
    let first = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let second = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
    let default = start(&world, &c, &world.cache(), 3);
    began(&world, "c");
    // A fresh process with no resident memory sees all three destinations.
    let rows = world.afk(&["doctor"]).json["candidates"].clone();
    for target in ["delivery/one", "delivery/two", "main"] {
        assert!(rows.as_array().unwrap().iter().any(|c| c["target_branch"] == target), "{rows}");
    }
    release(&world, "b", false);
    let second = finish(second);
    assert_eq!(second["outcome"], "merged", "{second}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/delivery/one"]), base);
    release(&world, "c", false);
    let default = finish(default);
    release(&world, "a", false);
    let first = finish(first);
    for (target, file, result) in
        [("delivery/one", "a", first), ("delivery/two", "b", second), ("main", "c", default)]
    {
        assert_eq!(result["outcome"], "merged", "{result}");
        assert_eq!(result["predecessor"], base, "no cross-target predecessor");
        assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{target}")]), result["commit"]);
        let files = world.remote(&["ls-tree", "--name-only", &format!("refs/heads/{target}")]);
        for other in ["a", "b", "c"] {
            assert_eq!(
                files.lines().any(|f| f == format!("{other}.txt")),
                other == file,
                "{target}: {files}"
            );
        }
        let record: Value = serde_json::from_str(&world.remote(&[
            "show",
            "-s",
            "--format=%B",
            &format!("refs/convoy/candidates/{target}"),
        ]))
        .unwrap();
        assert_eq!(record["candidates"].as_array().unwrap().len(), 1);
    }
    assert!(world.gh()["issues"].as_array().unwrap().iter().all(|i| i["state"] == "closed"));
}

#[test]
fn land_of_a_closed_issue_names_it_and_asks_for_no_turn() {
    let world = World::new();
    let gated = world.root.join("gated");
    world.set_gate(&format!("touch {}", quote(&gated)));
    let (source, _) = worker(&world, 1, "a");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    world.close_issue(1);
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &source)
        .args(["land", "--issue", "1"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let error = answer["error"].as_str().unwrap();
    assert!(error.contains("issue #1 is closed; nothing lands for it"), "{error}");
    assert!(!gated.exists(), "no gate ran: the turn was never asked for");
    assert!(
        world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/candidates"]).is_empty(),
        "no candidate was assigned"
    );
    assert_eq!(world.afk(&["doctor"]).json["host"]["exclusive"]["waiting"], json!([]));
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
}

#[test]
fn a_candidate_whose_issue_closed_during_validation_lands_nothing() {
    let world = world(2);
    let a = worker(&world, 1, "a").0;
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let _runner = world.host_held();
    let child = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    world.close_issue(1);
    release(&world, "a", false);
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stdout).unwrap();
    let said = error["error"].as_str().unwrap();
    assert!(said.contains("issue #1 is closed; nothing lands for it"), "{error}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base, "nothing landed");
    let candidate = candidate_of(&world, 1);
    assert_ne!(candidate["phase"], "landed", "{candidate}");
}

#[test]
fn a_pr_retargeted_during_validation_is_invalidated_without_publishing() {
    let world = world(2);
    git(&world.clone, &["push", "-q", "origin", "HEAD:delivery/one"]);
    let a = worker(&world, 1, "a").0;
    world.afk(&["target", "--issue", "1", "--branch", "delivery/one"]).ok();
    world.with_gh(|gh| gh["prs"][0]["baseRefName"] = json!("delivery/one"));
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let _runner = world.host_held();
    let child = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    world.with_gh(|gh| gh["prs"][0]["baseRefName"] = json!("main"));
    release(&world, "a", false);
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(error["error"].as_str().unwrap().contains("pinned target"), "{error}");
    for target in ["main", "delivery/one"] {
        assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{target}")]), base);
    }
    assert_eq!(world.gh()["issues"][0]["state"], "open");
}

/// Four ready, mutually clean PRs (docs/adr/0083): the turn builds two
/// pairs and fast-gates both at once, then their pair, then gates the root
/// merged onto the tip whole once, and lands all four in one fast-forward;
/// each issue closes.
#[test]
fn four_ready_prs_pair_twice_gate_their_pair_and_land_in_one_fast_forward() {
    let world = split_world(3);
    let (e, _) = worker(&world, 5, "e");
    let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c"), (4, "d")]
        .into_iter()
        .map(|(n, name)| worker(&world, n, name))
        .collect();
    let _runner = world.host_held();
    let turns: Vec<(&Path, u64)> =
        workers.iter().zip(1..).map(|((dir, _), n)| (dir.as_path(), n)).collect();
    let children = one_round(
        &world,
        &e,
        &turns,
        || until(|| started(&world, "fast", "e")),
        || {
            let_go(&world, "fast", "e", false);
            let_go(&world, "slow", "e", false);
        },
    );
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    // The two pairs gate at once, each by its later-asked PR's turn.
    until(|| started(&world, "fast", "b") && started(&world, "fast", "d"));
    assert!(!started(&world, "fast", "a") && !started(&world, "fast", "c"));
    assert_eq!(pairs(&world), [(0, 20), (0, 40)]);
    let_go(&world, "fast", "b", false);
    let_go(&world, "fast", "d", false);
    // Their pair, then the root's whole gate, at the last-asked PR's turn.
    until(|| started(&world, "slow", "d"));
    assert_eq!(pairs(&world), [(0, 20), (0, 40), (1, 40)]);
    for name in ["a", "b", "c"] {
        assert!(!started(&world, "slow", name), "the root is gated whole once");
    }
    let rows = chained(&world);
    assert_eq!(rows.len(), 4, "one candidate of the root per PR: {rows:?}");
    assert_eq!(rows[0]["predecessor"], base);
    for i in 1..4 {
        assert_eq!(rows[i]["predecessor"], rows[i - 1]["commit"]);
    }
    let_go(&world, "slow", "d", false);
    let answers: Vec<Value> = children.into_iter().map(finish).collect();
    for answer in &answers {
        assert_eq!(answer["outcome"], "merged", "{answer}");
    }
    assert_eq!(answers[3]["landed"], json!([10, 20, 30, 40]), "{}", answers[3]);
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        rows[3]["commit"].as_str().unwrap()
    );
    let history =
        world.remote(&["rev-list", "--first-parent", &format!("{base}..refs/heads/main")]);
    assert_eq!(history.lines().count(), 4, "one fast-forward through a merge of each");
    for n in 1..=4 {
        assert_eq!(world.state(n), "closed");
        assert_eq!(
            git(&workers[n as usize - 1].0, &["rev-parse", "HEAD"]),
            workers[n as usize - 1].1
        );
    }
}

/// A pair whose fast gate is red stops no other pair (docs/adr/0083): its
/// later-asked PR's turn is given the red run, its earlier one goes up
/// unpaired, and the rest of the round lands.
#[test]
fn a_red_pair_stops_no_other_its_later_pr_gets_the_run_and_the_rest_lands() {
    let world = split_world(3);
    let (e, _) = worker(&world, 5, "e");
    let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c"), (4, "d")]
        .into_iter()
        .map(|(n, name)| worker(&world, n, name))
        .collect();
    let _runner = world.host_held();
    let turns: Vec<(&Path, u64)> =
        workers.iter().zip(1..).map(|((dir, _), n)| (dir.as_path(), n)).collect();
    let mut children = one_round(
        &world,
        &e,
        &turns,
        || until(|| started(&world, "fast", "e")),
        || {
            let_go(&world, "fast", "e", false);
            let_go(&world, "slow", "e", false);
        },
    );
    until(|| started(&world, "fast", "b") && started(&world, "fast", "d"));
    let_go(&world, "fast", "b", true);
    let b = children.remove(1);
    let red = finish(b);
    assert_eq!(red["outcome"], "gate_red", "{red}");
    assert_eq!(red["gate"], "fast");
    assert_eq!(red["pair"]["with"], json!([10]), "{red}");
    assert!(red["detail"].as_str().unwrap().contains("#10"), "{red}");
    let_go(&world, "fast", "d", false);
    until(|| started(&world, "slow", "d"));
    let_go(&world, "slow", "d", false);
    for answer in children.into_iter().map(finish) {
        assert_eq!(answer["outcome"], "merged", "{answer}");
    }
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    for (name, landed) in [("a", true), ("b", false), ("c", true), ("d", true)] {
        assert_eq!(files.lines().any(|f| f == format!("{name}.txt")), landed, "{files}");
    }
    assert_eq!(world.state(2), "open");
    assert_eq!(git(&workers[1].0, &["rev-parse", "HEAD"]), workers[1].1, "its branch stays");
}

#[test]
fn a_gate_past_its_limit_is_red_and_terminated() {
    let world = world(2);
    let config = std::fs::read_to_string(world.clone.join("docs/agents/afkconvoy.md"))
        .unwrap()
        .replace("gate_seconds: 600", "gate_seconds: 4");
    commit(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (a, _) = worker(&world, 1, "a");
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let failed = finish(aa);
    assert_eq!(failed["outcome"], "gate_red", "{failed}");
    assert_eq!(failed["timed_out"], true);
    gone(&world, "a");
}

#[test]
fn a_stalled_gate_record_push_releases_the_lane_without_recording_green() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let started = world.root.join("push-started");
    let hook = format!(
        "#!/bin/sh\nwhile read old new ref; do\n  case \"$ref\" in refs/convoy/gate/*)\n    \
         if test ! -f '{0}'; then touch '{0}'; sleep 30; fi;;\n  esac\ndone\n",
        started.display()
    );
    let hook_path = world.bare.join("hooks/pre-receive");
    std::fs::write(&hook_path, hook).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let _runner = world.host_held();
    let mut aa = start_with_record_push_limit(&world, &a, &world.cache(), 1, Some(1));
    until(|| world.root.join("start-a").exists() || aa.try_wait().unwrap().is_some());
    if aa.try_wait().unwrap().is_some() {
        panic!("the first turn ended before its gate: {}", finish(aa));
    }
    release(&world, "a", false);
    until(|| started.exists() || aa.try_wait().unwrap().is_some());
    if aa.try_wait().unwrap().is_some() {
        panic!("the first turn ended before the push stalled: {}", finish(aa));
    }
    let mut bb = start(&world, &b, &world.cache(), 2);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!world.root.join("start-b").exists(), "the stalled push held the only worker lane");
    assert!(bb.try_wait().unwrap().is_none(), "the queued turn is still waiting");

    let failed = finish(aa);
    assert_eq!(failed["outcome"], "gate_red", "{failed}");
    assert_eq!(failed["status"], "green", "the tests passed before the push stalled");
    assert_eq!(failed["exit_code"], 0);
    assert_eq!(failed["timed_out"], false, "the gate did not reach its own limit");
    assert_eq!(failed["recorded"], false);
    let not_recorded = failed["not_recorded"].as_str().unwrap();
    assert!(not_recorded.contains("did not end within 1s"), "{failed}");
    assert!(not_recorded.contains("Git reported "), "{failed}");
    assert!(not_recorded.contains(" sent before the push was ended"), "{failed}");
    assert_eq!(world.state(1), "open");

    began(&world, "b");
    release(&world, "b", false);
    let landed = finish(bb);
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    assert!(files.lines().any(|file| file == "b.txt"));
    assert!(!files.lines().any(|file| file == "a.txt"));
    gone(&world, "a");
    gone(&world, "b");
}

#[test]
fn a_changed_head_cancels_its_candidate_and_an_external_base_move_invalidates_the_chain() {
    for head_change in [true, false] {
        let world = world(2);
        let (a, _) = worker(&world, 1, "a");
        let _runner = world.host_held();
        let aa = start(&world, &a, &world.cache(), 1);
        began(&world, "a");
        if head_change {
            move_head(&a);
        } else {
            commit(&world.clone, "outside.txt", "outside");
            git(&world.clone, &["push", "-q", "origin", "HEAD"]);
        }
        let answer = finish(aa);
        let expected = if head_change { "cancelled" } else { "target_moved" };
        assert_eq!(answer["outcome"], expected, "{answer}");
        gone(&world, "a");
        assert_eq!(world.state(1), "open");
    }
}

#[test]
fn dead_owner_is_cancelled_before_replacement_and_duplicate_requests_never_start_two_gates() {
    let world = world(2);
    let (a, head) = worker(&world, 1, "a");
    let _runner = world.host_held();
    let mut first = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let duplicate = start(&world, &a, &world.cache(), 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(chain(&world)["candidates"].as_array().unwrap().len(), 1);
    first.kill().unwrap();
    let _ = first.wait();
    until(|| chain(&world)["candidates"][0]["stopped"] == true);
    gone(&world, "a");
    release(&world, "a", false);
    assert_eq!(finish(duplicate)["outcome"], "merged");
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
}

/// The PRs that asked to land are taken into a round as it starts, its root
/// one chain of candidates; a PR asking during the round waits for the next
/// (docs/adr/0083). An odd PR goes up a level unpaired.
#[test]
fn a_round_takes_the_prs_that_asked_and_one_asking_during_it_waits_for_the_next() {
    let world = world(3);
    let (e, _) = worker(&world, 5, "e");
    let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c"), (4, "d")]
        .into_iter()
        .map(|(n, name)| worker(&world, n, name))
        .collect();
    let _runner = world.host_held();
    let turns: Vec<(&Path, u64)> =
        workers[..3].iter().zip(1..).map(|((dir, _), n)| (dir.as_path(), n)).collect();
    let children =
        one_round(&world, &e, &turns, || began(&world, "e"), || release(&world, "e", false));
    began(&world, "b");
    assert_eq!(pairs(&world), [(0, 20)], "c goes up unpaired");
    let mut fourth = start(&world, &workers[3].0, &world.cache(), 4);
    until(|| chain(&world)["pending"].as_array().is_some_and(|p| p.len() == 1));
    release(&world, "b", false);
    began(&world, "c");
    assert_eq!(pairs(&world), [(0, 20), (1, 30)]);
    std::thread::sleep(Duration::from_millis(200));
    assert!(fourth.try_wait().unwrap().is_none());
    assert!(!world.root.join("start-d").exists(), "it asked during the round");
    release(&world, "c", false);
    for child in children {
        let result = finish(child);
        assert_eq!(result["outcome"], "merged", "{result}");
    }
    release(&world, "d", false);
    assert_eq!(finish(fourth)["outcome"], "merged");
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    for name in ["a", "b", "c", "d", "e"] {
        assert!(files.lines().any(|f| f == format!("{name}.txt")), "{files}");
    }
}

#[test]
fn semantic_blockers_hold_a_pending_turn_and_a_live_lock_blocks_a_waiting_gate() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &b, &world.cache(), 2);
    until(|| chain(&world)["pending"].as_array().is_some_and(|p| !p.is_empty()));
    world.issue(9, &["fleet-lock"]);
    release(&world, "a", false);
    assert_eq!(finish(aa)["outcome"], "locked");
    assert_eq!(finish(bb)["outcome"], "locked");
    assert!(!world.root.join("start-b").exists());
    world.with_gh(|gh| {
        for issue in gh["issues"].as_array_mut().unwrap() {
            if issue["number"] == 9 {
                issue["state"] = json!("closed");
            }
        }
        gh["edges"] = json!({"2":[1]});
    });
    let blocked = finish(start(&world, &b, &world.cache(), 2));
    assert_eq!(blocked["outcome"], "blocked", "{blocked}");
    assert_eq!(blocked["blocked_by"], json!([1]));
    assert!(!world.root.join("start-b").exists());
}

#[test]
fn publication_survives_issue_settlement_failure_without_a_duplicate_gate() {
    let world = world(1);
    let (a, head) = worker(&world, 1, "a");
    world.with_gh(|gh| gh["fail"] = json!(["state=closed"]));
    release(&world, "a", false);
    // The landing is published: a close that fails does not unsay it
    // (sunfmin/control#247).
    let landed = finish(start(&world, &a, &world.cache(), 1));
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert!(landed["settle_error"].as_str().is_some_and(|e| !e.is_empty()), "{landed}");
    let state = chain(&world);
    assert_eq!(state["candidates"][0]["phase"], "landed");
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        state["candidates"][0]["commit"].as_str().unwrap()
    );
    assert_eq!(world.state(1), "open");
    world.with_gh(|gh| gh["fail"] = json!([]));
    let recovered = finish(start(&world, &a, &world.cache(), 1));
    assert_eq!(recovered["outcome"], "merged");
    assert!(recovered.get("settle_error").is_none(), "{recovered}");
    assert_eq!(world.state(1), "closed");
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
    assert_eq!(chain(&world)["candidates"].as_array().unwrap().len(), 1);
}

#[test]
fn a_close_refused_while_github_closes_the_issue_itself_is_a_close() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    // GitHub closes the issue as the merged PR reaches main, and refuses
    // the turn's own close at that moment. Closed before the push, the turn
    // would land nothing for it (sunfmin/control#1081).
    world.with_gh(|gh| gh["fail"] = json!(["state=closed"]));
    let hook = world.bare.join("hooks/post-receive");
    let gh = world.root.join("gh.json");
    let script = format!(
        r#"#!/bin/sh
while read old new ref; do
  case "$ref" in refs/heads/main)
    sed 's/"number":1,"state":"open"/"number":1,"state":"closed"/' {gh} > {gh}.closing
    mv {gh}.closing {gh}
  esac
done
"#,
        gh = quote(&gh)
    );
    std::fs::write(&hook, script).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    release(&world, "a", false);
    let landed = finish(start(&world, &a, &world.cache(), 1));
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert!(landed.get("settle_error").is_none(), "{landed}");
    assert_eq!(world.gh()["issues"][0]["state"], "closed", "the hook closed it at the push");
}

#[test]
fn a_head_moving_during_publication_cannot_land_the_stale_candidate() {
    use std::os::unix::fs::PermissionsExt;
    let world = world(1);
    let (a, original) = worker(&world, 1, "a");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let later = world.root.join("later-source");
    git(
        &world.clone,
        &["worktree", "add", "--quiet", "--detach", later.to_str().unwrap(), &original],
    );
    commit(&later, "late.txt", "a PR head advanced during publication");
    let changed = git(&later, &["rev-parse", "HEAD"]);
    git(&later, &["push", "--quiet", "origin", "HEAD:refs/heads/later-source"]);
    let hook = world.bare.join("hooks/pre-receive");
    let script = format!(
        r#"#!/bin/sh
while read old new ref; do
  case "$ref" in refs/heads/main)
    unset GIT_QUARANTINE_PATH GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES
    git update-ref refs/heads/issue-1 {changed} {original} || exit 1
    touch {}
  esac
done
"#,
        quote(&world.root.join("head-moved-in-transaction"))
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    release(&world, "a", false);
    let answer = finish(start(&world, &a, &world.cache(), 1));
    assert!(world.root.join("head-moved-in-transaction").exists());
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-1"]), changed);
    assert_ne!(
        answer["outcome"], "merged",
        "stale candidate landed after its source head changed: {answer}"
    );
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
}

#[test]
fn an_injected_clock_can_publish_its_own_fresh_record() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    release(&world, "a", false);
    let landed = world.afk_in(&a, &["land", "--issue", "1", "--now", "1000"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
}

#[test]
fn runner_restart_keeps_running_candidate_ownership_and_capacity() {
    let world = world(2);
    let (a, _) = worker(&world, 1, "a");
    let first_runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    world.kill_host_runner("KILL");
    drop(first_runner);
    let _replacement = world.host_held();
    let mut restored = Value::Null;
    until(|| {
        restored = world.afk(&["queue"]).json;
        restored["status"] == "available"
    });
    assert_eq!(restored["worker"]["used"], 1, "{restored}");
    assert!(restored["running"].as_array().unwrap().iter().any(|r| r["issue"] == 1), "{restored}");
    let ordinary_marker = world.root.join("ordinary-started");
    let ordinary_script = format!("touch {}", quote(&ordinary_marker));
    let ordinary = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &a)
        .args(["exclusive", "--", "sh", "-c", &ordinary_script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(!ordinary_marker.exists(), "ordinary work retains repo exclusion after restart");
    release(&world, "a", false);
    assert_eq!(finish(aa)["outcome"], "merged");
    assert!(ordinary.wait_with_output().unwrap().status.success());
    assert!(ordinary_marker.exists());
}

#[test]
fn ordinary_work_recovers_an_orphan_gate_after_owner_and_runner_death() {
    let world = world(2);
    let (a, head) = worker(&world, 1, "a");
    let runner = world.host_held();
    let mut aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    world.kill_host_runner("KILL");
    drop(runner);
    aa.kill().unwrap();
    let _ = aa.wait();
    let _replacement = world.host_held();
    let marker = world.root.join("ordinary-recovered");
    let script = format!("touch {}", quote(&marker));
    let mut ordinary = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &a)
        .args(["exclusive", "--", "sh", "-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    until(|| marker.exists() || ordinary.try_wait().unwrap().is_some());
    let output = ordinary.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(marker.exists());
    gone(&world, "a");
    let state = chain(&world);
    assert_eq!(state["candidates"][0]["phase"], "cancelled");
    assert_eq!(state["candidates"][0]["stopped"], true);
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
}

#[test]
fn a_reparented_descendant_is_still_owned_and_terminated() {
    let world = world(2);
    let gate = std::fs::read_to_string(world.clone.join("gate.sh")).unwrap().replace(
        "sleep 120 &",
        "sh -c 'sleep 120 & echo $! > \"$1/escaped\"' sh \"$root\"\nsleep 120 &",
    );
    commit(&world.clone, "gate.sh", &gate);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (a, _) = worker(&world, 1, "a");
    let _runner = world.host_held();
    let child = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let pid = std::fs::read_to_string(world.root.join("escaped")).unwrap();
    release(&world, "a", true);
    assert_eq!(finish(child)["outcome"], "gate_red");
    let ps = Command::new("ps").args(["-p", pid.trim(), "-o", "stat="]).output().unwrap();
    let status = String::from_utf8_lossy(&ps.stdout);
    assert!(status.trim().is_empty() || status.trim().starts_with('Z'), "{status}");
}

#[test]
fn restart_after_durable_cancellation_finishes_cleanup_before_rebuilding() {
    use std::os::unix::fs::PermissionsExt;
    let world = world(2);
    let (a, _) = worker(&world, 1, "a");
    let runner = world.host_held();
    let mut aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let old = gate_pids(&world, "a");
    std::fs::write(world.root.join("owner"), aa.id().to_string()).unwrap();
    let hook = world.bare.join("hooks/post-receive");
    let script = format!(
        r#"#!/bin/sh
root={}
while read old new ref; do
  case "$ref" in refs/convoy/candidates/main)
    text=$(git show -s --format=%B "$new")
    case "$text" in *'"phase":"cancelling"'*)
      if mkdir "$root/cancellation-checkpoint" 2>/dev/null; then
        kill -STOP "$(cat "$root/owner")"
        touch "$root/cancellation-durable"
      fi;;
    esac;;
  esac
done
"#,
        quote(&world.root)
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Its head moves: its own turn cancels its candidate, durably, and is
    // frozen and killed before it stops the gate.
    move_head(&a);
    until(|| world.root.join("cancellation-durable").exists());
    aa.kill().unwrap();
    let _ = aa.wait();
    world.kill_host_runner("KILL");
    drop(runner);
    let _replacement = world.host_held();
    // The next turn finishes the cleanup before it gates anything again.
    let rebuilt = start(&world, &a, &world.cache(), 1);
    ended_all(&old);
    until(|| chain(&world)["candidates"][0]["phase"] == "cancelled");
    let state = chain(&world);
    let first = &state["candidates"][0];
    assert_eq!(first["phase"], "cancelled", "{state}");
    assert_eq!(first["cause"], "head_changed", "{state}");
    release(&world, "a", false);
    let rebuilt = finish(rebuilt);
    assert_eq!(rebuilt["outcome"], "merged", "{rebuilt}");
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    assert!(files.lines().any(|f| f == "changed.txt"), "its new head landed: {files}");
}

/// A cancellation that froze a gate and cannot record what it froze still
/// kills it: the turn fails, and no gate, nor what it started, is left
/// stopped (sunfmin/control#508).
#[test]
fn a_cancellation_that_cannot_record_its_frozen_gate_still_kills_it() {
    use std::os::unix::fs::PermissionsExt;
    let world = world(2);
    let (a, _) = worker(&world, 1, "a");
    let _runner = world.host_held();
    let mut aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let hook = world.bare.join("hooks/pre-receive");
    let script = r#"#!/bin/sh
while read old new ref; do
  case "$ref" in refs/convoy/candidates/main)
    case "$(git show -s --format=%B "$new")" in *'"descendants":[{'*)
      echo "the frozen tree is never recorded" >&2
      exit 1;;
    esac;;
  esac
done
"#;
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    move_head(&a);
    let began = Instant::now();
    while aa.try_wait().unwrap().is_none() {
        if began.elapsed() > Duration::from_secs(60) {
            let _ = aa.kill();
            let _ = aa.wait();
            panic!("a turn whose cancellation cannot be recorded never ended");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    gone(&world, "a");
}

fn worktrees(world: &World) -> String {
    git(&world.clone, &["worktree", "list", "--porcelain"])
}

fn listed(world: &World, path: &Path) -> bool {
    worktree_listed(&worktrees(world), path)
}

fn candidate_path(world: &World, issue: u64) -> PathBuf {
    let row = chain(world)["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["issue"] == issue)
        .unwrap()
        .clone();
    PathBuf::from(row["worktree"].as_str().unwrap())
}

/// The gate leaves a mode 0555 directory behind. It is gitignored, so the run
/// is still of the committed tree.
fn seal_gate(world: &World) {
    let script = std::fs::read_to_string(world.clone.join("gate.sh")).unwrap();
    let script = script.replace(
        "test ! -f \"$root/fail-$name\"\n",
        "mkdir -p sealed && chmod 0555 sealed\ntest ! -f \"$root/fail-$name\"\n",
    );
    assert!(script.contains("chmod 0555 sealed"), "the gate script was not rewritten");
    commit(&world.clone, ".gitignore", "sealed/\n");
    commit(&world.clone, "gate.sh", &script);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    // The base gate ran in the clone and left its own sealed directory. The
    // candidate worktrees are what the product removes; this one is the test's.
    let sealed = world.clone.join("sealed");
    if sealed.is_dir() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn chain_tip(world: &World) -> String {
    world.remote(&["rev-parse", "refs/convoy/candidates/main"])
}

/// The look's one listing of the remote. It is the only `ls-remote` whose
/// patterns include the candidate chain.
const LOOK_LISTING: &str = "ls-remote origin refs/convoy/* refs/heads/* refs/tags/v*";

fn clear_git_log(world: &World) {
    std::fs::write(world.root.join("git.log"), "").unwrap();
}

fn lists_candidate_chain(call: &str) -> bool {
    call.starts_with("ls-remote ")
        && call
            .split_whitespace()
            .any(|arg| arg == "refs/convoy/*" || arg.starts_with("refs/convoy/candidates"))
}

/// A tick that does not write the candidate chain lists it once, as part of
/// the look, and does not read that chain on its own.
fn assert_chain_listed_once(world: &World) {
    let calls = world.git_calls();
    let listed: Vec<&String> = calls.iter().filter(|call| lists_candidate_chain(call)).collect();
    assert_eq!(listed, [&LOOK_LISTING.to_string()], "{calls:#?}");
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("push ") && call.contains("refs/convoy/candidates")),
        "the tick wrote the candidate chain: {calls:#?}"
    );
}

/// The startup probe, when it runs, lists the fleet's refs to sweep litter.
/// Removal still follows the look, and nothing reads the candidate chain alone.
fn assert_removal_follows_the_look(world: &World) {
    let calls = world.git_calls();
    assert!(
        !calls.iter().any(|call| {
            call.split_whitespace().any(|arg| arg.starts_with("refs/convoy/candidates"))
        }),
        "the tick read the candidate chain on its own: {calls:#?}"
    );
    let look = calls.iter().position(|call| call == LOOK_LISTING);
    let removed = calls.iter().position(|call| call.starts_with("worktree remove "));
    assert!(
        look.is_some_and(|look| removed.is_some_and(|removed| removed > look)),
        "removal follows the look's listing: {calls:#?}"
    );
}

fn tick(world: &World) {
    // The gate records were written at the real clock. A tick dated in 1970
    // treats them as past their day and sweeps them; this tick is now.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok();
}

/// Kills the process it holds, however the test ends.
struct Hold(Child);

impl Drop for Hold {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_gating_candidate_keeps_its_worktree_and_a_stopped_one_loses_it() {
    use std::os::unix::fs::PermissionsExt;
    let world = world(2);
    seal_gate(&world);
    let (issue_a, _) = worker(&world, 1, "a");
    let _runner = world.host_held();
    let aa = start(&world, &issue_a, &world.cache(), 1);
    began(&world, "a");
    let a_tree = candidate_path(&world, 1);
    assert_eq!(chained(&world)[0]["phase"], "validating");
    assert!(listed(&world, &a_tree), "a validating candidate stays listed");
    let hold = Hold(Command::new("sleep").arg("300").current_dir(&a_tree).spawn().unwrap());
    release(&world, "a", false);
    let landed_a = finish(aa);
    assert_eq!(landed_a["outcome"], "merged", "{landed_a}");
    assert!(a_tree.is_dir(), "a worktree a process is working in stays");
    assert!(listed(&world, &a_tree));
    assert!(listed(&world, &issue_a), "the issue worktree stays");
    let candidate = chained(&world).into_iter().find(|c| c["issue"] == 1).unwrap();
    assert_eq!(candidate["phase"], "landed");
    assert_eq!(candidate["commit"], landed_a["commit"]);
    let gates = world.remote(&["for-each-ref", "--format=%(objectname)", "refs/convoy/gate"]);
    assert!(!gates.is_empty(), "the recorded gate run stays on the remote");
    let tip = chain_tip(&world);
    drop(hold);
    world.watch_git();
    tick(&world);
    assert!(!a_tree.exists(), "a later tick removes the worktree once the process is gone");
    assert!(!listed(&world, &a_tree));
    assert_removal_follows_the_look(&world);
    assert_eq!(chain_tip(&world), tip, "the candidate chain is not rewritten");
    assert_eq!(
        world.remote(&["for-each-ref", "--format=%(objectname)", "refs/convoy/gate"]),
        gates
    );
    assert!(issue_a.is_dir());
    std::fs::create_dir_all(&a_tree).unwrap();
    let sealed = a_tree.join("sealed");
    std::fs::create_dir(&sealed).unwrap();
    std::fs::write(sealed.join("kept"), "kept").unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert!(!listed(&world, &a_tree), "the registration is already gone");
    let tip = chain_tip(&world);
    clear_git_log(&world);
    tick(&world);
    assert!(!a_tree.exists(), "a half-removed worktree is removed");
    assert_chain_listed_once(&world);
    assert_eq!(chain_tip(&world), tip, "removing a leftover directory does not rewrite the chain");
    clear_git_log(&world);
    tick(&world);
    assert_chain_listed_once(&world);
    assert_eq!(chain_tip(&world), tip, "a tick that removes nothing leaves the chain");
}

#[test]
fn a_red_candidate_loses_its_worktree_and_the_chain_keeps_its_evidence() {
    let world = world(2);
    seal_gate(&world);
    let (issue_a, _) = worker(&world, 1, "a");
    let _runner = world.host_held();
    world.watch_git();
    let aa = start(&world, &issue_a, &world.cache(), 1);
    began(&world, "a");
    let a_tree = candidate_path(&world, 1);
    release(&world, "a", true);
    let red = finish(aa);
    assert_eq!(red["outcome"], "gate_red", "{red}");
    assert!(!a_tree.exists(), "a red candidate's worktree is gone");
    assert!(!listed(&world, &a_tree));
    let calls = world.git_calls();
    let wrote = calls
        .iter()
        .rposition(|call| call.starts_with("push ") && call.contains("refs/convoy/candidates/"));
    let reread = calls.iter().rposition(|call| call == "ls-remote origin refs/convoy/candidates/*");
    assert!(
        wrote.is_some_and(|wrote| reread.is_some_and(|reread| reread > wrote)),
        "a turn reads the chain it just wrote before it returns: {calls:#?}"
    );
    assert!(listed(&world, &issue_a), "the issue worktree stays");
    let failed = chained(&world).into_iter().find(|c| c["issue"] == 1).unwrap();
    assert_eq!(failed["phase"], "red");
    assert!(failed["result"]["excerpt"].is_array(), "the chain keeps the gate excerpt: {failed}");
    assert_eq!(failed["result"]["exit_code"], 1, "{failed}");
    assert!(failed["cause"].as_str().is_some_and(|cause| !cause.is_empty()), "{failed}");
}

#[test]
#[ignore = "controlled timing experiment; run explicitly through afkconvoy exclusive"]
fn measure_serial_and_parallel_batches() {
    let mut batches = vec![];
    for lanes in [1, 3] {
        let world = world(lanes);
        let gate=std::fs::read_to_string(world.clone.join("gate.sh")).unwrap().replace(
            "while test -d \"$root\" && test ! -f \"$root/release-$name\"; do sleep 0.02; done",
            "awk 'BEGIN { n=500000; for(i=0;i<n;i++)a[i]=i; for(j=0;j<40;j++)for(i=0;i<n;i++)s+=a[i]; print s }' >/dev/null");
        let gate = format!("{gate}\ntouch \"$root/end-$name\"\n");
        commit(&world.clone, "gate.sh", &gate);
        git(&world.clone, &["push", "-q", "origin", "HEAD"]);
        assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
        let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c")]
            .into_iter()
            .map(|(n, name)| worker(&world, n, name))
            .collect();
        let _runner = world.host_held();
        let began = Instant::now();
        let mut children = vec![];
        for (i, (dir, _)) in workers.iter().enumerate() {
            children.push(start(&world, dir, &world.cache(), i as u64 + 1));
        }
        let mut seconds = vec![];
        for child in children {
            let answer = finish(child);
            assert_eq!(answer["outcome"], "merged", "{answer}");
            seconds.push(answer["seconds"].clone());
        }
        let gate_wall: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|name| {
                let first = std::fs::metadata(world.root.join(format!("start-{name}")))
                    .unwrap()
                    .modified()
                    .unwrap();
                let last = std::fs::metadata(world.root.join(format!("end-{name}")))
                    .unwrap()
                    .modified()
                    .unwrap();
                last.duration_since(first).unwrap().as_secs_f64()
            })
            .collect();
        batches.push(json!({"lanes":lanes,"batch_seconds":began.elapsed().as_secs_f64(),"gate_seconds":seconds,"gate_wall_seconds":gate_wall,"discarded_gate_seconds":0}));
    }
    eprintln!(
        "CANDIDATE_BATCH_MEASUREMENT={}",
        json!({"version":afkconvoy::VERSION,"batches":batches})
    );
}

/// A world whose config declares a fast gate and a slow gate. Each part
/// waits for its own release file, `release-<part>-<name>`, and fails on
/// `fail-<part>-<name>`; the whole gate is the two in a row.
fn split_world(lanes: u64) -> World {
    let world = World::new();
    let root = quote(&world.root);
    let script = format!(
        r#"#!/bin/sh
set -e
root={root}
part=$1
name=
for n in a b c d e; do if test -f "$n.txt"; then name=$n; fi; done
case "${{AFKCONVOY_ISSUE:-}}" in 1) name=a;; 2) name=b;; 3) name=c;; 4) name=d;; 5) name=e;; esac
test -n "$name" || exit 0
touch "$root/start-$part-$name"
while test -d "$root" && test ! -f "$root/release-$part-$name"; do sleep 0.02; done
{said}
test ! -f "$root/fail-$part-$name"
"#,
        said = said("$part-$name")
    );
    commit(&world.clone, "gate.sh", &script);
    let afk = afkconvoy_core::line::Line::new().program(env!("CARGO_BIN_EXE_afkconvoy")).own();
    let config = format!(
        "```yaml\nbase_branch: main\ngate_command: \"{afk} exclusive -- sh ./gate.sh whole\"\ngate_seconds: 600\n```\n\n```yaml gates\nfast:\n  command: \"{afk} exclusive -- sh ./gate.sh fast\"\n  seconds: 300\n  budget_seconds: 200\nslow:\n  command: \"{afk} exclusive -- sh ./gate.sh slow\"\n  seconds: 300\n  budget_seconds: 200\n```\n"
    );
    std::fs::write(world.clone.join("docs/agents/afkconvoy.md"), config).unwrap();
    git(&world.clone, &["commit", "-q", "-am", "split the gate"]);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    world.say_host_settings(json!({"gate_lanes": lanes}));
    world
}

fn started(world: &World, part: &str, name: &str) -> bool {
    world.root.join(format!("start-{part}-{name}")).exists()
}

fn let_go(world: &World, part: &str, name: &str, red: bool) {
    if red {
        std::fs::write(world.root.join(format!("fail-{part}-{name}")), "").unwrap();
    } else {
        let _ = std::fs::remove_file(world.root.join(format!("fail-{part}-{name}")));
    }
    std::fs::write(world.root.join(format!("release-{part}-{name}")), "").unwrap();
}

#[test]
fn a_round_gates_its_pair_then_its_root_whole_once_and_lands_both_together() {
    let world = split_world(3);
    let (e, _) = worker(&world, 5, "e");
    let (a, ah) = worker(&world, 1, "a");
    let (b, bh) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let mut children = one_round(
        &world,
        &e,
        &[(&a, 1), (&b, 2)],
        || until(|| started(&world, "fast", "e")),
        || {
            let_go(&world, "fast", "e", false);
            let_go(&world, "slow", "e", false);
        },
    );
    until(|| started(&world, "fast", "b"));
    assert!(!started(&world, "fast", "a"), "the pair is its later-asked PR's to gate");
    let_go(&world, "fast", "b", false);
    until(|| started(&world, "slow", "b"));
    assert!(!started(&world, "slow", "a"), "only the round's last-asked PR runs its slow gate");
    let candidates = chained(&world);
    let_go(&world, "slow", "b", false);
    let bb = children.pop().unwrap();
    let aa = children.pop().unwrap();
    let (landed_a, landed_b) = (finish(aa), finish(bb));
    assert_eq!(landed_a["outcome"], "merged", "{landed_a}");
    assert_eq!(landed_b["outcome"], "merged", "{landed_b}");
    assert_eq!(landed_b["landed"], json!([10, 20]));
    assert_eq!(landed_a["commit"], candidates[0]["commit"]);
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        candidates[1]["commit"].as_str().unwrap()
    );
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-1"]), ah);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-2"]), bh);
    // The round put the whole gate on record for the new tip: the next turn
    // starts on it without a run of the whole gate.
    let (c, _) = worker(&world, 3, "c");
    let_go(&world, "fast", "c", false);
    let_go(&world, "slow", "c", false);
    let landed_c = finish(start(&world, &c, &world.cache(), 3));
    assert_eq!(landed_c["outcome"], "merged", "{landed_c}");
    assert!(!started(&world, "whole", "b"), "the round's tip needed no certification run");
}

#[test]
fn a_red_root_is_fixed_whole_by_its_last_asked_pr_and_the_others_stop() {
    let world = split_world(3);
    let (e, _) = worker(&world, 5, "e");
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let mut children = one_round(
        &world,
        &e,
        &[(&a, 1), (&b, 2)],
        || until(|| started(&world, "fast", "e")),
        || {
            let_go(&world, "fast", "e", false);
            let_go(&world, "slow", "e", false);
        },
    );
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let_go(&world, "fast", "b", false);
    until(|| started(&world, "slow", "b"));
    let tip = chained(&world)[1]["commit"].as_str().unwrap().to_string();
    let_go(&world, "slow", "b", true);
    let bb = children.pop().unwrap();
    let aa = children.pop().unwrap();
    let (stopped, red) = (finish(aa), finish(bb));
    assert_eq!(red["outcome"], "gate_red", "{red}");
    assert_eq!(red["gate"], "slow");
    assert_eq!(red["round"].as_array().map(Vec::len), Some(2), "{red}");
    assert_eq!(stopped["outcome"], "round_red", "{stopped}");
    assert_eq!(stopped["round_pr"], 20);
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        base,
        "nothing of a red round lands"
    );
    assert!(!started(&world, "slow", "a"), "a red round is not split");
    // The tip's branch holds the whole round; its next turn lands it all.
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), tip);
    let_go(&world, "slow", "b", false);
    let fixed = finish(start(&world, &b, &world.cache(), 2));
    assert_eq!(fixed["outcome"], "merged", "{fixed}");
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    assert!(files.lines().any(|f| f == "a.txt") && files.lines().any(|f| f == "b.txt"));
    let history = world.remote(&["log", "--format=%s", "refs/heads/main"]);
    assert!(
        history.contains("(#10)"),
        "the stopped candidate's merge is in the history: {history}"
    );
}

/// A PR that merges cleanly with no partner leaves the round for the next
/// (docs/adr/0083); there, merged onto the tip that landed the other, it
/// conflicts, and its worker resolves the conflict left in its worktree.
#[test]
fn a_pr_clean_with_no_partner_waits_for_the_next_round_and_resolves_its_conflict_there() {
    let world = split_world(3);
    let (e, _) = worker(&world, 5, "e");
    let a = world.worker(1, "x.txt", "a");
    let b = world.worker(2, "x.txt", "b");
    let _runner = world.host_held();
    let mut children = one_round(
        &world,
        &e,
        &[(&a, 1), (&b, 2)],
        || until(|| started(&world, "fast", "e")),
        || {
            let_go(&world, "fast", "e", false);
            let_go(&world, "slow", "e", false);
        },
    );
    for part in ["fast", "slow"] {
        let_go(&world, part, "a", false);
    }
    let conflict = finish(children.pop().unwrap());
    assert_eq!(finish(children.pop().unwrap())["outcome"], "merged");
    assert_eq!(conflict["outcome"], "conflict", "{conflict}");
    assert_eq!(conflict["files"], json!(["x.txt"]));
    assert!(pairs(&world).is_empty(), "the two pair with nothing");
    std::fs::write(b.join("x.txt"), "ab").unwrap();
    git(&b, &["commit", "-q", "-am", "resolve"]);
    for part in ["fast", "slow"] {
        let_go(&world, part, "b", false);
    }
    assert_eq!(finish(start(&world, &b, &world.cache(), 2))["outcome"], "merged");
    assert_eq!(world.remote(&["show", "refs/heads/main:x.txt"]), "ab");
}

#[test]
fn with_one_lane_every_pair_gates_in_turn_and_the_round_lands_on_one_slow_gate() {
    let world = split_world(1);
    let (e, _) = worker(&world, 5, "e");
    let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c")]
        .into_iter()
        .map(|(n, name)| worker(&world, n, name))
        .collect();
    let _runner = world.host_held();
    let turns: Vec<(&Path, u64)> =
        workers.iter().zip(1..).map(|((dir, _), n)| (dir.as_path(), n)).collect();
    let children = one_round(
        &world,
        &e,
        &turns,
        || until(|| started(&world, "fast", "e")),
        || {
            let_go(&world, "fast", "e", false);
            let_go(&world, "slow", "e", false);
        },
    );
    until(|| started(&world, "fast", "b"));
    assert!(!started(&world, "fast", "c"), "c goes up unpaired");
    let_go(&world, "fast", "b", false);
    until(|| started(&world, "fast", "c"));
    let_go(&world, "fast", "c", false);
    until(|| started(&world, "slow", "c"));
    assert!(!started(&world, "slow", "a") && !started(&world, "slow", "b"));
    let_go(&world, "slow", "c", false);
    let answers: Vec<Value> = children.into_iter().map(finish).collect();
    for answer in &answers {
        assert_eq!(answer["outcome"], "merged", "{answer}");
    }
    assert_eq!(answers[2]["landed"], json!([10, 20, 30]), "one round: {}", answers[2]);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), answers[2]["commit"]);
}

/// A record past `ARG_MAX` is written all the same, and the ended candidates
/// nothing holds any more are not carried into it (sunfmin/control#308).
#[test]
fn a_record_past_arg_max_is_written_and_drops_the_candidates_nothing_holds() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    release(&world, "a", false);
    let landed = world.afk_in(&a, &["land", "--issue", "1"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let mut seed = chain(&world);
    let template = seed["candidates"][0].clone();
    // Its owner's pid: a process this test saw exit.
    let mut exited = Command::new("true").spawn().unwrap();
    let dead = exited.id();
    exited.wait().unwrap();
    let pad = "x".repeat(4096);
    let ended: Vec<Value> = (0..300)
        .map(|n| {
            let mut c = template.clone();
            c["id"] = json!(format!("ended-{n}"));
            c["owner"]["pid"] = json!(dead);
            c["worktree"] = json!(world.root.join(format!("gone-{n}")));
            c["result"]["excerpt"] = json!([pad]);
            c
        })
        .collect();
    seed["candidates"] = json!(ended);
    let text = seed.to_string();
    assert!(text.len() > 1_048_576, "the record is past macOS's ARG_MAX");
    let file = world.root.join("seed.json");
    std::fs::write(&file, &text).unwrap();
    let state = world.remote(&["rev-parse", "refs/convoy/candidates/main"]);
    let tree = world.remote(&["hash-object", "-t", "tree", "/dev/null"]);
    let seeded = world.remote(&["commit-tree", &tree, "-p", &state, "-F", file.to_str().unwrap()]);
    world.remote(&["update-ref", "refs/convoy/candidates/main", &seeded]);

    let (b, _) = worker(&world, 2, "b");
    release(&world, "b", false);
    let landed = world.afk_in(&b, &["land", "--issue", "2"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let kept = chain(&world)["candidates"].as_array().unwrap().clone();
    assert!(!kept.iter().any(|c| c["id"].as_str().unwrap().starts_with("ended-")), "{kept:?}");
    assert!(kept.iter().any(|c| c["issue"] == 2 && c["phase"] == "landed"), "{kept:?}");
    let history = world.remote(&["rev-list", "refs/convoy/candidates/main"]);
    assert!(history.lines().any(|sha| sha == seeded), "the seeded record stays reachable");
}

/// `land --issue n` in `dir` with its clock at `now`.
fn start_at(world: &World, dir: &Path, n: u64, now: u64) -> Child {
    world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), dir)
        .env("XDG_CACHE_HOME", world.cache())
        .env("AFKCONVOY_ISSUE", n.to_string())
        .args(["land", "--issue", &n.to_string(), "--now", &now.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

fn candidate_of(world: &World, issue: u64) -> Value {
    let state = chain(world);
    let rows = state["candidates"].as_array().unwrap();
    rows.iter().find(|c| c["issue"] == issue).cloned().unwrap_or(Value::Null)
}

/// The Offices-MBP case (sunfmin/control#400): a host whose turn holds a
/// candidate goes quiet — its lid closed, here its turn stopped — and its
/// gate with it. While a fleet heartbeat of it is within the lease it is
/// left alone, and a turn here waits for the next round; past the lease,
/// the next turn here takes its candidate over and its round with it, and
/// the turns here land in a round of their own. When the quiet host comes
/// back, its turn finds its candidate taken, stops its own gate, and
/// answers cancelled.
#[test]
fn a_quiet_hosts_candidate_is_taken_over_past_the_lease_and_its_turn_learns_it_on_waking() {
    use afkconvoy_core::lease::LEASE;
    let world = world(2);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let (c, _) = worker(&world, 3, "c");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let _runner = world.host_held();
    // A turn reads its host's name once, as it starts: this one is on "far".
    let here = world.checkout_list()["settings"].clone();
    let mut far_settings = here.clone();
    far_settings["host_name"] = json!("far");
    world.say_host_settings(far_settings);
    let far = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    assert_eq!(candidate_of(&world, 1)["owner"]["host"], "far");
    assert!(chain(&world)["heartbeats"]["far"].is_u64(), "its turn said it was alive");
    world.say_host_settings(here);
    // A turn here would stack on its round, whose whole gate runs: a
    // barrier's place of that far turn holds it pending instead, and goes
    // with what the far host owns.
    hold_pending(&world);
    let asleep = Held::stop(&far);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let beat = (now + 1000).to_string();
    world.afk(&["heartbeat", "--instance", "farfleet", "--host", "far", "--now", &beat]).ok();

    // The chain heard nothing of it for longer than the lease, but its fleet
    // beat within it: it is alive, and a turn here waits for the next round.
    let behind = start_at(&world, &b, 2, now + LEASE + 100);
    until(|| chain(&world)["pending"].as_array().is_some_and(|p| p.len() == 1));
    let first = candidate_of(&world, 1);
    assert_eq!(
        (first["phase"].clone(), first["cancelled_by"].clone()),
        (json!("validating"), Value::Null)
    );

    // Past the lease on both counts: it is dead.
    let next = start_at(&world, &c, 3, now + 1001 + 2 * LEASE);
    began(&world, "c");
    let first = candidate_of(&world, 1);
    assert_eq!(
        (first["phase"].clone(), first["cause"].clone(), first["abandoned"].clone()),
        (json!("cancelled"), json!("owner_died"), json!(true)),
        "{first}"
    );
    assert_eq!(first["stopped"], false, "nobody here confirmed its processes gone");
    assert_eq!(pairs(&world), [(0, 30)], "the turns here pair in a round of their own");
    release(&world, "c", false);
    assert_eq!(finish(behind)["outcome"], "merged");
    assert_eq!(finish(next)["outcome"], "merged");
    assert_eq!(
        chained(&world).iter().find(|c| c["issue"] == 2).unwrap()["predecessor"],
        json!(base)
    );

    // The far host wakes.
    drop(asleep);
    let woke = finish(far);
    assert_eq!(
        (woke["outcome"].clone(), woke["cause"].clone()),
        (json!("cancelled"), json!("owner_died")),
        "{woke}"
    );
    assert!(
        woke["detail"].as_str().unwrap().contains("another host took this candidate over"),
        "{woke}"
    );
    gone(&world, "a");
    assert_eq!(candidate_of(&world, 1)["stopped"], true, "its own host confirmed them gone");
    assert!(!world.root.join("release-a").exists(), "gate a was stopped, never released");
}

/// A candidate's gate red only on TIMEOUTs it cannot classify on an
/// overloaded host says nothing of it (sunfmin/control#617): its turn answers
/// `gate_unsettled`, naming the TIMEOUT and the load, nothing lands and
/// nothing is recorded red. Its next turn on a quieter host lands it.
#[test]
fn a_candidate_red_only_on_timeouts_under_overload_is_unsettled_not_red() {
    let world = world(2);
    let (a, _) = worker(&world, 1, "a");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let overloaded = world.root.join("overloaded-a");
    std::fs::write(&overloaded, "").unwrap();
    release(&world, "a", false);
    let unsettled = finish(aa);
    assert_eq!(unsettled["outcome"], "gate_unsettled", "{unsettled}");
    assert_eq!(unsettled["gate"], "fast");
    let detail = unsettled["detail"].as_str().unwrap();
    assert!(detail.contains("afkconvoy::cli dsh::the_line"), "{detail}");
    assert!(detail.contains("load 23.57, CPUs 12"), "{detail}");
    let candidates = chain(&world)["candidates"].clone();
    assert!(candidates.as_array().unwrap().iter().all(|c| c["phase"] != "red"), "{candidates}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base, "nothing landed");
    assert!(
        !world
            .remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/gate/"])
            .contains("/red-1-"),
        "an unsettled run is not recorded red"
    );
    assert_eq!(world.state(1), "open");
    std::fs::remove_file(&overloaded).unwrap();
    let landed = finish(start(&world, &a, &world.cache(), 1));
    assert_eq!(landed["outcome"], "merged", "{landed}");
}

/// A TIMEOUT the gate classified — it said the host was quiet — is a red
/// gate, as a FAIL is.
#[test]
fn a_candidate_red_on_a_timeout_the_gate_classified_is_gate_red() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    let _runner = world.host_held();
    std::fs::write(world.root.join("timeout-a"), "").unwrap();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    release(&world, "a", false);
    let red = finish(aa);
    assert_eq!(red["outcome"], "gate_red", "{red}");
    assert!(red["unsettled"].is_null(), "{red}");
}

/// A round's whole gate red only on TIMEOUTs it cannot classify on an
/// overloaded host fails no round (sunfmin/control#617): its last-asked PR
/// answers `gate_unsettled`, the candidate before it is not stopped
/// `round_red` but is the round's tip now, and its whole gate lands it.
#[test]
fn an_unsettled_round_fails_no_round_and_the_candidate_before_it_lands() {
    let world = split_world(3);
    let (e, _) = worker(&world, 5, "e");
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let mut children = one_round(
        &world,
        &e,
        &[(&a, 1), (&b, 2)],
        || until(|| started(&world, "fast", "e")),
        || {
            let_go(&world, "fast", "e", false);
            let_go(&world, "slow", "e", false);
        },
    );
    let_go(&world, "fast", "b", false);
    until(|| started(&world, "slow", "b"));
    std::fs::write(world.root.join("overloaded-slow-b"), "").unwrap();
    let_go(&world, "slow", "b", false);
    let unsettled = finish(children.pop().unwrap());
    assert_eq!(unsettled["outcome"], "gate_unsettled", "{unsettled}");
    assert_eq!(unsettled["gate"], "slow");
    let_go(&world, "fast", "a", false);
    until(|| started(&world, "slow", "a"));
    let_go(&world, "slow", "a", false);
    let landed = finish(children.pop().unwrap());
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    assert!(files.lines().any(|f| f == "a.txt") && !files.lines().any(|f| f == "b.txt"));
}

/// The Studio64 case (sunfmin/control#1117): with the one lane taken by a
/// gate, a turn behind it holds a pending place as it waits for a lane. Past
/// its bound it ends `waited_past_bound`, and the chain holds its place no
/// more; the gate ahead lands as before.
#[test]
fn a_pending_turn_past_its_bound_ends_and_frees_its_place() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let mut first = start(&world, &a, &world.cache(), 1);
    until(|| world.root.join("start-a").exists() || first.try_wait().unwrap().is_some());
    if first.try_wait().unwrap().is_some() {
        panic!("first turn exited: {}", finish(first));
    }
    // It would stack on the round whose whole gate runs: a barrier holds it.
    hold_pending(&world);
    let behind = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &b)
        .env("XDG_CACHE_HOME", world.cache())
        .env("AFKCONVOY_ISSUE", "2")
        .env("AFKCONVOY_PENDING_SECONDS", "3")
        .args(["land", "--issue", "2"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let pid = behind.id();
    until(|| {
        chain(&world)["pending"]
            .as_array()
            .is_some_and(|p| p.iter().any(|b| b["owner"]["pid"] == pid && b["since"].is_u64()))
    });
    let answer = finish(behind);
    assert_eq!(answer["outcome"], "waited_past_bound", "{answer}");
    assert!(answer["detail"].as_str().unwrap().contains("for a barrier"), "{answer}");
    let pending = chain(&world)["pending"].clone();
    assert!(pending.as_array().is_none_or(Vec::is_empty), "its place is freed: {pending}");
    assert!(!world.root.join("start-b").exists(), "nothing of it ran");
    release(&world, "a", false);
    assert_eq!(finish(first)["outcome"], "merged");
}

/// A PR ready while a round's whole gate runs stacks on its root
/// (sunfmin/control#1140): its merge onto the round's root-onto-tip commit
/// is gated at once, beside the running gate, which is neither killed nor
/// restarted. The round lands green: the PR's own round starts from that
/// commit as the tip, its root has the stacked merge's tree, and that run on
/// record answers for its gate. The round ends red: the stacked merge is
/// discarded, its turn answers nothing of it, and it lands in a round of
/// its own.
#[test]
fn a_pr_ready_while_a_round_s_whole_gate_runs_stacks_on_its_root() {
    for red in [false, true] {
        let world = world(3);
        let (e, _) = worker(&world, 5, "e");
        let (a, _) = worker(&world, 1, "a");
        let _runner = world.host_held();
        let held = start(&world, &e, &world.cache(), 5);
        began(&world, "e");
        let holder = candidate_of(&world, 5);
        let stacking = start(&world, &a, &world.cache(), 1);
        began(&world, "a");
        let state = chain(&world);
        assert_eq!(state["stacked"]["on"]["commit"], holder["commit"], "{state}");
        let merge = state["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["pair"]["stacked"] == true)
            .cloned()
            .unwrap_or_else(|| panic!("no stacked merge: {state}"));
        assert_eq!(merge["predecessor"], holder["commit"]);
        let gate_e = std::fs::read_to_string(world.root.join("gate-e")).unwrap();
        release(&world, "a", false);
        until(|| {
            let state = chain(&world);
            let rows = state["candidates"].as_array().cloned().unwrap_or_default();
            rows.iter().any(|c| c["id"] == merge["id"] && c["phase"] == "green")
        });
        let running = candidate_of(&world, 5);
        assert_eq!(running["phase"], "validating", "the round's gate runs on: {running}");
        assert_eq!(std::fs::read_to_string(world.root.join("gate-e")).unwrap(), gate_e);
        release(&world, "e", red);
        let first = finish(held);
        let second = finish(stacking);
        assert_eq!(second["outcome"], "merged", "{second}");
        let landed = candidate_of(&world, 1);
        if red {
            assert_eq!(first["outcome"], "gate_red", "{first}");
            let base = holder["predecessor"].clone();
            assert_eq!(landed["predecessor"], base, "it paired anew on the tip: {landed}");
        } else {
            assert_eq!(first["outcome"], "merged", "{first}");
            assert_eq!(landed["predecessor"], holder["commit"], "{landed}");
            assert_eq!(landed["tree"], merge["tree"], "its root has the stacked merge's tree");
            assert!(!landed["result"]["reused"].is_null(), "no second run: {landed}");
        }
        let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
        assert!(files.lines().any(|f| f == "a.txt"), "{files}");
    }
}

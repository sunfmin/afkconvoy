//! Real binary, real remotes and kernel processes. Gates wait on controlled
//! release files; assertions inspect refs, original heads and child exit.
use crate::common::{World, commit, git, worktree_listed};
use serde_json::{Value, json};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

fn quote(path: &Path) -> String {
    afkconvoy_core::line::Line::new().word(&path.to_string_lossy()).own()
}

fn until(done: impl FnMut() -> bool) {
    afkconvoy_patience::until_true("the candidate transition", done);
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
            let group =
                afkconvoy::census::command("ps").args(["-p", pid, "-o", "pgid="]).output().unwrap();
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
                let _ = afkconvoy::census::command("kill")
                    .args(["-KILL", "--", &format!("-{group}")])
                    .status();
            }
            for pid in std::iter::once(gate).chain(read("child", name)) {
                if group_of(&pid) == group {
                    let _ = afkconvoy::census::command("kill").args(["-KILL", &pid]).status();
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
    room_for(&world, lanes);
    Fleet(world)
}

fn start(world: &World, dir: &Path, cache: &Path, n: u64) -> Child {
    let mut command = world.command(env!("CARGO_BIN_EXE_afkconvoy"), dir);
    command
        .env("XDG_CACHE_HOME", cache)
        .env("XDG_DATA_HOME", crate::common::data_beside(cache))
        .env("AFKCONVOY_ISSUE", n.to_string())
        .args(["land", "--issue", &n.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
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
    world.chain("main")
}

fn gone(world: &World, name: &str) {
    for kind in ["gate", "child"] {
        let pid = std::fs::read_to_string(world.root.join(format!("{kind}-{name}"))).unwrap();
        until(|| {
            let ps = afkconvoy::census::command("ps")
                .args(["-p", pid.trim(), "-o", "stat="])
                .output()
                .unwrap();
            let status = String::from_utf8_lossy(&ps.stdout);
            status.trim().is_empty() || status.trim().starts_with('Z')
        });
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
    world.edit_chain("main", |state| {
        let owner = state["round"]["asked"][0]["owner"].clone();
        assert!(owner.is_object(), "no round holds a turn: {state}");
        let barriers = state["barriers"].as_array_mut().unwrap();
        barriers.push(json!({"id": "held-by-the-holder", "owner": owner}));
    });
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

/// Every Orca metadata set made, in order (`orca worktree set`).
fn shown(world: &World) -> Vec<Value> {
    world.orca()["shown"].as_array().cloned().unwrap_or_default()
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
            let ps =
                afkconvoy::census::command("ps").args(["-p", pid, "-o", "stat="]).output().unwrap();
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
    assert!(!world.gate_records().is_empty());
    assert_eq!(world.remote_gate_refs(), [] as [String; 0], "nothing is pushed of it");
}

/// A candidate's gate builds warm, in a clone of its worker's `target/`
/// in its own checkout, and writes nothing into the worker's: what it
/// built there has the candidate's checkout compiled in, and the worker's
/// next gate would run it once that checkout is gone (sunfmin/control#1262).
#[test]
fn a_candidates_gate_builds_in_a_clone_of_its_workers_target_and_leaves_the_workers_alone() {
    let world = World::new();
    commit(&world.clone, ".gitignore", "target/\n");
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    world.set_gate(
        r#"test -n "${AFKCONVOY_CANDIDATE_PROCESS:-}" || exit 0
test -z "${CARGO_TARGET_DIR:-}" || exit 6
test "$(cat target/warm)" = built-by-the-worker || exit 7
pwd > target/built-for
echo built-by-the-candidate > target/warm"#,
    );
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (source, _) = worker(&world, 1, "a");
    std::fs::create_dir_all(source.join("target")).unwrap();
    std::fs::write(source.join("target/warm"), "built-by-the-worker\n").unwrap();
    let landed = world.afk_in(&source, &["land", "--issue", "1"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert_eq!(
        std::fs::read_to_string(source.join("target/warm")).unwrap(),
        "built-by-the-worker\n",
        "the candidate's build changed the worker's"
    );
    assert!(!source.join("target/built-for").exists(), "the candidate built in the worker's");
    let candidate = candidate_of(&world, 1);
    assert!(!Path::new(candidate["worktree"].as_str().unwrap()).exists(), "{candidate}");
}

/// Orca refusing to name a worktree stops no landing: the turn says so once
/// in its log and lands (sunfmin/control#1226).
#[test]
fn a_landing_goes_on_when_orca_refuses_to_name_its_worktrees() {
    let world = World::new();
    world.set_gate("true");
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    world.with_orca(|orca| orca["set_fails"] = json!(true));
    let (source, _) = worker(&world, 1, "a");
    let ran = world.afk_in(&source, &["land", "--issue", "1"]);
    let said = ran.stderr.matches("warning: orca did not name the worktree").count();
    assert_eq!(said, 1, "said once: {}", ran.stderr);
    let landed = ran.ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert!(shown(&world).is_empty());
}

/// A turn that cannot finish still answers an ending its worker is taught,
/// `error`, beside the error itself (sunfmin/control#1267).
#[test]
fn a_turn_that_cannot_finish_answers_the_error_ending() {
    let world = World::new();
    world.set_gate("true");
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (source, _) = worker(&world, 1, "a");
    std::fs::write(source.join("a.txt"), "changed and not committed\n").unwrap();
    let ran = world.afk_in(&source, &["land", "--issue", "1"]);
    assert_eq!(ran.json["outcome"], "error", "{}", ran.json);
    let error = ran.error();
    assert!(error.contains("uncommitted changes"), "{error}");
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
            .env("XDG_DATA_HOME", crate::common::data_beside(checkout.join(".candidate-cache")))
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
    assert_eq!(chain(&world)["candidates"], json!(null), "no candidate was assigned");
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
    world.edit_chain("main", |chain| *chain = state.clone());
    let seeded = chain_tip(&world);
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
        let record = world.chain(target);
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
    assert_eq!(chain(&world)["candidates"], json!(null), "no candidate was assigned");
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

/// Four ready, mutually clean PRs (docs/adr/0083, sunfmin/control#1239):
/// the turns build one group of all four, fast-gated once by the last-asked
/// PR's turn, then gate the root merged onto the tip whole once, and land
/// all four in one fast-forward; each issue closes.
#[test]
fn four_ready_prs_are_one_group_gated_fast_once_and_land_in_one_fast_forward() {
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
    // One group, gated by its last-asked PR's turn.
    until(|| started(&world, "fast", "d"));
    assert_eq!(pairs(&world), [(0, 40)], "one fast gate, not three");
    let_go(&world, "fast", "d", false);
    // Then the root's whole gate, at the last-asked PR's turn.
    until(|| started(&world, "slow", "d"));
    assert_eq!(pairs(&world), [(0, 40)]);
    for name in ["a", "b", "c"] {
        assert!(!started(&world, "fast", name), "no other fast gate ran");
    }
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

/// A group whose fast gate is red stops no other group (docs/adr/0083,
/// sunfmin/control#1239): its last-asked PR carries the others — its branch
/// fast-forwarded to the group's merge and pushed, its turn given the red
/// run — and each other is closed "carried by #…", its branch deleted, its
/// issue told where its work went on; the rest of the round lands. c and d
/// conflict with a and b, so the level is two groups, gated at once.
#[test]
fn a_red_group_has_its_last_pr_carry_the_others_and_the_rest_lands() {
    let world = split_world(3);
    let (e, _) = worker(&world, 5, "e");
    let workers: Vec<_> = [(1, "x", "a"), (2, "y", "b"), (3, "x", "c"), (4, "y", "d")]
        .into_iter()
        .map(|(n, file, name)| {
            let dir = world.worker(n, &format!("{file}.txt"), name);
            let head = git(&dir, &["rev-parse", "HEAD"]);
            (dir, head)
        })
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
    assert_eq!(pairs(&world), [(0, 20), (0, 40)], "two groups, at once");
    let_go(&world, "fast", "b", true);
    let b = children.remove(1);
    let red = finish(b);
    assert_eq!(red["outcome"], "gate_red", "{red}");
    assert_eq!(red["gate"], "fast");
    assert_eq!(red["pair"]["with"], json!([10]), "{red}");
    assert_eq!(red["carrying"], json!([10]), "{red}");
    assert_eq!(red["carried"], json!([10]), "{red}");
    assert!(red["detail"].as_str().unwrap().contains("carried by this PR"), "{red}");
    let pair = red["pair"]["commit"].as_str().unwrap();
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-2"]), pair, "pushed");
    assert_eq!(git(&workers[1].0, &["rev-parse", "HEAD"]), pair, "its branch holds the group");
    let a = children.remove(0);
    let carried = finish(a);
    assert_eq!(carried["outcome"], "carried", "{carried}");
    assert_eq!(carried["carried_by"], 20, "{carried}");
    let gh = world.gh();
    let pr = |n: u64| gh["prs"].as_array().unwrap().iter().find(|p| p["number"] == n).unwrap();
    assert_eq!(pr(10)["state"], "closed");
    assert_eq!(gh["comments"]["10"][0]["body"], "carried by #20");
    let body = pr(20)["body"].as_str().unwrap();
    assert!(body.contains("<!--carries change=10-->\n## Carries #10: pr 10"), "{body}");
    assert!(body.starts_with("Closes #2\n") && body.contains("Closes #1"), "{body}");
    assert_eq!(world.remote(&["for-each-ref", "refs/heads/issue-1"]), "", "its branch deleted");
    let told = gh["comments"]["1"].as_array().unwrap();
    assert!(
        told.iter()
            .any(|c| c["body"].as_str().unwrap().starts_with(
                "<!--convoy:carried pr=10 by=20-->\n<!--convoy:branch name=issue-2-->"
            )),
        "{told:?}"
    );
    let_go(&world, "fast", "d", false);
    until(|| started(&world, "slow", "d"));
    let_go(&world, "slow", "d", false);
    for answer in children.into_iter().map(finish) {
        assert_eq!(answer["outcome"], "merged", "{answer}");
    }
    for (file, name) in [("x", "c"), ("y", "d")] {
        assert_eq!(world.remote(&["show", &format!("refs/heads/main:{file}.txt")]), name);
    }
    assert_eq!(world.state(1), "open", "a carried issue stays open until its carrier lands");
    assert_eq!(world.state(2), "open");
    // Its worker rewrites its description, dropping what it carries, and
    // #10's says no issue any more; fixed, it lands, and the carried issue
    // closes with it all the same, by the record the fleet left on it
    // (sunfmin/control#1248).
    world.describe_pr(20, "Closes #2\n");
    world.describe_pr(10, "Nothing.\n");
    let fixed_b = &workers[1].0;
    git(fixed_b, &["fetch", "-q", "origin", "main"]);
    git(fixed_b, &["merge", "-q", "--no-edit", "-X", "ours", "FETCH_HEAD"]);
    git(fixed_b, &["push", "-q", "origin", "HEAD"]);
    for part in ["fast", "slow"] {
        let_go(&world, part, "b", false);
    }
    let landed = finish(start(&world, fixed_b, &world.cache(), 2));
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let body =
        world.gh()["prs"].as_array().unwrap().iter().find(|p| p["number"] == 20).unwrap()["body"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(body.contains("<!--carries change=10-->\n## Carries #10: pr 10"), "{body}");
    assert!(!body.contains("Closes #1"), "{body}");
    assert_eq!(world.state(2), "closed");
    assert_eq!(world.state(1), "closed", "the carried issue closes as its carrier lands");
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
fn a_green_run_the_host_cannot_record_is_no_landing_and_frees_the_lane() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    world.refuse_gate_records("the disk is full");

    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    release(&world, "a", false);
    let failed = finish(aa);
    assert_eq!(failed["outcome"], "gate_red", "{failed}");
    assert_eq!(failed["status"], "green", "the tests passed");
    assert_eq!(failed["exit_code"], 0);
    assert_eq!(failed["timed_out"], false);
    assert_eq!(failed["recorded"], false);
    let not_recorded = failed["not_recorded"].as_str().unwrap();
    assert!(not_recorded.contains("the disk is full"), "{failed}");
    assert_eq!(world.state(1), "open");

    world.allow_gate_records();
    let bb = start(&world, &b, &world.cache(), 2);
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

/// A turn that dies holding a gating candidate whose process scope is gone
/// — as between a round's fast and slow gate — leaves no lane held for good:
/// the next `land` stops its gate and what it started, frees the lane, and
/// lands (sunfmin/control#1247). It once answered only
/// `{"error":"gate process ownership is missing; its lane remains occupied"}`.
#[test]
fn a_dead_turns_gate_without_its_scope_is_stopped_and_the_next_land_lands() {
    let world = world(2);
    let (a, head) = worker(&world, 1, "a");
    let _runner = world.host_held();
    let mut first = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let old = gate_pids(&world, "a");
    first.kill().unwrap();
    let _ = first.wait();
    let worktree = candidate_of(&world, 1)["worktree"].as_str().unwrap().to_string();
    std::fs::remove_file(format!("{worktree}.processes")).unwrap();
    let next = start(&world, &a, &world.cache(), 1);
    ended_all(&old);
    until(|| chain(&world)["candidates"][0]["stopped"] == true);
    release(&world, "a", false);
    let landed = finish(next);
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert_eq!(chain(&world)["candidates"][0]["cause"], "owner_died");
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
}

/// The PRs that asked to land are taken into a round as it starts, its root
/// one chain of candidates; a PR asking during the round waits for the next
/// (docs/adr/0083). Its three clean PRs are one group (sunfmin/control#1239).
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
    began(&world, "c");
    assert_eq!(pairs(&world), [(0, 30)], "one group of the three");
    let mut fourth = start(&world, &workers[3].0, &world.cache(), 4);
    until(|| chain(&world)["pending"].as_array().is_some_and(|p| p.len() == 1));
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
    assert_eq!(restored["running"].as_array().unwrap().len(), 1, "{restored}");
    assert!(restored["room"]["reading"].is_object(), "{restored}");
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
    let ps =
        afkconvoy::census::command("ps").args(["-p", pid.trim(), "-o", "stat="]).output().unwrap();
    let status = String::from_utf8_lossy(&ps.stdout);
    assert!(status.trim().is_empty() || status.trim().starts_with('Z'), "{status}");
}

#[test]
fn restart_after_durable_cancellation_finishes_cleanup_before_rebuilding() {
    let world = world(2);
    let (a, _) = worker(&world, 1, "a");
    let runner = world.host_held();
    let mut aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let old = gate_pids(&world, "a");
    // Its head moves, and its own turn cancels its candidate, durably, and
    // dies before it stops the gate: the host's chain holds the cancellation
    // and its gate still runs.
    move_head(&a);
    aa.kill().unwrap();
    let _ = aa.wait();
    world.edit_chain("main", |state| {
        let c = &mut state["candidates"][0];
        c["cancelled_by"] = c["id"].clone();
        c["cause"] = json!("head_changed");
        c["phase"] = json!("cancelling");
    });
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
    let world = world(2);
    let (a, _) = worker(&world, 1, "a");
    let _runner = world.host_held();
    let mut aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    world.refuse_chain_writes("\"descendants\":[{", "the frozen tree is never recorded");
    move_head(&a);
    let began = Instant::now();
    while aa.try_wait().unwrap().is_none() {
        if began.elapsed() > afkconvoy_patience::PATIENCE {
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

/// The host's chain of `main` as it stands, to see whether a step wrote it.
fn chain_tip(world: &World) -> String {
    chain(world).to_string()
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
    let hold =
        Hold(afkconvoy::census::command("sleep").arg("300").current_dir(&a_tree).spawn().unwrap());
    release(&world, "a", false);
    let landed_a = finish(aa);
    assert_eq!(landed_a["outcome"], "merged", "{landed_a}");
    assert!(a_tree.is_dir(), "a worktree a process is working in stays");
    assert!(listed(&world, &a_tree));
    assert!(listed(&world, &issue_a), "the issue worktree stays");
    let candidate = chained(&world).into_iter().find(|c| c["issue"] == 1).unwrap();
    assert_eq!(candidate["phase"], "landed");
    assert_eq!(candidate["commit"], landed_a["commit"]);
    let gates = world.gate_records();
    assert!(!gates.is_empty(), "the recorded gate run stays on the host's record");
    let tip = chain_tip(&world);
    drop(hold);
    world.watch_git();
    tick(&world);
    assert!(!a_tree.exists(), "a later tick removes the worktree once the process is gone");
    assert!(!listed(&world, &a_tree));
    assert_removal_follows_the_look(&world);
    assert_eq!(chain_tip(&world), tip, "the candidate chain is not rewritten");
    assert_eq!(world.gate_records(), gates);
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
    assert!(
        !calls.iter().any(|call| call.contains("refs/convoy/candidates/")),
        "no turn reads or writes a candidate ref of the remote: {calls:#?}"
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
    room_for(&world, lanes);
    world
}

/// The host's room, as `lanes` places once said it (docs/adr/0088): one, and
/// the cores read past the share, so that a command starts only while
/// nothing of the queue runs; more, and a tenth busy, so that each start
/// counts a quarter of the host for the minute the reading takes to include
/// it, and four start side by side.
fn room_for(world: &World, lanes: u64) {
    world.cores_busy(if lanes == 1 { 95 } else { 10 });
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
    // Orca names each worktree by its issues, never a PR's, and says its
    // gate's state as it changes (sunfmin/control#1226).
    let tip = candidates[1]["worktree"].as_str().unwrap();
    let shown = shown(&world);
    fn said(row: &Value) -> (&str, Option<&str>) {
        (row["displayName"].as_str().unwrap(), row["comment"].as_str())
    }
    let pair: Vec<_> =
        shown.iter().filter(|row| row["path"] != tip && row["path"] != "").map(said).collect();
    assert!(pair.contains(&("pair #1 + #2", Some("gating"))), "{shown:#?}");
    assert!(pair.contains(&("pair #1 + #2", Some("green"))), "{shown:#?}");
    let round: Vec<_> = shown.iter().filter(|row| row["path"] == tip).map(said).collect();
    assert_eq!(
        round,
        [
            ("round of #1, #2 · fast gate", Some("gating")),
            ("round of #1, #2 · slow gate", Some("gating")),
            ("round of #1, #2 · slow gate", Some("green")),
        ],
        "{shown:#?}"
    );
    assert!(shown.iter().all(|row| row["issue"].is_null()), "{shown:#?}");
    assert!(
        shown.iter().all(|row| !row["displayName"].as_str().unwrap().contains("#10")),
        "no PR number: {shown:#?}"
    );
    // The round put the whole gate on record for the new tip: the next turn
    // starts on it without a run of the whole gate.
    let (c, _) = worker(&world, 3, "c");
    let_go(&world, "fast", "c", false);
    let_go(&world, "slow", "c", false);
    let landed_c = finish(start(&world, &c, &world.cache(), 3));
    assert_eq!(landed_c["outcome"], "merged", "{landed_c}");
    assert!(!started(&world, "whole", "b"), "the round's tip needed no certification run");
}

/// A PR closed as carried while its turn gates a pair of its round
/// (sunfmin/control#1349): a later PR whose branch holds it asks to land,
/// and that turn closes it as carried. The carried PR's turn ends at once,
/// answering `carried`, its pair's gate stopped: no process of it is left in
/// its worktree for doctor to call settling waiting. The round goes on
/// without it, and the carrying PR lands its change with its own.
#[test]
fn a_pr_carried_while_its_turn_gates_a_pair_ends_at_once_answering_carried() {
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
    until(|| started(&world, "fast", "b"));
    // c's branch holds b's: its turn closes #20 as carried as it asks.
    let (c, _) = worker(&world, 3, "c");
    git(&c, &["fetch", "-q", "origin", "issue-2"]);
    git(&c, &["merge", "-q", "--no-edit", "FETCH_HEAD"]);
    git(&c, &["push", "-q", "origin", "HEAD"]);
    let cc = start(&world, &c, &world.cache(), 3);
    let carried = finish(children.pop().unwrap());
    assert_eq!(carried["outcome"], "carried", "{carried}");
    assert_eq!(carried["carried_by"], 30, "{carried}");
    let state = chain(&world);
    let pair = state["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| !c["pair"].is_null() && c["pr"] == 20)
        .cloned()
        .unwrap();
    assert_eq!(pair["cause"], "carried", "{state:#}");
    let pr = |n: u64| {
        world.gh()["prs"].as_array().unwrap().iter().find(|p| p["number"] == n).unwrap().clone()
    };
    assert_eq!(pr(20)["state"], "closed");
    for (part, name) in [("fast", "a"), ("slow", "a"), ("fast", "c"), ("slow", "c")] {
        let_go(&world, part, name, false);
    }
    let landed_a = finish(children.pop().unwrap());
    assert_eq!(landed_a["outcome"], "merged", "{landed_a}");
    let landed_c = finish(cc);
    assert_eq!(landed_c["outcome"], "merged", "{landed_c}");
    assert_eq!(world.remote(&["show", "refs/heads/main:b.txt"]), "b", "carried, it landed");
    assert_eq!(world.state(2), "closed", "the carried issue closes as its carrier lands");
}

/// A red root is not bisected (docs/adr/0083): the round's last-asked PR
/// fixes it on the spot. Its branch is brought up to the root's tip and
/// pushed, every other PR of the round is closed at once as carried by it,
/// and its fixed turn lands the whole round; every issue of it closes.
#[test]
fn a_red_root_is_fixed_on_the_spot_by_its_last_asked_pr_which_carries_the_round() {
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
    assert_eq!(red["carrying"], json!([10]), "{red}");
    assert_eq!(red["carried"], json!([10]), "{red}");
    assert_eq!(stopped["outcome"], "round_red", "{stopped}");
    assert_eq!(stopped["round_pr"], 20);
    assert_eq!(stopped["carried_by"], 20);
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        base,
        "nothing of a red round lands"
    );
    assert!(!started(&world, "slow", "a"), "a red round is not split");
    // The tip's branch holds the whole round, pushed; the other PR is carried.
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), tip);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/issue-2"]), tip);
    let pr = |n: u64| {
        let gh = world.gh();
        gh["prs"].as_array().unwrap().iter().find(|p| p["number"] == n).unwrap().clone()
    };
    assert_eq!(pr(10)["state"], "closed", "carried at once");
    assert!(pr(20)["body"].as_str().unwrap().contains("<!--carries change=10-->"));
    assert_eq!(world.state(1), "open", "the carried issue stays open until its carrier lands");
    // Its worker rewrites its description, dropping what it carries
    // (sunfmin/control#1248): landing writes it back before it lands.
    world.describe_pr(20, "Closes #2\n");
    let_go(&world, "slow", "b", false);
    let fixed = finish(start(&world, &b, &world.cache(), 2));
    let body = pr(20)["body"].as_str().unwrap().to_string();
    assert!(body.contains("<!--carries change=10-->") && body.contains("Closes #1"), "{body}");
    assert_eq!(fixed["outcome"], "merged", "{fixed}");
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    assert!(files.lines().any(|f| f == "a.txt") && files.lines().any(|f| f == "b.txt"));
    let history = world.remote(&["log", "--format=%s", "refs/heads/main"]);
    assert!(
        history.contains("(#10)"),
        "the stopped candidate's merge is in the history: {history}"
    );
    assert_eq!(world.state(2), "closed");
    assert_eq!(world.state(1), "closed", "every issue of the round closes as it lands");
}

/// Two PRs that conflict, neither with another partner (docs/adr/0083): the
/// later-asked carries the earlier, its merge left in its worktree to
/// resolve, and the earlier's turn answers `carried`. Resolved and asked
/// again, its turn closes the carried PR, and lands both; both issues close.
#[test]
fn a_pr_conflicting_with_every_partner_carries_the_earliest_and_lands_both() {
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
    let conflict = finish(children.pop().unwrap());
    let carried = finish(children.pop().unwrap());
    assert_eq!(conflict["outcome"], "conflict", "{conflict}");
    assert_eq!(conflict["files"], json!(["x.txt"]));
    assert_eq!(conflict["carrying"], json!([10]), "{conflict}");
    assert_eq!(carried["outcome"], "carried", "{carried}");
    assert_eq!(carried["carried_by"], 20);
    assert!(pairs(&world).is_empty(), "the two pair with nothing");
    let open = |n: u64| {
        let gh = world.gh();
        gh["prs"].as_array().unwrap().iter().find(|p| p["number"] == n).unwrap()["state"].clone()
    };
    assert_eq!(open(10), "open", "nothing is closed before the carrier holds it");
    std::fs::write(b.join("x.txt"), "ab").unwrap();
    git(&b, &["commit", "-q", "-am", "resolve"]);
    git(&b, &["push", "-q", "origin", "HEAD"]);
    for part in ["fast", "slow"] {
        let_go(&world, part, "b", false);
    }
    // Its worker rewrote its description before the turn that carries #10
    // (sunfmin/control#1248): that turn writes what it carries back in.
    world.describe_pr(20, "Resolved.\n\nCloses #2\n");
    let landed = finish(start(&world, &b, &world.cache(), 2));
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert_eq!(landed["carried"], json!([10]), "{landed}");
    let gh = world.gh();
    let body = gh["prs"].as_array().unwrap().iter().find(|p| p["number"] == 20).unwrap()["body"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(body.contains("<!--carries change=10-->") && body.contains("Closes #1"), "{body}");
    assert_eq!(open(10), "closed");
    assert_eq!(world.remote(&["show", "refs/heads/main:x.txt"]), "ab");
    assert_eq!(world.state(2), "closed");
    assert_eq!(world.state(1), "closed", "the carried issue closes as its carrier lands");
}

/// With one lane, three clean PRs are one group, fast-gated once by the
/// last-asked PR's turn (sunfmin/control#1239), and the round lands on one
/// slow gate.
#[test]
fn with_one_lane_a_round_s_one_group_gates_and_the_round_lands_on_one_slow_gate() {
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
    until(|| started(&world, "fast", "c"));
    assert_eq!(pairs(&world), [(0, 30)], "one group of the three");
    let_go(&world, "fast", "c", false);
    until(|| started(&world, "slow", "c"));
    assert!(!started(&world, "slow", "a") && !started(&world, "slow", "b"));
    assert!(!started(&world, "fast", "a") && !started(&world, "fast", "b"));
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
    let mut exited = afkconvoy::census::command("true").spawn().unwrap();
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
    world.edit_chain("main", |chain| *chain = seed.clone());

    let (b, _) = worker(&world, 2, "b");
    release(&world, "b", false);
    let landed = world.afk_in(&b, &["land", "--issue", "2"]).ok();
    assert_eq!(landed["outcome"], "merged", "{landed}");
    let kept = chain(&world)["candidates"].as_array().unwrap().clone();
    assert!(!kept.iter().any(|c| c["id"].as_str().unwrap().starts_with("ended-")), "{kept:?}");
    assert!(kept.iter().any(|c| c["issue"] == 2 && c["phase"] == "landed"), "{kept:?}");
}

fn candidate_of(world: &World, issue: u64) -> Value {
    let state = chain(world);
    let rows = state["candidates"].as_array().unwrap();
    rows.iter().find(|c| c["issue"] == issue).cloned().unwrap_or(Value::Null)
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
    assert!(world.red_gate_runs().is_empty(), "an unsettled run is not recorded red");
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
        .env("XDG_DATA_HOME", world.data())
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

/// Ten turns of one repo wait on one round, and the host starts no git
/// process for the chain: nothing of it is read from, or written to, the
/// remote (docs/adr/0117). What the host does start is printed, as the number
/// the ADR measures.
#[test]
fn ten_turns_waiting_on_one_round_start_no_git_process_for_the_chain() {
    let world = world(2);
    let (e, _) = worker(&world, 5, "e");
    let names: Vec<String> = (0..10).map(|n| format!("w{n}")).collect();
    let workers: Vec<(PathBuf, String)> =
        names.iter().zip(6..).map(|(name, n)| worker(&world, n, name)).collect();
    let _runner = world.host_held();
    let holder = start(&world, &e, &world.cache(), 5);
    began(&world, "e");
    hold_pending(&world);
    let mut children = vec![];
    for (asked, ((dir, _), n)) in workers.iter().zip(6..).enumerate() {
        children.push(start(&world, dir, &world.cache(), n));
        until(|| chain(&world)["pending"].as_array().is_some_and(|p| p.len() == asked + 1));
    }
    clear_git_log(&world);
    let window = Duration::from_secs(5);
    std::thread::sleep(window);
    let calls = world.git_calls();
    eprintln!(
        "git processes in {window:?} with ten turns waiting: {} ({:.1} a second)",
        calls.len(),
        calls.len() as f64 / window.as_secs_f64()
    );
    assert!(
        !calls.iter().any(|call| call.contains("refs/convoy/candidates")),
        "a turn read or wrote a candidate ref of the remote: {calls:#?}"
    );
    assert!(
        !calls.iter().any(|call| call.starts_with("ls-remote")),
        "a waiting turn asked the remote: {calls:#?}"
    );
    release(&world, "e", false);
    assert_eq!(finish(holder)["outcome"], "merged");
    for child in children {
        assert_eq!(finish(child)["outcome"], "merged");
    }
}

/// Two hosts, each with its own database and no knowledge of the other, land
/// a fast-green round on the same base: one lands, the other's push is
/// refused — or, its gate records being its own, it finds the new tip
/// uncertified — and its next turn certifies that tip, builds its round again
/// on it and gates it again. Neither lands an untested tree (docs/adr/0117,
/// docs/adr/0037).
#[test]
fn two_hosts_land_on_one_base_and_the_one_refused_gates_again_on_the_new_tip() {
    let world = world(3);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let other = world.root.join("cache-of-another-host");
    let _runner = world.host_held();
    let on_a = start(&world, &a, &world.cache(), 1);
    let on_b = start(&world, &b, &other, 2);
    began(&world, "a");
    began(&world, "b");
    release(&world, "a", false);
    let landed = finish(on_a);
    assert_eq!(landed["outcome"], "merged", "{landed}");
    release(&world, "b", false);
    let refused = finish(on_b);
    assert!(
        matches!(
            refused["outcome"].as_str(),
            Some("target_moved" | "cancelled" | "base_uncertified")
        ),
        "its push is refused: {refused}"
    );
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        landed["commit"].as_str().unwrap(),
        "nothing of b landed"
    );
    // Its host's chain holds nothing of a, and the other's nothing of b.
    let again = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &b)
        .env("XDG_CACHE_HOME", &other)
        .env("XDG_DATA_HOME", crate::common::data_beside(&other))
        .env("AFKCONVOY_ISSUE", "2")
        .args(["land", "--issue", "2"])
        .output()
        .unwrap();
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again["outcome"], "merged", "{again}");
    assert_eq!(again["merged"], landed["commit"], "built again on the new tip");
    let tip = world.remote(&["rev-parse", "refs/heads/main"]);
    assert_eq!(tip, again["commit"].as_str().unwrap());
    assert_eq!(
        world.remote(&["rev-parse", &format!("{tip}^1")]),
        landed["commit"].as_str().unwrap()
    );
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    assert!(files.lines().any(|f| f == "a.txt") && files.lines().any(|f| f == "b.txt"), "{files}");
    let tree = world.remote(&["rev-parse", &format!("{tip}^{{tree}}")]);
    assert!(world.gate_recorded_on(&other, &tree), "the tree it landed was gated on its host");
    assert!(!world.gate_recorded(&tree), "and is on no other host's record");
    assert!(
        world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/candidates"]).is_empty()
    );
}

/// A host runner killed in the middle of a round and started again finds the
/// round in the host's database, and every turn of it finishes.
#[test]
fn a_host_runner_restarted_mid_round_finds_its_round_in_the_database() {
    let world = world(3);
    let (e, _) = worker(&world, 5, "e");
    let workers: Vec<_> =
        [(1, "a"), (2, "b")].into_iter().map(|(n, name)| worker(&world, n, name)).collect();
    let runner = world.host_held();
    let turns: Vec<(&Path, u64)> =
        workers.iter().zip(1..).map(|((dir, _), n)| (dir.as_path(), n)).collect();
    let children =
        one_round(&world, &e, &turns, || began(&world, "e"), || release(&world, "e", false));
    began(&world, "b");
    let before = chain(&world)["round"]["id"].clone();
    world.kill_host_runner("KILL");
    drop(runner);
    let _replacement = world.host_held();
    assert_eq!(chain(&world)["round"]["id"], before, "the round is in the database still");
    release(&world, "a", false);
    release(&world, "b", false);
    for child in children {
        assert_eq!(finish(child)["outcome"], "merged");
    }
}

/// A remote chain record of an older afkconvoy is ignored, `doctor` says it is
/// left, and the tick removes it once no turn of this host holds a place on it.
#[test]
fn an_old_remote_chain_record_is_ignored_reported_and_removed() {
    let world = world(1);
    let tree = world.remote(&["hash-object", "-w", "-t", "tree", "/dev/null"]);
    let record = world.remote(&["commit-tree", &tree, "-m", "{\"candidates\":[],\"barriers\":[]}"]);
    world.remote(&["update-ref", "refs/convoy/candidates/main", &record]);
    let _runner = world.host_held();
    let doctor = world.afk(&["doctor"]).json["legacy_candidate_chains"].clone();
    assert_eq!(doctor, json!([{"ref": "refs/convoy/candidates/main", "in_use": false}]));
    let (a, _) = worker(&world, 1, "a");
    release(&world, "a", false);
    let landed = world.afk_in(&a, &["land", "--issue", "1"]).ok();
    assert_eq!(landed["outcome"], "merged", "ignored: {landed}");
    assert_eq!(world.remote(&["rev-parse", "refs/convoy/candidates/main"]), record, "untouched");
    tick(&world);
    assert!(
        world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/candidates"]).is_empty(),
        "removed"
    );
    assert_eq!(world.afk(&["doctor"]).json["legacy_candidate_chains"], json!([]));
}

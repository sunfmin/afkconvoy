//! Real binary, real remotes and kernel processes. Gates wait on controlled
//! release files; assertions inspect refs, original heads and child exit.
use crate::common::{World, commit, git, worktree_listed};
use serde_json::{Value, json};
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

fn world(lanes: u64) -> World {
    let world = World::new();
    let root = quote(&world.root);
    let script = format!(
        r#"#!/bin/sh
set -e
root={root}
name=
for n in a b c d; do if test -f "$n.txt"; then name=$n; fi; done
case "${{AFKCONVOY_ISSUE:-}}" in 1) name=a;; 2) name=b;; 3) name=c;; 4) name=d;; esac
test -n "$name" || exit 0
echo $$ > "$root/gate-$name"
sleep 120 &
echo $! > "$root/child-$name"
touch "$root/start-$name"
while test -d "$root" && test ! -f "$root/release-$name"; do sleep 0.02; done
test ! -f "$root/fail-$name"
"#
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
    world
}

fn start(world: &World, dir: &Path, cache: &Path, n: u64) -> Child {
    world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), dir)
        .env("XDG_CACHE_HOME", cache)
        .env("AFKCONVOY_ISSUE", n.to_string())
        .args(["land", "--issue", &n.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
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

fn green(world: &World, issue: u64) {
    until(|| {
        chain(world)["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["issue"] == issue && c["phase"] == "green")
    });
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

fn worker(world: &World, n: u64, name: &str) -> (PathBuf, String) {
    let dir = world.worker(n, &format!("{name}.txt"), name);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    (dir, head)
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

#[test]
fn three_candidates_overlap_finish_out_of_order_and_publish_exact_commits_once() {
    let world = world(3);
    let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c")]
        .into_iter()
        .map(|(n, name)| worker(&world, n, name))
        .collect();
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let _runner = world.host_held();
    let mut a = start(&world, &workers[0].0, &world.cache(), 1);
    until(|| world.root.join("start-a").exists() || a.try_wait().unwrap().is_some());
    if a.try_wait().unwrap().is_some() {
        panic!("first turn exited: {}", finish(a));
    }
    let b = start(&world, &workers[1].0, &world.cache(), 2);
    began(&world, "b");
    let c = start(&world, &workers[2].0, &world.cache(), 3);
    began(&world, "c");
    release(&world, "c", false);
    green(&world, 3);
    release(&world, "b", false);
    green(&world, 2);
    until(|| {
        world.afk(&["queue"]).ok()["running"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|r| r["issue"] == 2 && r["validation"]["waiting"] == "waiting_for_predecessor")
        })
    });
    let queue = world.afk(&["queue"]).ok();
    let waiting = queue["running"].as_array().unwrap().iter().find(|r| r["issue"] == 2).unwrap();
    assert_eq!(waiting["validation"]["predecessor"], chain(&world)["candidates"][0]["commit"]);
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
    let before = chain(&world);
    release(&world, "a", false);
    let answers = [finish(a), finish(b), finish(c)];
    for (i, answer) in answers.iter().enumerate() {
        assert_eq!(answer["outcome"], "merged", "{answer}");
        assert_eq!(answer["commit"], before["candidates"][i]["commit"]);
        assert_eq!(git(&workers[i].0, &["rev-parse", "HEAD"]), workers[i].1);
        assert_eq!(
            world.remote(&["rev-parse", &format!("refs/heads/issue-{}", i + 1)]),
            workers[i].1
        );
        if i > 0 {
            assert_eq!(answer["merged"], answers[i - 1]["commit"]);
        }
    }
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        answers[2]["commit"].as_str().unwrap()
    );
    assert_eq!(
        chain(&world)["candidates"].as_array().unwrap().len(),
        3,
        "no duplicate aggregate gate"
    );
    let landed = world.afk(&["doctor"]).json["candidates"][2].clone();
    assert_eq!(landed["waiting"], "landed");
    assert_eq!(landed["phase"], "landed");
    assert!(landed.get("excerpt").is_none(), "{landed}");
    assert!(landed.get("result").is_none(), "{landed}");
    for key in ["phase", "commit", "issue", "pr", "predecessor", "waiting", "target_branch"] {
        assert!(landed.get(key).is_some(), "{key} missing from {landed}");
    }
}

#[test]
fn first_failure_invalidates_running_suffix_kills_children_and_rebuilds_from_original_heads() {
    let world = world(3);
    let (a, _) = worker(&world, 1, "a");
    let (b, bh) = worker(&world, 2, "b");
    let (c, ch) = worker(&world, 3, "c");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
    let cc = start(&world, &c, &world.cache(), 3);
    began(&world, "c");
    release(&world, "a", true);
    let answers = [finish(aa), finish(bb), finish(cc)];
    eprintln!(
        "CANDIDATE_DISCARDED_WORK={}",
        json!({"cancelled_gate_seconds": answers[1..].iter().map(|a| a["seconds"].as_u64().unwrap_or(0)).sum::<u64>(), "gates": &answers[1..]})
    );
    assert_eq!(answers[0]["outcome"], "gate_red", "{:?}", answers);
    for answer in &answers[1..] {
        assert_eq!(answer["outcome"], "cancelled", "{answer}");
        assert_eq!(answer["cause"], "predecessor_failed");
        assert_eq!(answer["cancelled_by"], answers[0]["candidate"]);
    }
    for name in ["a", "b", "c"] {
        gone(&world, name);
    }
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), bh);
    assert_eq!(git(&c, &["rev-parse", "HEAD"]), ch);
    release(&world, "b", false);
    let rebuilt = finish(start(&world, &b, &world.cache(), 2));
    assert_eq!(rebuilt["outcome"], "merged", "{rebuilt}");
    assert_eq!(rebuilt["merged"], base);
    assert!(
        !world.remote(&["ls-tree", "--name-only", "refs/heads/main"]).lines().any(|f| f == "a.txt")
    );
    assert_eq!(world.remote(&["show", "refs/heads/main:b.txt"]), "b");
}

#[test]
fn middle_failure_preserves_landed_prefix_and_a_green_descendant_is_ineligible() {
    let world = world(3);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let (c, _) = worker(&world, 3, "c");
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
    let cc = start(&world, &c, &world.cache(), 3);
    began(&world, "c");
    release(&world, "c", false);
    green(&world, 3);
    release(&world, "a", false);
    let landed = finish(aa);
    assert_eq!(landed["outcome"], "merged");
    release(&world, "b", true);
    let failed = finish(bb);
    let cancelled = finish(cc);
    assert_eq!(failed["outcome"], "gate_red", "{failed}");
    assert_eq!(cancelled["outcome"], "cancelled", "{cancelled}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), landed["commit"].as_str().unwrap());
    release(&world, "c", false);
    let rebuilt = finish(start(&world, &c, &world.cache(), 3));
    assert_eq!(rebuilt["outcome"], "merged", "{rebuilt}");
    let files = world.remote(&["ls-tree", "--name-only", "refs/heads/main"]);
    assert!(files.lines().any(|f| f == "a.txt"));
    assert!(files.lines().any(|f| f == "c.txt"));
    assert!(!files.lines().any(|f| f == "b.txt"));
}

#[test]
fn a_gate_timeout_invalidates_and_terminates_its_running_successor() {
    let world = world(2);
    let config = std::fs::read_to_string(world.clone.join("docs/agents/afkconvoy.md"))
        .unwrap()
        .replace("gate_seconds: 600", "gate_seconds: 4");
    commit(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["push", "-q", "origin", "HEAD"]);
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
    let failed = finish(aa);
    let cancelled = finish(bb);
    assert_eq!(failed["outcome"], "gate_red", "{failed}");
    assert_eq!(failed["timed_out"], true);
    assert_eq!(cancelled["outcome"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["cause"], "predecessor_failed");
    gone(&world, "a");
    gone(&world, "b");
}

#[test]
fn a_changed_head_cancels_its_suffix_and_an_external_base_move_invalidates_the_chain() {
    for head_change in [true, false] {
        let world = world(2);
        let (a, _) = worker(&world, 1, "a");
        let (b, bh) = worker(&world, 2, "b");
        let _runner = world.host_held();
        let aa = start(&world, &a, &world.cache(), 1);
        began(&world, "a");
        let bb = start(&world, &b, &world.cache(), 2);
        began(&world, "b");
        if head_change {
            commit(&a, "changed.txt", "new head");
            git(&a, &["push", "-q", "origin", "HEAD"]);
        } else {
            commit(&world.clone, "outside.txt", "outside");
            git(&world.clone, &["push", "-q", "origin", "HEAD"]);
        }
        let answers = [finish(aa), finish(bb)];
        assert!(
            answers
                .iter()
                .all(|a| a["outcome"] == if head_change { "cancelled" } else { "target_moved" }),
            "{:?}",
            answers
        );
        for name in ["a", "b"] {
            gone(&world, name);
        }
        assert_eq!(git(&b, &["rev-parse", "HEAD"]), bh);
        assert_eq!(world.state(2), "open");
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

#[test]
fn concurrent_assignment_is_a_single_chain_and_the_window_stays_bounded() {
    let world = world(3);
    let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c"), (4, "d")]
        .into_iter()
        .map(|(n, name)| worker(&world, n, name))
        .collect();
    let _runner = world.host_held();
    let mut children: Vec<_> =
        (0..3).map(|i| start(&world, &workers[i].0, &world.cache(), i as u64 + 1)).collect();
    for name in ["a", "b", "c"] {
        began(&world, name);
    }
    let mut fourth = start(&world, &workers[3].0, &world.cache(), 4);
    std::thread::sleep(Duration::from_millis(200));
    let state = chain(&world);
    let rows = state["candidates"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for i in 1..3 {
        assert_eq!(rows[i]["predecessor"], rows[i - 1]["commit"]);
    }
    assert!(fourth.try_wait().unwrap().is_none());
    assert!(!world.root.join("start-d").exists());
    for name in ["a", "b", "c", "d"] {
        release(&world, name, false);
    }
    children.push(fourth);
    for child in children {
        let result = finish(child);
        assert_eq!(result["outcome"], "merged", "{result}");
    }
    assert_eq!(chain(&world)["candidates"].as_array().unwrap().len(), 4);
}

#[test]
fn semantic_blockers_hold_a_cancelled_successor_and_a_live_lock_blocks_a_waiting_gate() {
    let world = world(1);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &b, &world.cache(), 2);
    until(|| {
        world.afk(&["doctor"]).json["host"]["exclusive"]["waiting"]
            .as_array()
            .is_some_and(|w| !w.is_empty())
    });
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
    let failed = start(&world, &a, &world.cache(), 1).wait_with_output().unwrap();
    assert!(!failed.status.success());
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
    assert_eq!(world.state(1), "closed");
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
    assert_eq!(chain(&world)["candidates"].as_array().unwrap().len(), 1);
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
    let (b, _) = worker(&world, 2, "b");
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
    let bb = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
    assert!(!ordinary_marker.exists(), "ordinary work retains repo exclusion after restart");
    assert_eq!(chain(&world)["candidates"].as_array().unwrap().len(), 2);
    release(&world, "b", false);
    green(&world, 2);
    release(&world, "a", false);
    assert_eq!(finish(aa)["outcome"], "merged");
    assert_eq!(finish(bb)["outcome"], "merged");
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
    let (b, _) = worker(&world, 2, "b");
    let runner = world.host_held();
    let mut aa = start(&world, &a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
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
    release(&world, "a", true);
    until(|| world.root.join("cancellation-durable").exists());
    aa.kill().unwrap();
    let _ = aa.wait();
    world.kill_host_runner("KILL");
    drop(runner);
    let _replacement = world.host_held();
    assert_eq!(finish(bb)["outcome"], "cancelled");
    gone(&world, "a");
    gone(&world, "b");
    let state = chain(&world);
    assert_eq!(state["candidates"][0]["phase"], "red");
    assert_eq!(state["candidates"][0]["result"]["status"], "red");
    release(&world, "b", false);
    let rebuilt = finish(start(&world, &b, &world.cache(), 2));
    assert_eq!(rebuilt["outcome"], "merged", "{rebuilt}");
    assert!(
        !world.remote(&["ls-tree", "--name-only", "refs/heads/main"]).lines().any(|f| f == "a.txt")
    );
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
fn a_stopped_candidate_loses_its_worktree_and_a_live_one_keeps_it() {
    use std::os::unix::fs::PermissionsExt;
    let world = world(2);
    seal_gate(&world);
    let (issue_a, _) = worker(&world, 1, "a");
    let (issue_b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let aa = start(&world, &issue_a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &issue_b, &world.cache(), 2);
    began(&world, "b");
    let a_tree = candidate_path(&world, 1);
    let b_tree = candidate_path(&world, 2);
    assert_eq!(chain(&world)["candidates"][0]["phase"], "validating");
    assert!(listed(&world, &a_tree), "a validating candidate stays listed");
    assert!(listed(&world, &b_tree), "a validating candidate stays listed");
    release(&world, "b", false);
    green(&world, 2);
    let sealed = b_tree.join("sealed");
    let mode = std::fs::metadata(&sealed).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o555, "the gate left a directory the owner cannot write");
    assert!(
        listed(&world, &b_tree),
        "a green candidate waiting on its predecessor keeps its worktree"
    );
    let hold = Hold(Command::new("sleep").arg("300").current_dir(&a_tree).spawn().unwrap());
    release(&world, "a", false);
    let landed_a = finish(aa);
    let landed_b = finish(bb);
    assert_eq!(landed_a["outcome"], "merged", "{landed_a}");
    assert_eq!(landed_b["outcome"], "merged", "{landed_b}");
    assert!(a_tree.is_dir(), "a worktree a process is working in stays");
    assert!(listed(&world, &a_tree));
    assert!(!b_tree.exists(), "a landed candidate's worktree is gone, sealed directory included");
    assert!(!listed(&world, &b_tree));
    assert!(listed(&world, &issue_a) && listed(&world, &issue_b), "the issue worktrees stay");
    let state = chain(&world);
    for (issue, landed) in [(1, &landed_a), (2, &landed_b)] {
        let candidate = state["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["issue"] == issue)
            .unwrap();
        assert_eq!(candidate["phase"], "landed");
        assert_eq!(candidate["commit"], landed["commit"]);
    }
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
    assert!(issue_a.is_dir() && issue_b.is_dir());
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
fn a_red_candidate_and_a_cancelled_one_lose_their_worktrees() {
    let world = world(2);
    seal_gate(&world);
    let (issue_a, _) = worker(&world, 1, "a");
    let (issue_b, _) = worker(&world, 2, "b");
    let _runner = world.host_held();
    world.watch_git();
    let aa = start(&world, &issue_a, &world.cache(), 1);
    began(&world, "a");
    let bb = start(&world, &issue_b, &world.cache(), 2);
    began(&world, "b");
    let a_tree = candidate_path(&world, 1);
    let b_tree = candidate_path(&world, 2);
    release(&world, "a", true);
    let red = finish(aa);
    let cancelled = finish(bb);
    assert_eq!(red["outcome"], "gate_red", "{red}");
    assert_eq!(cancelled["outcome"], "cancelled", "{cancelled}");
    assert!(!a_tree.exists(), "a red candidate's worktree is gone");
    assert!(!listed(&world, &a_tree));
    assert!(!b_tree.exists(), "a cancelled candidate's worktree is gone");
    assert!(!listed(&world, &b_tree));
    let calls = world.git_calls();
    let wrote = calls
        .iter()
        .rposition(|call| call.starts_with("push ") && call.contains("refs/convoy/candidates/"));
    let reread = calls.iter().rposition(|call| call == "ls-remote origin refs/convoy/candidates/*");
    assert!(
        wrote.is_some_and(|wrote| reread.is_some_and(|reread| reread > wrote)),
        "a turn reads the chain it just wrote before it returns: {calls:#?}"
    );
    assert!(listed(&world, &issue_a) && listed(&world, &issue_b), "the issue worktrees stay");
    let state = chain(&world);
    let failed = state["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["issue"] == 1)
        .unwrap();
    let suffix = state["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["issue"] == 2)
        .unwrap();
    assert_eq!(failed["phase"], "red");
    assert!(failed["result"]["excerpt"].is_array(), "the chain keeps the gate excerpt: {failed}");
    assert_eq!(failed["result"]["exit_code"], 1, "{failed}");
    assert!(failed["cause"].as_str().is_some_and(|cause| !cause.is_empty()), "{failed}");
    assert_eq!(suffix["phase"], "cancelled");
    assert_eq!(suffix["cancelled_by"], failed["id"]);
    assert!(suffix["cause"].as_str().is_some_and(|cause| !cause.is_empty()), "{suffix}");
    assert_eq!(suffix["cause"], "predecessor_failed");
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

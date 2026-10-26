//! Real binary, real remotes and kernel processes. Gates wait on controlled
//! release files; assertions inspect refs, original heads and child exit.
use crate::common::{World, commit, git};
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
    assert_eq!(world.afk(&["doctor"]).json["candidates"][2]["waiting"], "landed");
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
        world.afk(&["doctor"]).json["host"]["exclusive"]["lanes"]["worker"]["waiting"]
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
    let bb = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
    assert_eq!(chain(&world)["candidates"].as_array().unwrap().len(), 2);
    release(&world, "b", false);
    green(&world, 2);
    release(&world, "a", false);
    assert_eq!(finish(aa)["outcome"], "merged");
    assert_eq!(finish(bb)["outcome"], "merged");
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

#[test]
#[ignore = "controlled timing experiment; run explicitly through afkconvoy exclusive"]
fn measure_serial_and_parallel_batches() {
    let mut batches = vec![];
    for lanes in [1, 3] {
        let world = world(lanes);
        let gate=std::fs::read_to_string(world.clone.join("gate.sh")).unwrap().replace(
            "while test -d \"$root\" && test ! -f \"$root/release-$name\"; do sleep 0.02; done",
            "awk 'BEGIN { n=500000; for(i=0;i<n;i++)a[i]=i; for(j=0;j<40;j++)for(i=0;i<n;i++)s+=a[i]; print s }' >/dev/null");
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
        batches.push(json!({"lanes":lanes,"batch_seconds":began.elapsed().as_secs_f64(),"gate_seconds":seconds,"discarded_gate_seconds":0}));
    }
    eprintln!("CANDIDATE_BATCH_MEASUREMENT={}", json!(batches));
}

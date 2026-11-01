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
        let ours = Command::new("ps")
            .args(["-p", &std::process::id().to_string(), "-o", "pgid="])
            .output()
            .unwrap();
        let ours = String::from_utf8_lossy(&ours.stdout).trim().to_string();
        for name in ["a", "b", "c", "d"] {
            for kind in ["gate", "child"] {
                let Ok(pid) = std::fs::read_to_string(self.root.join(format!("{kind}-{name}")))
                else {
                    continue;
                };
                let pid = pid.trim();
                let group = Command::new("ps").args(["-p", pid, "-o", "pgid="]).output().unwrap();
                let group = String::from_utf8_lossy(&group.stdout).trim().to_string();
                if !group.is_empty() && group != ours {
                    let _ =
                        Command::new("kill").args(["-KILL", "--", &format!("-{group}")]).status();
                }
                let _ = Command::new("kill").args(["-KILL", pid]).status();
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
fn stopped_candidates_in_the_old_root_are_removed_and_other_directories_are_left() {
    let world = World::new();
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
    // a's turn watches its gate limit; frozen, it cannot time a out before b
    // has begun, however long b takes to start on a loaded host.
    let held = Held::stop(&aa);
    let bb = start(&world, &b, &world.cache(), 2);
    began(&world, "b");
    drop(held);
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
    assert!(failed["not_recorded"].as_str().unwrap().contains("did not end within 1s"), "{failed}");
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
    // GitHub closes the issue as the merged PR says it, and refuses the
    // turn's own close at that moment.
    world.with_gh(|gh| {
        gh["fail"] = json!(["state=closed"]);
        let rows = gh["issues"].as_array_mut().unwrap();
        let issue = rows.iter_mut().find(|row| row["number"] == 1).unwrap();
        issue["state"] = json!("closed");
    });
    release(&world, "a", false);
    let landed = finish(start(&world, &a, &world.cache(), 1));
    assert_eq!(landed["outcome"], "merged", "{landed}");
    assert!(landed.get("settle_error").is_none(), "{landed}");
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
for n in a b c d; do if test -f "$n.txt"; then name=$n; fi; done
case "${{AFKCONVOY_ISSUE:-}}" in 1) name=a;; 2) name=b;; 3) name=c;; 4) name=d;; esac
test -n "$name" || exit 0
touch "$root/start-$part-$name"
while test -d "$root" && test ! -f "$root/release-$part-$name"; do sleep 0.02; done
test ! -f "$root/fail-$part-$name"
"#
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
fn a_round_slow_gates_its_tip_once_and_lands_every_fast_green_candidate_together() {
    let world = split_world(3);
    let (a, ah) = worker(&world, 1, "a");
    let (b, bh) = worker(&world, 2, "b");
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    until(|| started(&world, "fast", "a"));
    let bb = start(&world, &b, &world.cache(), 2);
    until(|| started(&world, "fast", "b"));
    let_go(&world, "fast", "a", false);
    green(&world, 1);
    assert_eq!(
        chain(&world)["candidates"][0]["slow"]["command"].as_str().map(|c| c.ends_with("slow")),
        Some(true)
    );
    let_go(&world, "fast", "b", false);
    until(|| started(&world, "slow", "b"));
    assert!(!started(&world, "slow", "a"), "only the round's tip runs the slow gate");
    let candidates = chain(&world)["candidates"].clone();
    assert_eq!(candidates[0]["phase"], "green");
    assert_eq!(candidates[1]["phase"], "slow");
    let_go(&world, "slow", "b", false);
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
fn a_red_round_is_fixed_whole_by_its_tip_and_its_other_candidates_stop() {
    let world = split_world(3);
    let (a, _) = worker(&world, 1, "a");
    let (b, _) = worker(&world, 2, "b");
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    until(|| started(&world, "fast", "a"));
    let bb = start(&world, &b, &world.cache(), 2);
    until(|| started(&world, "fast", "b"));
    let_go(&world, "fast", "a", false);
    let_go(&world, "fast", "b", false);
    until(|| started(&world, "slow", "b"));
    let tip = chain(&world)["candidates"][1]["commit"].as_str().unwrap().to_string();
    let_go(&world, "slow", "b", true);
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

#[test]
fn a_conflict_with_the_candidate_ahead_is_left_for_the_worker_to_resolve() {
    let world = split_world(3);
    let a = world.worker(1, "x.txt", "a");
    let b = world.worker(2, "x.txt", "b");
    let _runner = world.host_held();
    let aa = start(&world, &a, &world.cache(), 1);
    until(|| started(&world, "fast", "a"));
    let conflict = finish(start(&world, &b, &world.cache(), 2));
    assert_eq!(conflict["outcome"], "speculative_conflict", "{conflict}");
    assert_eq!(conflict["files"], json!(["x.txt"]));
    std::fs::write(b.join("x.txt"), "ab").unwrap();
    git(&b, &["commit", "-q", "-am", "resolve"]);
    let bb = start(&world, &b, &world.cache(), 2);
    until(|| started(&world, "fast", "b"));
    for part in ["fast", "slow"] {
        let_go(&world, part, "a", false);
        let_go(&world, part, "b", false);
    }
    assert_eq!(finish(aa)["outcome"], "merged");
    assert_eq!(finish(bb)["outcome"], "merged");
    assert_eq!(world.remote(&["show", "refs/heads/main:x.txt"]), "ab");
}

#[test]
fn with_one_lane_every_ready_pr_fast_gates_in_turn_and_lands_in_one_round() {
    let world = split_world(1);
    let workers: Vec<_> = [(1, "a"), (2, "b"), (3, "c")]
        .into_iter()
        .map(|(n, name)| worker(&world, n, name))
        .collect();
    let _runner = world.host_held();
    let aa = start(&world, &workers[0].0, &world.cache(), 1);
    until(|| started(&world, "fast", "a"));
    let bb = start(&world, &workers[1].0, &world.cache(), 2);
    let cc = start(&world, &workers[2].0, &world.cache(), 3);
    until(|| chain(&world)["pending"].as_array().is_some_and(|p| p.len() == 2));
    assert!(!started(&world, "fast", "b"), "one lane runs one gate");
    let_go(&world, "fast", "a", false);
    // A fast-green candidate frees its lane, and waits for the PRs still pending.
    until(|| started(&world, "fast", "b") || started(&world, "fast", "c"));
    assert!(!started(&world, "slow", "a"), "a round waits for every ready PR");
    let_go(&world, "fast", "b", false);
    let_go(&world, "fast", "c", false);
    let tip = if started(&world, "fast", "c") { "c" } else { "b" };
    until(|| started(&world, "slow", "b") || started(&world, "slow", "c"));
    assert!(!started(&world, "slow", "a"));
    let_go(&world, "slow", "b", false);
    let_go(&world, "slow", "c", false);
    let answers = [finish(aa), finish(bb), finish(cc)];
    for answer in &answers {
        assert_eq!(answer["outcome"], "merged", "{answer}");
    }
    let rows = chain(&world)["candidates"].as_array().unwrap().clone();
    let slowed: Vec<_> = rows.iter().filter(|c| c["slowed"] == true).collect();
    assert_eq!(slowed.len(), 1, "one slow gate run for the three: {rows:?}");
    assert_eq!(
        world.remote(&["rev-parse", "refs/heads/main"]),
        rows[2]["commit"].as_str().unwrap()
    );
    let _ = tip;
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
            c["result"] = json!({"pad": pad});
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
/// left alone; past the lease, the next turn here takes its candidate over,
/// cancels the candidate behind it and stops that one's gate, and is
/// assigned one of its own. When the quiet host comes back, its turn finds
/// its candidate taken, stops its own gate, and answers cancelled.
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
    let asleep = Held::stop(&far);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let beat = (now + 1000).to_string();
    world.afk(&["heartbeat", "--instance", "farfleet", "--host", "far", "--now", &beat]).ok();

    // The chain heard nothing of it for longer than the lease, but its fleet
    // beat within it: it is alive, and a turn here lines up behind it.
    let behind = start_at(&world, &b, 2, now + LEASE + 100);
    began(&world, "b");
    let first = candidate_of(&world, 1);
    assert_eq!(
        (first["phase"].clone(), first["cancelled_by"].clone()),
        (json!("validating"), Value::Null)
    );
    assert_eq!(candidate_of(&world, 2)["predecessor"], first["commit"]);

    // Past the lease on both counts: it is dead.
    let next = start_at(&world, &c, 3, now + 1001 + 2 * LEASE);
    until(|| candidate_of(&world, 3)["phase"] == "validating");
    let first = candidate_of(&world, 1);
    assert_eq!(
        (first["phase"].clone(), first["cause"].clone(), first["abandoned"].clone()),
        (json!("cancelled"), json!("owner_died"), json!(true)),
        "{first}"
    );
    assert_eq!(first["stopped"], false, "nobody here confirmed its processes gone");
    assert_eq!(candidate_of(&world, 3)["predecessor"], json!(base));
    let cancelled = finish(behind);
    assert_eq!(
        (cancelled["outcome"].clone(), cancelled["cancelled_by"].clone()),
        (json!("cancelled"), first["id"].clone()),
        "the candidate behind it cancels as behind any failed predecessor: {cancelled}"
    );
    gone(&world, "b");
    release(&world, "c", false);
    assert_eq!(finish(next)["outcome"], "merged");

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
